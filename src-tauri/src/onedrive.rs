//! OneDrive 同步（Microsoft Graph，**仅应用私有文件夹** `special/approot`）：OAuth2 授权码 +
//! PKCE——走系统浏览器登录、本机回环端口接回调，令牌存本机文件（**不进 config.json**，也不
//! 参与导出 / 同步存档）。规划 7.2-㉝ 的「云 API 通道」。
//!
//! 权限只申请最小的 `Files.ReadWrite.AppFolder`（配置存在 `Apps/Kada/` 私有目录，看不到用户
//! 其他文件）+ `offline_access`（换刷新令牌）+ `openid profile email`（拿账号名展示）。网络
//! 走 reqwest（复用进程 TLS provider，见 [`crate::ensure_tls_provider`]），PKCE 的 SHA-256 与
//! 随机数走 ring，浏览器由 [`kada_actions::open_url`] 打开。
//!
//! 与 Tauri 解耦：纯逻辑（base64url、PKCE、授权 URL、回调解析、JWT 取邮箱）是可单测的独立函数；
//! HTTP / 令牌 / Graph 读写是 async 函数，由壳层命令 `await`。**阻塞**的只有「等浏览器回调」，
//! 由壳层丢到 `spawn_blocking`（不占 async 执行器）。

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use ring::rand::SecureRandom;

/// 授权 / 令牌端点（v2.0）。租户用 `common`：同时支持个人与工作 / 学校账号——**Azure 应用注册的
/// 「支持的账户类型」必须包含它们**（选「任何组织目录中的账户和个人 Microsoft 账户」），否则登录报错。
const AUTHORITY: &str = "https://login.microsoftonline.com";
const TENANT: &str = "common";
const GRAPH: &str = "https://graph.microsoft.com/v1.0";
/// 申请的权限：仅应用私有文件夹 + 刷新令牌 + 账号展示（最小权限）。
const SCOPES: &str = "openid profile email offline_access Files.ReadWrite.AppFolder";

/// 内部 OneDrive 应用（客户端）ID：由项目维护者提供、随仓库公开——**不是机密**，公共客户端的
/// client ID 本就会出现在授权链接里。留空 = 未配置，登录入口会提示「未配置应用 ID」。
pub const CLIENT_ID: &str = "";

/// 云端文件名（写在应用私有文件夹 `Apps/Kada/` 里，各端固定同名）。
pub const FILE_NAME: &str = "kada-config.json";

/// 等浏览器回调的最长时间（用户可能慢慢登录；超时后放弃，不影响应用）。
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

/// 令牌到期前多少秒开始视为「该刷新了」。
const REFRESH_MARGIN_SECS: i64 = 60;

/// 本机保存的登录态（`onedrive-auth.json` 的内容）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Auth {
    pub access_token: String,
    /// 刷新令牌（`offline_access` 才有）。旧文件可能没有。
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// `access_token` 的到期时刻（unix 秒）。
    #[serde(default)]
    pub expires_at: i64,
    /// 展示用账号名（邮箱 / 用户名）。
    #[serde(default)]
    pub account: String,
}

impl Auth {
    /// 是否该刷新（无 access_token 或已过期 / 即将过期）。
    fn needs_refresh(&self) -> bool {
        self.access_token.is_empty() || self.expires_at - now_secs() <= REFRESH_MARGIN_SECS
    }
}

// ---- 纯逻辑（可单测） ----

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// 无填充 base64url 编码（JWT / PKCE 用；标准 base64 换成 URL 安全字母表并去掉 `=`）。
pub fn base64url_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | b2 as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(B64[(n >> 6) as usize & 63] as char);
        }
        if chunk.len() > 2 {
            out.push(B64[n as usize & 63] as char);
        }
    }
    out
}

/// 无填充 base64url 解码（容忍 `=` 填充与缺失；非法字符返回 `None`）。
pub fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut vals: Vec<u8> = Vec::with_capacity(s.len());
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            b'=' => continue,
            _ => return None,
        };
        vals.push(v);
    }
    let mut out = Vec::with_capacity(vals.len() / 4 * 3 + 2);
    for chunk in vals.chunks(4) {
        if chunk.len() == 1 {
            return None;
        }
        let n = ((chunk[0] as u32) << 18)
            | ((chunk[1] as u32) << 12)
            | ((*chunk.get(2).unwrap_or(&0) as u32) << 6)
            | (*chunk.get(3).unwrap_or(&0) as u32);
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

/// 查询参数用的百分号编码（RFC 3986 非保留字符集原样，其余 `%XX`）。
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 百分号解码（`+` 当空格，容忍非法序列时原样保留）。
fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// PKCE `code_challenge` = base64url(SHA-256(verifier))（`S256`，微软要求 S256，不接受 plain）。
pub fn pkce_challenge(verifier: &str) -> String {
    base64url_encode(ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes()).as_ref())
}

/// URL 安全随机串（PKCE verifier / state）：`n` 个随机字节的 base64url。
fn random_urlsafe(n: usize) -> String {
    let mut buf = vec![0u8; n];
    ring::rand::SystemRandom::new()
        .fill(&mut buf)
        .expect("系统随机源不可用");
    base64url_encode(&buf)
}

/// 拼授权 URL（`response_mode=query`：回调参数在查询串里）。
pub fn auth_url(client_id: &str, redirect_uri: &str, state: &str, challenge: &str) -> String {
    format!(
        "{AUTHORITY}/{TENANT}/oauth2/v2.0/authorize?client_id={}&response_type=code&response_mode=query\
         &redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
        urlencode(client_id),
        urlencode(redirect_uri),
        urlencode(SCOPES),
        urlencode(state),
        urlencode(challenge),
    )
}

/// 从回环回调的请求行（`GET /?code=..&state=.. HTTP/1.1`）解析出 `(code, state)`；`error=`
/// 一律当失败返回（附 `error_description`，若有）。
pub fn parse_callback(request_line: &str) -> Result<(String, String), String> {
    let target = request_line.split_whitespace().nth(1).unwrap_or("");
    let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
    let (mut code, mut state, mut err, mut desc) = (None, None, None, None);
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = urldecode(v);
        match k {
            "code" => code = Some(v),
            "state" => state = Some(v),
            "error" => err = Some(v),
            "error_description" => desc = Some(v),
            _ => {}
        }
    }
    if err.is_some() || desc.is_some() {
        // 两者都有时都带上（错误码便于定位，描述便于用户看懂）。
        let parts: Vec<String> = [err, desc].into_iter().flatten().collect();
        return Err(format!("授权被拒绝：{}", parts.join("；")));
    }
    match (code, state) {
        (Some(c), Some(s)) if !c.is_empty() => Ok((c, s)),
        _ => Err("回调里没拿到授权码".into()),
    }
}

/// 从 id_token（JWT）载荷里取展示用账号名（`preferred_username` / `email` / `upn`）。
pub fn jwt_account(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let bytes = base64url_decode(payload)?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    for k in ["preferred_username", "email", "upn"] {
        if let Some(s) = v.get(k).and_then(|x| x.as_str()) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

// ---- 本机令牌文件 ----

/// 读取本机登录态；文件不存在 / 解析失败返回 `None`（视作未登录）。
pub fn load(path: &Path) -> Option<Auth> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// 保存登录态（令牌文件；unix 下尽力设为 0600，别让同机其他用户随便读到）。
pub fn save(path: &Path, auth: &Auth) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text = serde_json::to_string_pretty(auth).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("保存登录信息失败：{e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// 删除登录态（退出登录）；文件本就不在也算成功。
pub fn clear(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("退出登录失败：{e}")),
    }
}

// ---- 网络流程 ----

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn client() -> Result<reqwest::Client, String> {
    crate::ensure_tls_provider();
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("kada/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("网络初始化失败：{e}"))
}

/// 从错误响应体里抠一句人话（Graph / AAD 都把原因放在 `error_description` / `error` / `message`）。
fn error_body(text: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        for k in ["error_description", "error", "message"] {
            if let Some(s) = v.get(k).and_then(|x| x.as_str()) {
                if !s.is_empty() {
                    return s.to_string();
                }
            }
        }
    }
    text.chars().take(300).collect()
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: i64,
    #[serde(default)]
    id_token: Option<String>,
}

/// 把表单参数编成 `application/x-www-form-urlencoded` 请求体（不用 reqwest 的 `.form`：
/// 精简构建下它不可用，自己编码也就几行，且 `urlencode` 可复用 / 可单测）。
fn form_body(params: &[(&str, &str)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

async fn post_token(
    c: &reqwest::Client,
    params: &[(&str, &str)],
) -> Result<TokenResponse, String> {
    let resp = c
        .post(format!("{AUTHORITY}/{TENANT}/oauth2/v2.0/token"))
        .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(form_body(params))
        .send()
        .await
        .map_err(|e| format!("令牌请求失败：{e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("令牌请求失败（HTTP {status}）：{}", error_body(&text)));
    }
    serde_json::from_str::<TokenResponse>(&text).map_err(|e| format!("令牌响应解析失败：{e}"))
}

fn auth_from_token(tr: TokenResponse) -> Auth {
    Auth {
        account: tr.id_token.as_deref().and_then(jwt_account).unwrap_or_default(),
        access_token: tr.access_token,
        refresh_token: tr.refresh_token,
        expires_at: now_secs() + tr.expires_in,
    }
}

/// 刷新令牌并就地更新 `auth`（保留原账号名 / 旧刷新令牌，若响应里没带）。
async fn refresh_into(client_id: &str, auth: &mut Auth) -> Result<(), String> {
    let rt = auth.refresh_token.clone().ok_or("登录已失效，请重新登录 OneDrive")?;
    let c = client()?;
    let tr = post_token(
        &c,
        &[
            ("client_id", client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", &rt),
            ("scope", SCOPES),
        ],
    )
    .await?;
    let account = tr.id_token.as_deref().and_then(jwt_account).filter(|a| !a.is_empty());
    auth.access_token = tr.access_token;
    if tr.refresh_token.is_some() {
        auth.refresh_token = tr.refresh_token;
    }
    auth.expires_at = now_secs() + tr.expires_in;
    if let Some(a) = account {
        auth.account = a;
    }
    Ok(())
}

/// 保证 `auth.access_token` 可用（过期就刷新）。
pub async fn ensure_token(client_id: &str, auth: &mut Auth) -> Result<(), String> {
    if auth.needs_refresh() {
        refresh_into(client_id, auth).await?;
    }
    Ok(())
}

/// 完整登录：起回环端口 → 开浏览器 → 等回调 → 换令牌。返回可用 [`Auth`]（调用方负责落盘）。
pub async fn authorize(client_id: &str) -> Result<Auth, String> {
    if client_id.is_empty() {
        return Err("尚未配置 OneDrive 应用 ID（client ID）".into());
    }
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("无法开本地回调端口：{e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect_uri = format!("http://localhost:{port}");
    let verifier = random_urlsafe(32);
    let challenge = pkce_challenge(&verifier);
    let state = random_urlsafe(16);
    let url = auth_url(client_id, &redirect_uri, &state, &challenge);

    kada_actions::open_url(&url)?;

    // 等回调是阻塞的（非阻塞轮询 + 超时）：丢到线程池，别占 async 执行器。
    let expected = state.clone();
    let code = tauri::async_runtime::spawn_blocking(move || wait_for_code(listener, &expected))
        .await
        .map_err(|e| format!("回调等待线程失败：{e}"))??;

    let c = client()?;
    let tr = post_token(
        &c,
        &[
            ("client_id", client_id),
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &redirect_uri),
            ("code_verifier", &verifier),
            ("scope", SCOPES),
        ],
    )
    .await?;
    Ok(auth_from_token(tr))
}

/// 阻塞等浏览器回调（首条连接即回调）：解析并校验 `state`，回一个小页面再返回授权码。
fn wait_for_code(listener: TcpListener, expected_state: &str) -> Result<String, String> {
    let _ = listener.set_nonblocking(true);
    let deadline = Instant::now() + CALLBACK_TIMEOUT;
    loop {
        match listener.accept() {
            Ok((mut s, _)) => {
                let mut line = String::new();
                let _ = BufReader::new(&s).read_line(&mut line);
                let (body, result) = match parse_callback(&line) {
                    Ok((code, st)) if st == expected_state => (
                        "<p>咔哒：授权成功，请回到 Kada。</p>",
                        Ok(code),
                    ),
                    Ok(_) => (
                        "<p>咔哒：回调校验失败（state 不匹配），请重试。</p>",
                        Err("回调 state 不匹配（可能被跨站伪造），已忽略".to_string()),
                    ),
                    Err(e) => ("<p>咔哒：授权失败，请回到 Kada 重试。</p>", Err(e)),
                };
                let html = format!(
                    "<!doctype html><meta charset=\"utf-8\"><title>Kada</title>{body}"
                );
                let _ = s.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                        html.len()
                    )
                    .as_bytes(),
                );
                return result;
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err("等待授权超时（5 分钟），请重试".into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(format!("回调端口出错：{e}")),
        }
    }
}

/// 把配置字节写到云端应用文件夹的 [`FILE_NAME`]。
pub async fn upload(client_id: &str, auth: &mut Auth, body: Vec<u8>) -> Result<(), String> {
    ensure_token(client_id, auth).await?;
    let c = client()?;
    let url = format!("{GRAPH}/me/drive/special/approot:/{FILE_NAME}:/content");
    let resp = c
        .put(url)
        .bearer_auth(&auth.access_token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| format!("上传失败：{e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("上传失败（HTTP {status}）：{}", error_body(&text)));
    }
    Ok(())
}

/// 从云端应用文件夹读回 [`FILE_NAME`]；文件不存在时报一句可读的错。
pub async fn download(client_id: &str, auth: &mut Auth) -> Result<Vec<u8>, String> {
    ensure_token(client_id, auth).await?;
    let c = client()?;
    let url = format!("{GRAPH}/me/drive/special/approot:/{FILE_NAME}:/content");
    let resp = c
        .get(url)
        .bearer_auth(&auth.access_token)
        .send()
        .await
        .map_err(|e| format!("下载失败：{e}"))?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err("云端还没有配置（请先在另一台机器「上传到 OneDrive」）".into());
    }
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("下载失败（HTTP {status}）：{}", error_body(&text)));
    }
    let bytes = resp.bytes().await.map_err(|e| format!("下载中断：{e}"))?;
    Ok(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_roundtrips_and_matches_known_values() {
        // "hello" 的 base64url（无填充）= "aGVsbG8"。
        assert_eq!(base64url_encode(b"hello"), "aGVsbG8");
        // 各字节边界都要能原样解回。
        for case in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar", &[0u8, 1, 2, 255, 254, 253][..]] {
            let enc = base64url_encode(case);
            assert!(!enc.contains('='), "无填充：{enc}");
            assert_eq!(base64url_decode(&enc).as_deref(), Some(case), "解回原文：{enc}");
        }
        // 非法字符 → None。
        assert_eq!(base64url_decode("a*b"), None);
    }

    #[test]
    fn pkce_matches_rfc7636_vector() {
        // RFC 7636 附录 B 的官方样例。
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn auth_url_carries_pkce_and_encoded_redirect() {
        let u = auth_url("abc-123", "http://localhost:5000", "st4te", "ch4llenge");
        assert!(u.starts_with("https://login.microsoftonline.com/common/oauth2/v2.0/authorize?"));
        assert!(u.contains("client_id=abc-123"));
        assert!(u.contains("code_challenge_method=S256"));
        assert!(u.contains("code_challenge=ch4llenge"));
        assert!(u.contains("state=st4te"));
        // scope / redirect 里的 `:` `/` 必须被编码（否则 URL 结构会被破坏）。
        assert!(u.contains("redirect_uri=http%3A%2F%2Flocalhost%3A5000"));
        assert!(u.contains("Files.ReadWrite.AppFolder"), "scope 原文可见（`.` 不编码）");
        assert!(!u.contains(" "), "参数间不能有裸空格");
    }

    #[test]
    fn parse_callback_reads_code_and_state_and_reports_errors() {
        let (code, state) =
            parse_callback("GET /?code=THE_CODE&state=THE_STATE HTTP/1.1").unwrap();
        assert_eq!(code, "THE_CODE");
        assert_eq!(state, "THE_STATE");

        // 百分号编码的值要解开。
        let (code, _) = parse_callback("GET /?code=a%2Bb%20c&state=s HTTP/1.1").unwrap();
        assert_eq!(code, "a+b c");

        // 授权被拒：带 error_description。
        let e = parse_callback("GET /?error=access_denied&error_description=user%20said%20no HTTP/1.1")
            .unwrap_err();
        assert!(e.contains("access_denied") && e.contains("user said no"), "{e}");

        // 缺 state / 缺 code 都要报错，不能当成成功。
        assert!(parse_callback("GET /?code=only HTTP/1.1").is_err());
        assert!(parse_callback("GET /?state=only HTTP/1.1").is_err());
    }

    #[test]
    fn jwt_account_extracts_email_claim() {
        let payload = base64url_encode(
            br#"{"preferred_username":"someone@outlook.com","name":"Someone"}"#,
        );
        let token = format!("header.{payload}.signature");
        assert_eq!(jwt_account(&token).as_deref(), Some("someone@outlook.com"));

        // 没有可用 claim / 结构不对 → None（不 panic）。
        let p = base64url_encode(br#"{"sub":"abc"}"#);
        assert_eq!(jwt_account(&format!("h.{p}.s")), None);
        assert_eq!(jwt_account("not-a-jwt"), None);
    }

    #[test]
    fn auth_roundtrips_through_file() {
        let dir = std::env::temp_dir().join(format!("kada_onedrive_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("onedrive-auth.json");

        assert!(load(&path).is_none(), "没文件 = 未登录");
        let auth = Auth {
            access_token: "AT".into(),
            refresh_token: Some("RT".into()),
            expires_at: now_secs() + 3600,
            account: "a@b.com".into(),
        };
        save(&path, &auth).unwrap();
        let back = load(&path).unwrap();
        assert_eq!(back.access_token, "AT");
        assert_eq!(back.refresh_token.as_deref(), Some("RT"));
        assert_eq!(back.account, "a@b.com");

        clear(&path).unwrap();
        assert!(load(&path).is_none());
        clear(&path).unwrap(); // 再删一次不报错

        let _ = std::fs::remove_dir_all(&dir);
    }
}
