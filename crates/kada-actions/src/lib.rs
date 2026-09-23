//! 动作执行引擎：把「触发 → 执行一串动作」的落地逻辑从桌面壳迁出，独立成 crate。
//!
//! 依赖 `kada-core`（动作/配置模型）与 `kada-hook`（文本/按键模拟注入）。
//! 命令类动作的执行结果以 [`CommandResult`] 形式产出，由调用方（桌面壳）通过
//! 回调提交进消息中心；本 crate 不依赖 Tauri，可独立单测。

use std::time::Duration;

use serde::Serialize;

#[cfg(feature = "automation")]
use std::fs;
#[cfg(feature = "automation")]
use std::path::Path;
#[cfg(feature = "automation")]
use std::sync::atomic::{AtomicU64, Ordering};

use kada_core::{substitute_vars, Action, FrontmostContext, Key, TextMode, Vars};

#[cfg(feature = "automation")]
use kada_core::{AppOperation, FileObject, OsOperation, Shell, TextValue, Value};

#[cfg(target_os = "windows")]
use kada_hook::win::simulate;
#[cfg(target_os = "linux")]
use kada_hook::linux::simulate;

/// 一次命令类动作（CMD / PowerShell / 关闭程序 / 脚本）的执行结果，进入消息中心。
#[derive(Clone, Serialize)]
pub struct CommandResult {
    /// 动作种类："cmd" | "powershell" | "close_program" | "script"。
    pub kind: String,
    /// 展示标签："CMD" / "PowerShell" / "关闭程序" / "脚本"。
    pub label: String,
    /// 实际执行的命令文本。
    pub command: String,
    /// 触发它的快捷键（已渲染为 "Ctrl+Alt+C" 形式）。
    pub trigger: String,
    /// 快捷键名称（无名称时为空字符串）。
    pub name: String,
    /// 标准输出（UTF-8）。
    pub stdout: String,
    /// 标准错误（UTF-8）。
    pub stderr: String,
    /// 进程退出码；进程未能启动时为 None。
    pub exit_code: Option<i32>,
    /// 是否弹出结果弹窗（仅影响展示，不影响记录）。
    pub show_output: bool,
    /// 执行时间（本地时区，"YYYY-MM-DD HH:MM:SS"）。
    pub time: String,
}

/// 递归执行一串动作：遇到「条件判断」动作时按条件求值选择 `then` / `otherwise` 分支继续。
/// 命令类动作的结果通过 `commit` 回调产出（由调用方决定如何进消息中心）。
/// `vars` 承载本次触发内的变量，`last_copied` 记录最近一次复制/剪切的来源，
/// `frontmost` 为触发时的前台窗口上下文（供「前台应用/窗口」类条件求值）。
#[cfg(any(target_os = "windows", target_os = "linux"))]
#[cfg_attr(not(feature = "automation"), allow(unused_variables))]
pub fn run_actions(
    commit: &mut dyn FnMut(CommandResult),
    actions: &[Action],
    trigger: &str,
    name: &str,
    vars: &mut Vars,
    last_copied: &mut Option<String>,
    frontmost: Option<&FrontmostContext>,
) {
    for action in actions {
        #[cfg(feature = "automation")]
        if let Action::If { condition, then, otherwise, .. } = action {
            let branch = if condition.matches(vars, frontmost) { then } else { otherwise };
            run_actions(commit, branch, trigger, name, vars, last_copied, frontmost);
            continue;
        }
        match run_action(action, trigger, name, vars, last_copied) {
            Ok(Some(result)) => commit(result),
            Ok(None) => {}
            Err(e) => eprintln!("动作执行失败: {e}"),
        }
    }
}

/// 顺序执行一个动作。文本/按键走模拟输入，进程类走 std::process，文件类走 std::fs。
/// 命令类动作（CMD/PowerShell/关闭程序）捕获输出并返回 `Some(CommandResult)`，其余返回 `None`。
/// `vars` 承载本次触发内的变量（`GetFileProps` 写 File、`App::Status` 写 Bool、
/// 命令动作 `var` 非空时写 Text），供后续动作用占位符引用；
/// `last_copied` 记录最近一次复制/剪切的来源，供「粘贴」动作使用。
#[cfg(any(target_os = "windows", target_os = "linux"))]
#[cfg_attr(not(feature = "automation"), allow(unused_variables))]
fn run_action(
    action: &Action,
    trigger: &str,
    name: &str,
    vars: &mut Vars,
    last_copied: &mut Option<String>,
) -> Result<Option<CommandResult>, String> {
    match action {
        Action::Text { text, mode, .. } => match mode {
            TextMode::Input => {
                let text = substitute_vars(text, vars);
                simulate::type_text(&text).map_err(|e| e.to_string())?;
            }
            TextMode::ToUpper | TextMode::ToLower => {
                transform_case(matches!(mode, TextMode::ToUpper))?;
            }
        },
        Action::Keys { keys, .. } => {
            let ks: Vec<Key> = keys
                .iter()
                .map(|k| k.parse::<Key>().map_err(|e| e.to_string()))
                .collect::<Result<_, _>>()?;
            // 全部按下 → 稍停 → 逆序松开
            for k in &ks {
                simulate::down(*k);
            }
            std::thread::sleep(Duration::from_millis(30));
            for k in ks.iter().rev() {
                simulate::up(*k);
            }
        }
        Action::PauseMs { ms, .. } => std::thread::sleep(Duration::from_millis(*ms)),
        #[cfg(feature = "automation")]
        Action::Command { shell, command, show_output, var, .. } => {
            let (label, kind, is_powershell) = match shell {
                Shell::Cmd => ("CMD", "cmd", false),
                Shell::Powershell => ("PowerShell", "powershell", true),
            };
            if is_powershell && !cfg!(target_os = "windows") {
                return Err("PowerShell 命令仅 Windows 可用".into());
            }
            let command = substitute_vars(command, vars);
            let (stdout, stderr, exit_code) = run_cmd(label, is_powershell, &command)?;
            if !var.trim().is_empty() {
                vars.insert(var.trim().to_string(), Value::Text(TextValue { text: stdout.trim().to_string(), exit_code }));
            }
            return Ok(Some(CommandResult {
                kind: kind.into(),
                label: label.into(),
                command,
                trigger: trigger.into(),
                name: name.into(),
                stdout,
                stderr,
                exit_code,
                show_output: *show_output,
                time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            }));
        }
        #[cfg(feature = "automation")]
        Action::Cmd { command, show_output, var, .. } => {
            let command = substitute_vars(command, vars);
            let (stdout, stderr, exit_code) = run_cmd("CMD", false, &command)?;
            if !var.trim().is_empty() {
                vars.insert(var.trim().to_string(), Value::Text(TextValue { text: stdout.trim().to_string(), exit_code }));
            }
            return Ok(Some(CommandResult {
                kind: "cmd".into(),
                label: "CMD".into(),
                command,
                trigger: trigger.into(),
                name: name.into(),
                stdout,
                stderr,
                exit_code,
                show_output: *show_output,
                time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            }));
        }
        #[cfg(feature = "automation")]
        Action::Powershell { command, show_output, var, .. } => {
            if !cfg!(target_os = "windows") {
                return Err("PowerShell 动作仅 Windows 可用".into());
            }
            let command = substitute_vars(command, vars);
            let (stdout, stderr, exit_code) = run_cmd("PowerShell", true, &command)?;
            if !var.trim().is_empty() {
                vars.insert(var.trim().to_string(), Value::Text(TextValue { text: stdout.trim().to_string(), exit_code }));
            }
            return Ok(Some(CommandResult {
                kind: "powershell".into(),
                label: "PowerShell".into(),
                command,
                trigger: trigger.into(),
                name: name.into(),
                stdout,
                stderr,
                exit_code,
                show_output: *show_output,
                time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            }));
        }
        #[cfg(feature = "automation")]
        Action::Launch { program, args, .. } => {
            // 旧版动作（正常流程已在加载时迁移到 App，此处兜底）。
            let program = substitute_vars(program, vars);
            let args: Vec<String> = args.iter().map(|a| substitute_vars(a, vars)).collect();
            launch_program(&program, &args)?;
        }
        #[cfg(feature = "automation")]
        Action::CloseProgram { program, .. } => {
            // 旧版动作（正常流程已在加载时迁移到 App，此处兜底）。
            let program = substitute_vars(program, vars);
            return Ok(Some(close_program(&program, trigger, name)?));
        }
        #[cfg(feature = "automation")]
        Action::OpenFolder { path, .. } => {
            // 旧版动作（正常流程已在加载时迁移到 Os::OpenFolder，此处兜底）。
            run_os(&OsOperation::OpenFolder { path: path.clone() }, vars, last_copied)?;
        }
        #[cfg(feature = "automation")]
        Action::Os { operation, .. } => {
            run_os(operation, vars, last_copied)?;
        }
        #[cfg(feature = "automation")]
        Action::App { operation, .. } => {
            return run_app(operation, trigger, name, vars);
        }
        #[cfg(feature = "automation")]
        Action::OpenUrl { url, .. } => {
            let url = substitute_vars(url, vars);
            open_url(&url)?;
        }
        #[cfg(feature = "automation")]
        Action::If { .. } => {
            // 条件判断动作由 run_actions 拦截处理，不会到达这里。
            return Err("内部错误：条件判断动作应在运行器内处理".into());
        }
        #[cfg(feature = "automation")]
        Action::Script { path, interpreter, show_output, var, .. } => {
            let path = substitute_vars(path, vars);
            let (stdout, stderr, exit_code) =
                run_script(interpreter.as_deref(), &path, trigger, name, vars)?;
            if !var.trim().is_empty() {
                vars.insert(var.trim().to_string(), Value::Text(TextValue { text: stdout.trim().to_string(), exit_code }));
            }
            let command = match interpreter {
                Some(i) => format!("{i} \"{path}\""),
                None => path.clone(),
            };
            return Ok(Some(CommandResult {
                kind: "script".into(),
                label: "脚本".into(),
                command,
                trigger: trigger.into(),
                name: name.into(),
                stdout,
                stderr,
                exit_code,
                show_output: *show_output,
                time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            }));
        }
    }
    Ok(None)
}

/// 执行一个操作系统动作（文件复制/剪切/粘贴/删除/新建/压缩/取属性）。
#[cfg(all(any(target_os = "windows", target_os = "linux"), feature = "automation"))]
fn run_os(
    op: &OsOperation,
    vars: &mut Vars,
    last_copied: &mut Option<String>,
) -> Result<(), String> {
    match op {
        OsOperation::Copy { source, dest } => {
            let s = substitute_vars(source, vars);
            let d = substitute_vars(dest, vars);
            copy_path(&s, &d)?;
            *last_copied = Some(s);
        }
        OsOperation::Cut { source, dest } => {
            let s = substitute_vars(source, vars);
            let d = substitute_vars(dest, vars);
            move_path(&s, &d)?;
            *last_copied = Some(d);
        }
        OsOperation::Paste { dest } => {
            let d = substitute_vars(dest, vars);
            let s = last_copied
                .clone()
                .ok_or_else(|| "粘贴失败：尚无复制/剪切的来源".to_string())?;
            copy_path(&s, &d)?;
        }
        OsOperation::Delete { path } => {
            let p = substitute_vars(path, vars);
            delete_path(&p)?;
        }
        OsOperation::NewFile { path } => {
            let p = substitute_vars(path, vars);
            new_file(&p)?;
        }
        OsOperation::NewFolder { path } => {
            let p = substitute_vars(path, vars);
            std::fs::create_dir_all(&p).map_err(|e| format!("新建目录失败: {e}"))?;
        }
        OsOperation::OpenFolder { path } => {
            let p = substitute_vars(path, vars);
            if cfg!(target_os = "windows") {
                std::process::Command::new("explorer")
                    .arg(&p)
                    .spawn()
                    .map_err(|e| format!("打开目录失败: {e}"))?;
            } else {
                std::process::Command::new("xdg-open")
                    .arg(&p)
                    .spawn()
                    .map_err(|e| format!("打开目录失败: {e}"))?;
            }
        }
        OsOperation::Zip { source, dest } => {
            let s = substitute_vars(source, vars);
            let d = substitute_vars(dest, vars);
            zip_path(&s, &d)?;
        }
        OsOperation::Unzip { source, dest } => {
            let s = substitute_vars(source, vars);
            let d = substitute_vars(dest, vars);
            unzip_path(&s, &d)?;
        }
        OsOperation::GetFileProps { path, var } => {
            let p = substitute_vars(path, vars);
            let obj = file_object(&p)?;
            let name = if var.trim().is_empty() { "file".to_string() } else { var.clone() };
            vars.insert(name, Value::File(obj));
        }
    }
    Ok(())
}

/// 把当前选中/剪贴板文本转为大写或小写后粘贴回原处：
/// 复制选中（Ctrl+C）→ 读剪贴板 → 转换 → 粘贴（Ctrl+V）。
/// 无选中时 Ctrl+C 通常不改剪贴板，即退化为「转换剪贴板文本」。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn transform_case(upper: bool) -> Result<(), String> {
    simulate::chord(&[Key::Control, Key::C]);
    std::thread::sleep(Duration::from_millis(80));
    let clip = simulate::get_clipboard_text().map_err(|e| e.to_string())?;
    let out = if upper { clip.to_uppercase() } else { clip.to_lowercase() };
    simulate::type_text(&out).map_err(|e| e.to_string())?;
    Ok(())
}

/// 执行一个应用动作（打开/关闭/查询状态/重启）。
#[cfg(all(any(target_os = "windows", target_os = "linux"), feature = "automation"))]
fn run_app(
    op: &AppOperation,
    trigger: &str,
    name: &str,
    vars: &mut Vars,
) -> Result<Option<CommandResult>, String> {
    match op {
        AppOperation::Launch { program, args } => {
            let program = substitute_vars(program, vars);
            let args: Vec<String> = args.iter().map(|a| substitute_vars(a, vars)).collect();
            launch_program(&program, &args)?;
            Ok(None)
        }
        AppOperation::Close { program } => {
            let program = substitute_vars(program, vars);
            Ok(Some(close_program(&program, trigger, name)?))
        }
        AppOperation::Status { program, var, retries, interval_ms } => {
            let program = substitute_vars(program, vars);
            let image = image_name(&program);
            // 未运行则按 retries/interval_ms 轮询，等程序起来（如「启动后等它就绪」）。
            let mut running = app_running(&image);
            for _ in 0..*retries {
                if running {
                    break;
                }
                std::thread::sleep(Duration::from_millis(*interval_ms));
                running = app_running(&image);
            }
            let name = if var.trim().is_empty() { "app".to_string() } else { var.clone() };
            vars.insert(name, Value::Bool(running));
            Ok(None)
        }
        AppOperation::Restart { program, args } => {
            let program = substitute_vars(program, vars);
            let args: Vec<String> = args.iter().map(|a| substitute_vars(a, vars)).collect();
            restart_program(&program, &args)?;
            Ok(None)
        }
    }
}

/// 启动程序（可选参数）。
#[cfg(all(any(target_os = "windows", target_os = "linux"), feature = "automation"))]
fn launch_program(program: &str, args: &[String]) -> Result<(), String> {
    std::process::Command::new(program)
        .args(args)
        .spawn()
        .map_err(|e| format!("启动程序「{program}」失败: {e}"))?;
    Ok(())
}

/// 用系统默认浏览器打开网址。Windows 走 `explorer`（直接以参数传入，不经 shell，
/// 避免 URL 里的 `&`/`?` 等被 cmd 二次解析）；Linux 走 `xdg-open`。
#[cfg(all(any(target_os = "windows", target_os = "linux"), feature = "automation"))]
fn open_url(url: &str) -> Result<(), String> {
    let res = if cfg!(target_os = "windows") {
        std::process::Command::new("explorer").arg(url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    res.map(|_| ()).map_err(|e| format!("打开网址「{url}」失败: {e}"))
}

/// 关闭程序（结束所有同名进程），返回命令结果供消息中心展示。
#[cfg(all(any(target_os = "windows", target_os = "linux"), feature = "automation"))]
fn close_program(program: &str, trigger: &str, name: &str) -> Result<CommandResult, String> {
    let image = image_name(program);
    let cmd = if cfg!(target_os = "windows") {
        format!("taskkill /IM {image} /F /T")
    } else {
        format!("pkill -f {image}")
    };
    let (stdout, stderr, exit_code) = run_cmd("关闭程序", false, &cmd)?;
    Ok(CommandResult {
        kind: "close_program".into(),
        label: "关闭程序".into(),
        command: cmd,
        trigger: trigger.into(),
        name: name.into(),
        stdout,
        stderr,
        exit_code,
        show_output: false,
        time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    })
}

/// 重启程序：先关闭（忽略「未在运行」），再启动。
#[cfg(all(any(target_os = "windows", target_os = "linux"), feature = "automation"))]
fn restart_program(program: &str, args: &[String]) -> Result<(), String> {
    let _ = close_program(program, "", "");
    launch_program(program, args)?;
    Ok(())
}

/// 判断程序是否正在运行。
#[cfg(all(any(target_os = "windows", target_os = "linux"), feature = "automation"))]
fn app_running(program: &str) -> bool {
    if cfg!(target_os = "windows") {
        match std::process::Command::new("tasklist")
            .args(["/FI", &format!("IMAGENAME eq {program}"), "/NH"])
            .output()
        {
            Ok(o) => String::from_utf8_lossy(&o.stdout)
                .to_lowercase()
                .contains(&program.to_lowercase()),
            Err(_) => false,
        }
    } else {
        std::process::Command::new("pgrep")
            .args(["-x", program])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// 取程序名（镜像名）：`program` 为带路径形式时取最后一段文件名，否则原样返回。
#[cfg(all(any(target_os = "windows", target_os = "linux"), feature = "automation"))]
fn image_name(program: &str) -> String {
    Path::new(program)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| program.to_string())
}

/// 复制文件或目录（目录递归）。`dest` 为已存在目录时复制到其下，否则视为完整目标路径。
#[cfg(all(any(target_os = "windows", target_os = "linux"), feature = "automation"))]
fn copy_path(source: &str, dest: &str) -> Result<(), String> {
    let src = Path::new(source);
    let dst = Path::new(dest);
    if !src.exists() {
        return Err(format!("复制失败：源「{source}」不存在"));
    }
    let target = if dst.is_dir() {
        dst.join(src.file_name().ok_or("源路径无效")?)
    } else {
        dst.to_path_buf()
    };
    if let Some(parent) = target.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if src.is_dir() {
        copy_dir_rec(src, &target).map_err(|e| format!("复制目录失败: {e}"))
    } else {
        fs::copy(src, &target).map_err(|e| format!("复制文件失败: {e}"))?;
        Ok(())
    }
}

#[cfg(all(any(target_os = "windows", target_os = "linux"), feature = "automation"))]
fn copy_dir_rec(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_rec(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// 移动文件或目录（同盘 `rename`；跨盘回退为复制后删除源）。
#[cfg(all(any(target_os = "windows", target_os = "linux"), feature = "automation"))]
fn move_path(source: &str, dest: &str) -> Result<(), String> {
    let src = Path::new(source);
    if !src.exists() {
        return Err(format!("剪切失败：源「{source}」不存在"));
    }
    let dst = Path::new(dest);
    let target = if dst.is_dir() {
        dst.join(src.file_name().ok_or("源路径无效")?)
    } else {
        dst.to_path_buf()
    };
    if let Some(parent) = target.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if fs::rename(src, &target).is_ok() {
        return Ok(());
    }
    // 跨盘 rename 失败：复制后删除源。
    copy_path(source, dest)?;
    delete_path(source)
}

/// 删除文件或目录（目录递归删除）。
#[cfg(all(any(target_os = "windows", target_os = "linux"), feature = "automation"))]
fn delete_path(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    if !p.exists() {
        return Err(format!("删除失败：「{path}」不存在"));
    }
    if p.is_dir() {
        fs::remove_dir_all(p).map_err(|e| format!("删除目录失败: {e}"))
    } else {
        fs::remove_file(p).map_err(|e| format!("删除文件失败: {e}"))
    }
}

/// 新建空文件（自动创建父目录；已存在则截断为空）。
#[cfg(all(any(target_os = "windows", target_os = "linux"), feature = "automation"))]
fn new_file(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建父目录失败: {e}"))?;
    }
    fs::write(p, b"").map_err(|e| format!("新建文件失败: {e}"))
}

/// 压缩为 zip：Windows 走 `Compress-Archive`，Linux 走 `zip`。
#[cfg(all(any(target_os = "windows", target_os = "linux"), feature = "automation"))]
fn zip_path(source: &str, dest: &str) -> Result<(), String> {
    let src = Path::new(source);
    if !src.exists() {
        return Err(format!("压缩失败：源「{source}」不存在"));
    }
    let dest = if dest.to_lowercase().ends_with(".zip") {
        dest.to_string()
    } else {
        format!("{dest}.zip")
    };
    if let Some(parent) = Path::new(&dest).parent() {
        if !parent.as_os_str().is_empty() {
            let _ = fs::create_dir_all(parent);
        }
    }
    let status = if cfg!(target_os = "windows") {
        let full =
            format!("Compress-Archive -Path '{}' -DestinationPath '{}' -Force", source, dest);
        std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", full.as_str()])
            .status()
    } else {
        std::process::Command::new("zip").args(["-r", &dest, source]).status()
    }
    .map_err(|e| format!("启动压缩命令失败: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("压缩失败（源不存在或系统缺少压缩组件）".into())
    }
}

/// 解压 zip：Windows 走 `Expand-Archive`，Linux 走 `unzip`。
#[cfg(all(any(target_os = "windows", target_os = "linux"), feature = "automation"))]
fn unzip_path(source: &str, dest: &str) -> Result<(), String> {
    let src = Path::new(source);
    if !src.exists() {
        return Err(format!("解压失败：压缩包「{source}」不存在"));
    }
    fs::create_dir_all(dest).map_err(|e| format!("创建解压目录失败: {e}"))?;
    let status = if cfg!(target_os = "windows") {
        let full =
            format!("Expand-Archive -Path '{}' -DestinationPath '{}' -Force", source, dest);
        std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", full.as_str()])
            .status()
    } else {
        std::process::Command::new("unzip")
            .args(["-o", source, "-d", dest])
            .status()
    }
    .map_err(|e| format!("启动解压命令失败: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("解压失败（压缩包损坏或系统缺少解压组件）".into())
    }
}

/// 读取文件/目录属性，构造 [`FileObject`]。
#[cfg(all(any(target_os = "windows", target_os = "linux"), feature = "automation"))]
fn file_object(path: &str) -> Result<FileObject, String> {
    let p = Path::new(path);
    let meta = fs::metadata(p).map_err(|e| format!("读取属性失败：{e}"))?;
    let is_dir = meta.is_dir();
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = p.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let size = if is_dir { 0 } else { meta.len() };
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(FileObject { name, path: path.to_string(), dir, stem, ext, size, modified, is_dir })
}

/// 执行 shell 命令并捕获 UTF-8 输出。
/// Windows 下 cmd 前缀 `chcp 65001`、PowerShell 前缀设置输出编码，保证中文不乱码；
/// Linux 下 CMD 走 `sh -c`。
#[cfg(all(any(target_os = "windows", target_os = "linux"), feature = "automation"))]
fn run_cmd(label: &str, is_powershell: bool, command: &str) -> Result<(String, String, Option<i32>), String> {
    let output = if cfg!(target_os = "windows") {
        if is_powershell {
            let full = format!("[Console]::OutputEncoding=[Text.Encoding]::UTF8; {command}");
            std::process::Command::new("powershell")
                .args(["-NoProfile", "-Command", full.as_str()])
                .output()
        } else {
            // Windows：命令可能含嵌套引号（如 `start "" /min cmd /c "…"`），直接 `cmd /C "…"`
            // 会被 Rust 的 argv 转义成 `\"`，而 cmd 不认反斜杠转义，导致「xxx 不是内部或外部命令」。
            // 改为写入临时批处理文件再执行，彻底绕开引号二次解析；chcp 单独一行保证中文输出不乱码。
            static SERIAL: AtomicU64 = AtomicU64::new(0);
            let id = SERIAL.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("kada_cmd_{}_{}.bat", std::process::id(), id));
            let content = format!("@echo off\r\nchcp 65001>nul\r\n{command}\r\n");
            let run = std::fs::write(&path, content.as_bytes())
                .and_then(|_| std::process::Command::new("cmd").arg("/C").arg(&path).output());
            let _ = std::fs::remove_file(&path);
            run
        }
    } else {
        std::process::Command::new("sh").args(["-c", command]).output()
    }
    .map_err(|e| format!("执行 {label} 命令失败: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    Ok((stdout, stderr, output.status.code()))
}

/// 执行脚本文件：可选解释器（`None` = 直接执行脚本本身），注入环境变量
/// `KADA_TRIGGER`（触发组合）、`KADA_NAME`（快捷键名）、`KADA_VARS`（变量表 JSON）。
#[cfg(all(any(target_os = "windows", target_os = "linux"), feature = "automation"))]
fn run_script(
    interpreter: Option<&str>,
    path: &str,
    trigger: &str,
    name: &str,
    vars: &Vars,
) -> Result<(String, String, Option<i32>), String> {
    let mut cmd = match interpreter {
        Some(i) => {
            let mut c = std::process::Command::new(i);
            c.arg(path);
            c
        }
        None => std::process::Command::new(path),
    };
    let output = cmd
        .env("KADA_TRIGGER", trigger)
        .env("KADA_NAME", name)
        .env("KADA_VARS", vars_to_json(vars))
        .output()
        .map_err(|e| format!("执行脚本失败: {e}"))?;
    Ok((
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code(),
    ))
}

/// 把变量表序列化为 JSON 对象字符串（供脚本环境变量 `KADA_VARS` 使用）。
#[cfg(all(any(target_os = "windows", target_os = "linux"), feature = "automation"))]
fn vars_to_json(vars: &Vars) -> String {
    let mut map = serde_json::Map::new();
    for (name, value) in vars {
        let v = match value {
            Value::File(f) => serde_json::json!({
                "name": f.name,
                "path": f.path,
                "dir": f.dir,
                "stem": f.stem,
                "ext": f.ext,
                "size": f.size,
                "modified": f.modified,
                "is_dir": f.is_dir,
            }),
            Value::Bool(b) => serde_json::json!(b),
            Value::Text(t) => serde_json::json!({ "text": t.text, "exit_code": t.exit_code }),
        };
        map.insert(name.clone(), v);
    }
    serde_json::Value::Object(map).to_string()
}
