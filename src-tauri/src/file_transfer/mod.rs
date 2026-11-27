//! Files offered to streaming clients through the clipboard (kind 4).
//!
//! Copying files in Explorer (or "send to Sunshine client" on a file) hands
//! their paths to Sunshine (`POST /api/v1/file-transfer/offers`). Sunshine
//! lists them as the logged-on user and returns the offer: an id and the
//! relative paths and sizes. The offer goes to the clients as a clipboard
//! frame; a client that supports it puts the files on its own clipboard and
//! fetches each one from Sunshine when the user pastes. Nothing is copied
//! ahead of a paste.
//!
//! Files copied on a client and pasted here are in `inbound` (with `inbound_win`, the
//! Windows clipboard side).

pub mod inbound;
#[cfg(target_os = "windows")]
pub mod inbound_win;

use log::{debug, info, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::clipboard;
use crate::sunshine::{create_https_client, get_active_sessions, get_sunshine_url};

/// Offer copied files to clients. Mirrors `DesktopSettings::clipboard_files_enabled`.
static ENABLED: AtomicBool = AtomicBool::new(true);

/// Explorer can raise more than one clipboard change for one copy.
const REPEAT_WINDOW: Duration = Duration::from_secs(2);
static LAST_OFFERED: Mutex<Option<(Vec<String>, Instant)>> = Mutex::new(None);

pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Release);
}

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Acquire)
}

pub fn parse_send_to_client_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut take_rest = false;

    for arg in args {
        if take_rest {
            if !arg.trim().is_empty() {
                out.push(arg.clone());
            }
            continue;
        }

        if arg == "--send-to-client" {
            take_rest = true;
            continue;
        }

        if let Some(path) = arg.strip_prefix("--send-to-client=") {
            if !path.trim().is_empty() {
                out.push(path.to_string());
            }
        }
    }

    out
}

const NO_RUNNING_SESSION: &str = "没有正在串流的客户端连接";

pub fn dispatch_cli_send(paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }

    tauri::async_runtime::spawn(async move {
        match offer_paths(paths, "send").await {
            Ok(msg) => info!("file transfer: {msg}"),
            Err(e) => warn!("file transfer failed: {e}"),
        }
    });
}

/// The local clipboard now holds these files (an Explorer copy). Called from
/// the clipboard watcher; returns at once and offers them in the background.
pub fn offer_copied_files(paths: Vec<String>) {
    if !is_enabled() || paths.is_empty() {
        return;
    }
    {
        let mut last = LAST_OFFERED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((previous, at)) = last.as_ref() {
            if *previous == paths && at.elapsed() < REPEAT_WINDOW {
                return;
            }
        }
        *last = Some((paths.clone(), Instant::now()));
    }

    tauri::async_runtime::spawn(async move {
        match offer_paths(paths, "clipboard").await {
            Ok(msg) => info!("file transfer: {msg}"),
            // Copying files with no stream running is the normal case.
            Err(e) if e == NO_RUNNING_SESSION => {
                debug!("file transfer: copied files not offered: {e}")
            }
            Err(e) => warn!("file transfer: copied files not offered: {e}"),
        }
    });
}

#[tauri::command]
pub async fn send_file_to_client(path: String) -> Result<String, String> {
    offer_paths(vec![path], "send").await
}

async fn offer_paths(paths: Vec<String>, offer_type: &str) -> Result<String, String> {
    if !is_enabled() {
        return Err("复制文件到客户端已关闭".to_string());
    }

    let active_sessions = get_active_sessions()
        .await
        .map_err(|e| format!("无法查询客户端连接状态: {e}"))?;
    let running_sessions = active_sessions
        .iter()
        .filter(|s| s.state.eq_ignore_ascii_case("RUNNING"))
        .count();
    if running_sessions == 0 {
        return Err(NO_RUNNING_SESSION.to_string());
    }

    let offer = register_offer(&paths, offer_type).await?;
    let entries = offer
        .get("files")
        .and_then(|f| f.as_array())
        .map(|f| f.len())
        .unwrap_or(0);
    let total = offer.get("total_size").and_then(|t| t.as_u64()).unwrap_or(0);
    let payload = serde_json::to_vec(&offer).map_err(|e| format!("编码文件 offer 失败: {e}"))?;
    clipboard::post_file_offer_payload(payload)
        .await
        .map_err(|e| format!("发送文件 offer 失败: {e}"))?;

    Ok(format!(
        "已发送文件 offer: {} 项 ({} bytes, {} 个客户端)",
        entries, total, running_sessions
    ))
}

/// Register the paths with Sunshine; returns its offer JSON unchanged, which
/// is what the clients receive.
async fn register_offer(paths: &[String], offer_type: &str) -> Result<serde_json::Value, String> {
    let url = get_sunshine_url().await?;
    let client = create_https_client()?;
    let endpoint = format!("{}/api/v1/file-transfer/offers", url.trim_end_matches('/'));

    let body = serde_json::json!({
        "paths": paths,
        "type": offer_type,
    });

    let resp = client
        .post(endpoint)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("注册文件 offer 失败: HTTP {status} {text}"));
    }

    let offer: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("解析文件 offer 响应失败: {e}; body={text}"))?;
    if offer.get("id").and_then(|v| v.as_str()).is_none()
        || offer.get("files").and_then(|v| v.as_array()).is_none()
    {
        return Err(format!("文件 offer 响应缺少 id 或 files: {text}"));
    }
    Ok(offer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_to_client_args_take_everything_after_the_flag() {
        let args: Vec<String> = ["gui.exe", "--send-to-client", "C:\\a b.txt", "D:\\c"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(parse_send_to_client_args(&args), vec!["C:\\a b.txt", "D:\\c"]);

        let eq: Vec<String> = ["gui.exe", "--send-to-client=C:\\x"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(parse_send_to_client_args(&eq), vec!["C:\\x"]);
    }

    #[test]
    fn disabled_offers_nothing() {
        set_enabled(false);
        offer_copied_files(vec!["C:\\a.txt".to_string()]);
        assert!(LAST_OFFERED.lock().unwrap().is_none());
        set_enabled(true);
    }
}
