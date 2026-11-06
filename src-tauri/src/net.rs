// 网络叶子（T-02 从 lib.rs 搬出）：卡带索引 / 卡带的下载、官方发布 POST 与统一 reqwest 客户端。
// 只收 http(s)、下载后判体积、错误里的内部地址先抹掉（见 说明书 §6.7 / §6.11）。

use std::time::Duration;

/// 卡带下载的时限与体积上限：一份索引 / 一个卡带都是几百 KB 量级，超了当坏文件处理。
const SNIPPET_FETCH_TIMEOUT: Duration = Duration::from_secs(20);
const SNIPPET_MAX_BYTES: usize = 4 * 1024 * 1024;

/// 把文本里的 http(s) URL 抹掉，避免把内置地址（更新清单、卡带索引、发布端点）暴露给界面。
///
/// reqwest 的 `Display` 会带上完整 URL（如 `error sending request for url (https://…)`），
/// 原样抛给用户等于把内部下载地址摊在界面上。这里只处理 http(s)；本地路径、`file://` 等
/// 一律不动——那是用户自己的输入，不是内部地址。
pub(crate) fn strip_urls(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(pos) = rest.find("http://").or_else(|| rest.find("https://")) {
        out.push_str(&rest[..pos]);
        out.push('…');
        // 从 URL 起点吞到下一个空白 / 收尾符（括号、引号、尖括号、反斜杠、逗号、方括号）。
        let tail = &rest[pos..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '"' | '>' | '\\' | ',' | ']'))
            .unwrap_or(tail.len());
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// 下载一段文本（索引 / 卡带本体）。
///
/// 只收 http(s)：设置里的这个字符串不该变成读本地文件的口子（`file://` 之类一律拒）。
/// 体积上限是「下完再判」——几百 KB 的上限下没必要为流式计数多写一层。
pub(crate) async fn http_get_text(url: &str) -> Result<String, String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!("不是 http(s) 地址：{url}"));
    }
    let client = http_client(SNIPPET_FETCH_TIMEOUT)?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| strip_urls(&format!("下载失败：{e}")))?;
    if !resp.status().is_success() {
        return Err(format!("下载失败：HTTP {}", resp.status()));
    }
    let body = resp
        .bytes()
        .await
        .map_err(|e| strip_urls(&format!("下载中断：{e}")))?;
    if body.len() > SNIPPET_MAX_BYTES {
        return Err(format!(
            "内容太大（{} MB），不像是索引或卡带",
            body.len() / 1024 / 1024
        ));
    }
    String::from_utf8(body.to_vec()).map_err(|_| "下载内容不是 UTF-8 文本".to_string())
}

/// 发一段 JSON 到**固定**的官方发布端点。
///
/// 地址不接受前端传入：发布能力只对官方市场开放，避免客户端被诱导把配置 POST 到任意地址。
pub(crate) async fn http_post_json(url: &str, body: &str) -> Result<String, String> {
    let client = http_client(Duration::from_secs(30))?;
    let resp = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| strip_urls(&format!("发布失败：{e}")))?;
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("发布响应中断：{e}"))?;
    if bytes.len() > 1024 * 1024 {
        return Err("发布接口返回内容过大".into());
    }
    let text =
        String::from_utf8(bytes.to_vec()).map_err(|_| "发布接口返回的不是 UTF-8".to_string())?;
    if !status.is_success() {
        let brief: String = text.chars().take(200).collect();
        return Err(format!("发布失败：HTTP {status} {brief}"));
    }
    Ok(text)
}

/// 统一的 reqwest 客户端：进程级 rustls provider 装一次（updater 之外没人装过，
/// 直接建 client 会 panic），再带上超时与 UA。
fn http_client(timeout: Duration) -> Result<reqwest::Client, String> {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(concat!("kada/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("网络初始化失败：{e}"))
}
