//! 系统代理开关（`SystemOperation::SwitchProxy`）。
//!
//! 三平台各走各的系统接口：
//!
//! - **Windows**：读写当前用户注册表 `Internet Settings` 的 `ProxyEnable` / `ProxyServer` /
//!   `ProxyOverride`（排除项），再广播 `InternetSetOption` 通知 WinINET 立即生效。
//! - **macOS**：调 `networksetup`，对「已启用的网络服务」逐个开关（HTTP / HTTPS / SOCKS 三类）
//!   并写排除域名（`-setproxybypassdomains`）。修改系统网络设置需要管理员权限；失败时把系统
//!   给的原文带回去，不静默。
//! - **Linux**：调 GNOME 的 `gsettings`（`org.gnome.system.proxy`，排除项是 `ignore-hosts`
//!   字符串数组）。其它桌面环境没有统一接口，gsettings 不可用时明确报错。
//!
//! 这里只负责「改系统状态」，动作模型 / 摘要 / 执行记录都在上层。

use kada_core::{ProxyMode, ProxyServer};

/// 期望的最终状态（纯函数）：`Toggle` 按当前状态取反。
fn desired(mode: ProxyMode, current: bool) -> bool {
    match mode {
        ProxyMode::On => true,
        ProxyMode::Off => false,
        ProxyMode::Toggle => !current,
    }
}

/// 开关系统代理，返回一句可进消息中心的结果说明。
///
/// `bypass` 是排除项（已拆分去重，见 `kada_core::parse_proxy_bypass`）：**空列表 = 不动
/// 系统里已有的排除项**（跟 `server` 留空沿用现有地址同理），避免一个「开 / 关」热键顺手
/// 把用户手工配好的例外清掉。
pub(super) fn switch(
    mode: ProxyMode,
    server: Option<&ProxyServer>,
    bypass: &[String],
) -> Result<String, String> {
    #[cfg(windows)]
    {
        windows::switch(mode, server, bypass)
    }
    #[cfg(target_os = "macos")]
    {
        macos::switch(mode, server, bypass)
    }
    #[cfg(target_os = "linux")]
    {
        linux::switch(mode, server, bypass)
    }
}

/// 结果说明：开启时带上地址（填了排除项再报一句项数），关闭时只报状态。
fn enabled_message(server: &str, bypass: usize) -> String {
    if bypass == 0 {
        format!("系统代理已开启（{server}）")
    } else {
        format!("系统代理已开启（{server}，{bypass} 项排除）")
    }
}

fn disabled_message() -> String {
    "系统代理已关闭".into()
}

#[cfg(windows)]
mod windows {
    use super::{desired, disabled_message, enabled_message};
    use kada_core::{ProxyMode, ProxyScheme, ProxyServer};
    use windows_registry::{Key, CURRENT_USER};

    /// WinINET 的当前用户代理设置（「设置 → 网络和 Internet → 代理」写的就是这里）。
    const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

    pub(super) fn switch(
        mode: ProxyMode,
        server: Option<&ProxyServer>,
        bypass: &[String],
    ) -> Result<String, String> {
        let key = CURRENT_USER
            .create(KEY)
            .map_err(|e| format!("打开系统代理设置失败: {e}"))?;
        let current = is_enabled(&key);
        let want = desired(mode, current);
        let mut shown = current_server(&key);

        if want {
            if let Some(server) = server {
                let value = registry_value(server);
                key.set_string("ProxyServer", value)
                    .map_err(|e| format!("写入代理地址失败: {e}"))?;
                shown = Some(server.display());
            }
            if shown.is_none() {
                return Err(
                    "系统里还没有代理地址：请在动作里填「代理地址」后再开启（关闭不需要地址）"
                        .into(),
                );
            }
            // 排除项留空就不碰系统里已有的 `ProxyOverride`（别把用户手工配好的例外清掉）。
            if !bypass.is_empty() {
                key.set_string("ProxyOverride", override_value(bypass))
                    .map_err(|e| format!("写入代理排除项失败: {e}"))?;
            }
        }

        key.set_u32("ProxyEnable", u32::from(want))
            .map_err(|e| format!("写入系统代理开关失败: {e}"))?;
        refresh();

        Ok(if want {
            enabled_message(
                shown.as_deref().unwrap_or("沿用系统里已有的地址"),
                bypass.len(),
            )
        } else {
            disabled_message()
        })
    }

    fn is_enabled(key: &Key) -> bool {
        key.get_u32("ProxyEnable").map(|v| v != 0).unwrap_or(false)
    }

    fn current_server(key: &Key) -> Option<String> {
        key.get_string("ProxyServer")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// 写进 `ProxyServer` 的字符串：HTTP 代理同时接管 http / https；SOCKS 只配 socks。
    /// 不写「裸 `host:port`」，免得 FTP 之类也走同一个代理、把另一类协议顶掉。
    fn registry_value(server: &ProxyServer) -> String {
        // IPv6 在代理串里要带方括号（`socks=[::1]:1080`），否则分不清哪段是端口。
        let host = if server.host.contains(':') {
            format!("[{}]", server.host)
        } else {
            server.host.clone()
        };
        match server.scheme {
            ProxyScheme::Http => {
                format!("http={host}:{port};https={host}:{port}", port = server.port)
            }
            ProxyScheme::Socks5 => format!("socks={host}:{}", server.port),
        }
    }

    /// 写进 `ProxyOverride` 的字符串：WinINET 用分号分隔，`<local>` 表示跳过本地（不带点）地址。
    fn override_value(bypass: &[String]) -> String {
        bypass.join(";")
    }

    /// 通知 WinINET「设置变了」，否则已打开的程序（浏览器）可能还拿旧设置。
    fn refresh() {
        use windows::Win32::Networking::WinInet::{
            InternetSetOptionW, INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED,
        };
        unsafe {
            let _ = InternetSetOptionW(None, INTERNET_OPTION_SETTINGS_CHANGED, None, 0);
            let _ = InternetSetOptionW(None, INTERNET_OPTION_REFRESH, None, 0);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{override_value, registry_value};
        use kada_core::{ProxyScheme, ProxyServer};

        #[test]
        fn registry_value_covers_http_and_socks() {
            let http = ProxyServer {
                scheme: ProxyScheme::Http,
                host: "127.0.0.1".into(),
                port: 7890,
            };
            assert_eq!(
                registry_value(&http),
                "http=127.0.0.1:7890;https=127.0.0.1:7890"
            );

            let socks = ProxyServer {
                scheme: ProxyScheme::Socks5,
                host: "::1".into(),
                port: 1080,
            };
            assert_eq!(registry_value(&socks), "socks=[::1]:1080");
        }

        #[test]
        fn override_value_is_semicolon_separated() {
            assert_eq!(
                override_value(&["localhost".into(), "192.168.*".into(), "<local>".into()]),
                "localhost;192.168.*;<local>"
            );
        }
    }
}

/// 从 `networksetup -getwebproxy` / `-getsecurewebproxy` / `-getsocksfirewallproxy`
/// 的输出里读 (是否启用, 服务器, 端口)。纯文本解析，好单测。
#[cfg(any(target_os = "macos", test))]
fn parse_networksetup_proxy(text: &str) -> (bool, String, String) {
    let mut enabled = false;
    let mut server = String::new();
    let mut port = String::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "Enabled" => enabled = value.eq_ignore_ascii_case("Yes"),
            "Server" => server = value.to_string(),
            "Port" => port = value.to_string(),
            _ => {}
        }
    }
    (enabled, server, port)
}

/// `networksetup -setproxybypassdomains <服务> <域名…>` 的参数串（纯函数，好单测）。
///
/// 空列表不该走到这里（调用方先判空、不碰系统里已有的排除项）。
#[cfg(any(target_os = "macos", test))]
fn bypass_args(service: &str, bypass: &[String]) -> Vec<String> {
    let mut args = vec!["-setproxybypassdomains".to_string(), service.to_string()];
    args.extend(bypass.iter().cloned());
    args
}

/// GNOME 的 `ignore-hosts` 是字符串数组（GVariant `as`）：拼成 `['a', 'b']`。
///
/// 只转义单引号与反斜杠——这两个不转义会把 GVariant 字面量拼坏，其余字符原样进去。
#[cfg(any(target_os = "linux", test))]
fn ignore_hosts_gvariant(bypass: &[String]) -> String {
    let mut out = String::from("[");
    for (i, item) in bypass.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push('\'');
        for c in item.chars() {
            if matches!(c, '\'' | '\\') {
                out.push('\\');
            }
            out.push(c);
        }
        out.push('\'');
    }
    out.push(']');
    out
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{
        bypass_args, desired, disabled_message, enabled_message, parse_networksetup_proxy,
    };
    use kada_core::{ProxyMode, ProxyScheme, ProxyServer};
    use std::process::Command;

    const NETWORKSETUP: &str = "/usr/sbin/networksetup";

    /// 三类代理的查询 / 设置 / 开关命令后缀（networksetup 的参数名）。
    const KINDS: [(&str, &str, &str); 3] = [
        ("-getwebproxy", "-setwebproxy", "-setwebproxystate"),
        (
            "-getsecurewebproxy",
            "-setsecurewebproxy",
            "-setsecurewebproxystate",
        ),
        (
            "-getsocksfirewallproxy",
            "-setsocksfirewallproxy",
            "-setsocksfirewallproxystate",
        ),
    ];

    pub(super) fn switch(
        mode: ProxyMode,
        server: Option<&ProxyServer>,
        bypass: &[String],
    ) -> Result<String, String> {
        let services = services()?;
        let current = services.iter().any(|service| {
            KINDS
                .iter()
                .any(|(get, _, _)| queried_enabled(service, get))
        });
        let want = desired(mode, current);

        if want {
            if let Some(server) = server {
                for service in &services {
                    apply_server(service, server)?;
                    apply_bypass(service, bypass)?;
                }
                return Ok(enabled_message(&server.display(), bypass.len()));
            }
            // 没填地址：把系统里已经配好地址的那几类打开；一个都没有就明确报错。
            let mut any = false;
            for service in &services {
                for (get, _, set_state) in KINDS {
                    let (_, host, _) = queried(service, get)?;
                    if !host.trim().is_empty() {
                        run(&[set_state, service, "on"])?;
                        any = true;
                    }
                }
            }
            if !any {
                return Err(
                    "系统里还没有代理地址：请在动作里填「代理地址」后再开启（关闭不需要地址）"
                        .into(),
                );
            }
            for service in &services {
                apply_bypass(service, bypass)?;
            }
            return Ok(enabled_message("沿用系统里已有的地址", bypass.len()));
        }

        for service in &services {
            for (_, _, set_state) in KINDS {
                run(&[set_state, service, "off"])?;
            }
        }
        Ok(disabled_message())
    }

    /// 已启用的网络服务（`-listallnetworkservices` 的首行是说明，`*` 开头的是停用服务）。
    fn services() -> Result<Vec<String>, String> {
        let out = run(&["-listallnetworkservices"])?;
        let list: Vec<String> = out
            .lines()
            .skip(1)
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('*'))
            .map(str::to_string)
            .collect();
        if list.is_empty() {
            return Err("找不到可用的网络服务（networksetup 没有列出任何已启用的服务）".into());
        }
        Ok(list)
    }

    fn queried(service: &str, get: &str) -> Result<(bool, String, String), String> {
        let out = run(&[get, service])?;
        Ok(parse_networksetup_proxy(&out))
    }

    fn queried_enabled(service: &str, get: &str) -> bool {
        queried(service, get).map(|(on, _, _)| on).unwrap_or(false)
    }

    /// 写入地址并启用指定的那一类代理，同时关掉另一类，避免两套代理同时生效。
    fn apply_server(service: &str, server: &ProxyServer) -> Result<(), String> {
        let port = server.port.to_string();
        match server.scheme {
            ProxyScheme::Http => {
                run(&["-setwebproxy", service, &server.host, &port])?;
                run(&["-setsecurewebproxy", service, &server.host, &port])?;
                run(&["-setsocksfirewallproxystate", service, "off"])?;
            }
            ProxyScheme::Socks5 => {
                run(&["-setsocksfirewallproxy", service, &server.host, &port])?;
                run(&["-setwebproxystate", service, "off"])?;
                run(&["-setsecurewebproxystate", service, "off"])?;
            }
        }
        Ok(())
    }

    /// 写排除域名（`-setproxybypassdomains` 后面跟一串域名）。留空 = 不动系统里已有的排除项。
    fn apply_bypass(service: &str, bypass: &[String]) -> Result<(), String> {
        if bypass.is_empty() {
            return Ok(());
        }
        let args = bypass_args(service, bypass);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        run(&refs).map(|_| ())
    }

    fn run(args: &[&str]) -> Result<String, String> {
        let out = Command::new(NETWORKSETUP)
            .args(args)
            .output()
            .map_err(|e| format!("调用 networksetup 失败: {e}"))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stdout = String::from_utf8_lossy(&out.stdout);
            let detail = format!("{}{}", stderr.trim(), stdout.trim())
                .trim()
                .to_string();
            let hint = if detail.to_ascii_lowercase().contains("admin") {
                "（修改 macOS 系统代理需要管理员权限，请把 Kada 以管理员身份运行）"
            } else {
                ""
            };
            return Err(format!(
                "networksetup {} 失败{hint}: {detail}",
                args.join(" ")
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{desired, disabled_message, enabled_message, ignore_hosts_gvariant};
    use kada_core::{ProxyMode, ProxyScheme, ProxyServer};
    use std::process::Command;

    /// GNOME 的系统代理 schema（Cinnamon / MATE 等沿用 GNOME 接口的桌面也能读）。
    const SCHEMA: &str = "org.gnome.system.proxy";

    pub(super) fn switch(
        mode: ProxyMode,
        server: Option<&ProxyServer>,
        bypass: &[String],
    ) -> Result<String, String> {
        let current = !matches!(get("mode")?.trim_matches('\''), "none");
        let want = desired(mode, current);

        if !want {
            set("mode", "none")?;
            return Ok(disabled_message());
        }

        match server {
            Some(server) => {
                apply_server(server)?;
                apply_bypass(bypass)?;
                set("mode", "manual")?;
                Ok(enabled_message(&server.display(), bypass.len()))
            }
            None => {
                if !has_configured_server()? {
                    return Err(
                        "系统里还没有代理地址：请在动作里填「代理地址」后再开启（关闭不需要地址）"
                            .into(),
                    );
                }
                apply_bypass(bypass)?;
                set("mode", "manual")?;
                Ok(enabled_message("沿用系统里已有的地址", bypass.len()))
            }
        }
    }

    /// 写排除项（`ignore-hosts` 字符串数组）。留空 = 不动系统里已有的排除项。
    fn apply_bypass(bypass: &[String]) -> Result<(), String> {
        if bypass.is_empty() {
            return Ok(());
        }
        set("ignore-hosts", &ignore_hosts_gvariant(bypass))
    }

    /// 写入地址：HTTP 代理同时接管 http / https，SOCKS5 只配 socks；另一类清空。
    fn apply_server(server: &ProxyServer) -> Result<(), String> {
        let port = server.port.to_string();
        match server.scheme {
            ProxyScheme::Http => {
                set("http host", &server.host)?;
                set("http port", &port)?;
                set("https host", &server.host)?;
                set("https port", &port)?;
                set("socks host", "")?;
            }
            ProxyScheme::Socks5 => {
                set("socks host", &server.host)?;
                set("socks port", &port)?;
                set("http host", "")?;
                set("https host", "")?;
            }
        }
        Ok(())
    }

    /// 系统里是否已经配了至少一个代理地址（三种协议任意一类有 host）。
    fn has_configured_server() -> Result<bool, String> {
        for key in ["http host", "https host", "socks host"] {
            if !get(key)?.trim().trim_matches('\'').is_empty() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn get(key: &str) -> Result<String, String> {
        run(&["get", SCHEMA, key])
    }

    fn set(key: &str, value: &str) -> Result<(), String> {
        run(&["set", SCHEMA, key, value]).map(|_| ())
    }

    fn run(args: &[&str]) -> Result<String, String> {
        let out = Command::new("gsettings")
            .args(args)
            .output()
            .map_err(|e| format!("调用 gsettings 失败（系统代理目前只支持 GNOME 桌面）: {e}"))?;
        if !out.status.success() {
            let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(format!(
                "gsettings {} 失败（系统代理目前只支持 GNOME 桌面）: {detail}",
                args.join(" ")
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desired_state_follows_mode() {
        assert!(desired(ProxyMode::On, false));
        assert!(desired(ProxyMode::On, true));
        assert!(!desired(ProxyMode::Off, false));
        assert!(!desired(ProxyMode::Off, true));
        assert!(desired(ProxyMode::Toggle, false));
        assert!(!desired(ProxyMode::Toggle, true));
    }

    #[test]
    fn parse_networksetup_output() {
        let on = "Enabled: Yes\nServer: 127.0.0.1\nPort: 7890\nAuthenticated Proxy Enabled: 0\n";
        assert_eq!(
            parse_networksetup_proxy(on),
            (true, "127.0.0.1".into(), "7890".into())
        );
        let off = "Enabled: No\nServer: \nPort: 0\nAuthenticated Proxy Enabled: 0\n";
        assert_eq!(
            parse_networksetup_proxy(off),
            (false, String::new(), "0".into())
        );
        // 别的字段 / 空行不该干扰。
        assert_eq!(
            parse_networksetup_proxy("garbage\nEnabled: YES\n"),
            (true, String::new(), String::new())
        );
    }

    #[test]
    fn messages_name_the_result() {
        assert!(enabled_message("127.0.0.1:7890", 0).contains("127.0.0.1:7890"));
        let with_bypass = enabled_message("127.0.0.1:7890", 3);
        assert!(with_bypass.contains("127.0.0.1:7890"), "{with_bypass}");
        assert!(with_bypass.contains('3'), "{with_bypass}");
        assert!(disabled_message().contains("已关闭"));
    }

    /// macOS 的排除项参数串：一次调用带全部域名，空白项已在解析阶段丢掉。
    #[test]
    fn bypass_args_carry_every_domain() {
        let args = bypass_args("Wi-Fi", &["localhost".into(), "*.local".into()]);
        assert_eq!(
            args,
            vec![
                "-setproxybypassdomains".to_string(),
                "Wi-Fi".to_string(),
                "localhost".to_string(),
                "*.local".to_string()
            ]
        );
    }

    /// GNOME 的 `ignore-hosts` 是 GVariant 字符串数组，单引号 / 反斜杠必须转义。
    #[test]
    fn ignore_hosts_is_a_gvariant_array() {
        assert_eq!(
            ignore_hosts_gvariant(&["localhost".into(), "192.168.*".into()]),
            "['localhost', '192.168.*']"
        );
        assert_eq!(ignore_hosts_gvariant(&["it's".into()]), "['it\\'s']");
    }
}
