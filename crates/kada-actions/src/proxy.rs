//! 系统代理开关（`SystemOperation::SwitchProxy`）。
//!
//! 三平台各走各的系统接口：
//!
//! - **Windows**：读写当前用户注册表 `Internet Settings` 的 `ProxyEnable` / `ProxyServer`，
//!   再广播 `InternetSetOption` 通知 WinINET 立即生效。
//! - **macOS**：调 `networksetup`，对「已启用的网络服务」逐个开关（HTTP / HTTPS / SOCKS 三类）。
//!   修改系统网络设置需要管理员权限；失败时把系统给的原文带回去，不静默。
//! - **Linux**：调 GNOME 的 `gsettings`（`org.gnome.system.proxy`）。其它桌面环境没有
//!   统一接口，gsettings 不可用时明确报错。
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
pub(super) fn switch(mode: ProxyMode, server: Option<&ProxyServer>) -> Result<String, String> {
    #[cfg(windows)]
    {
        windows::switch(mode, server)
    }
    #[cfg(target_os = "macos")]
    {
        macos::switch(mode, server)
    }
    #[cfg(target_os = "linux")]
    {
        linux::switch(mode, server)
    }
}

/// 结果说明：开启时带上地址，关闭时只报状态。
fn enabled_message(server: &str) -> String {
    format!("系统代理已开启（{server}）")
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

    pub(super) fn switch(mode: ProxyMode, server: Option<&ProxyServer>) -> Result<String, String> {
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
        }

        key.set_u32("ProxyEnable", u32::from(want))
            .map_err(|e| format!("写入系统代理开关失败: {e}"))?;
        refresh();

        Ok(if want {
            enabled_message(shown.as_deref().unwrap_or("沿用系统里已有的地址"))
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
        use super::registry_value;
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

#[cfg(target_os = "macos")]
mod macos {
    use super::{desired, disabled_message, enabled_message, parse_networksetup_proxy};
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

    pub(super) fn switch(mode: ProxyMode, server: Option<&ProxyServer>) -> Result<String, String> {
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
                }
                return Ok(enabled_message(&server.display()));
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
            return Ok(enabled_message("沿用系统里已有的地址"));
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
    use super::{desired, disabled_message, enabled_message};
    use kada_core::{ProxyMode, ProxyScheme, ProxyServer};
    use std::process::Command;

    /// GNOME 的系统代理 schema（Cinnamon / MATE 等沿用 GNOME 接口的桌面也能读）。
    const SCHEMA: &str = "org.gnome.system.proxy";

    pub(super) fn switch(mode: ProxyMode, server: Option<&ProxyServer>) -> Result<String, String> {
        let current = !matches!(get("mode")?.trim_matches('\''), "none");
        let want = desired(mode, current);

        if !want {
            set("mode", "none")?;
            return Ok(disabled_message());
        }

        match server {
            Some(server) => {
                apply_server(server)?;
                set("mode", "manual")?;
                Ok(enabled_message(&server.display()))
            }
            None => {
                if !has_configured_server()? {
                    return Err(
                        "系统里还没有代理地址：请在动作里填「代理地址」后再开启（关闭不需要地址）"
                            .into(),
                    );
                }
                set("mode", "manual")?;
                Ok(enabled_message("沿用系统里已有的地址"))
            }
        }
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
        assert!(enabled_message("127.0.0.1:7890").contains("127.0.0.1:7890"));
        assert!(disabled_message().contains("已关闭"));
    }
}
