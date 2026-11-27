//! Files copied on a streaming client, pasted on the host.
//!
//! The client sends the list of the files it copied as a clipboard frame
//! (kind 4); Sunshine hands it to this agent with the id of the streaming
//! session it came from. On Windows the agent puts the files on the host
//! clipboard as virtual files (see `inbound_win`); nothing is
//! transferred until something pastes them. A paste reads each file in
//! chunks: `GET /api/v1/file-transfer/inbound/<sid>/<id>/<index>?offset=&length=`
//! makes Sunshine ask the client for that chunk over the stream and answers
//! with the bytes once the client has sent them.

use log::{info, warn};
use std::collections::HashSet;

/// Bounds shared with Sunshine (src/file_transfer.h) and the clients.
pub const MAX_ENTRIES: usize = 10000;
/// Longest relative path in UTF-16 units: FILEDESCRIPTORW holds MAX_PATH.
pub const MAX_PATH_UNITS: usize = 259;
/// Size of one chunk request (at most Sunshine's file_transfer_pull::kMaxChunk,
/// 4 MiB). Sunshine gives the client 30 s plus the chunk at 64 KiB/s to post
/// it, 46 s for 1 MiB; a slow uplink then still finishes a chunk well inside
/// that (8.4 s at 1 Mbit/s), and less is lost when one has to be asked again.
pub const CHUNK: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Relative, '\\'-separated, UTF-16 (FILEDESCRIPTORW's cFileName).
    pub path: Vec<u16>,
    pub size: u64,
    pub dir: bool,
    /// Seconds since the Unix epoch, 0 when unknown.
    pub mtime: i64,
}

#[derive(Clone, Debug)]
pub struct ClientOffer {
    pub sid: u64,
    pub id: String,
    pub entries: Vec<Entry>,
    pub total_size: u64,
}

/// Device names Windows reserves whatever the extension, COM and LPT with a superscript digit included.
const RESERVED: [&str; 28] = [
    "CON",
    "PRN",
    "AUX",
    "NUL",
    "COM1",
    "COM2",
    "COM3",
    "COM4",
    "COM5",
    "COM6",
    "COM7",
    "COM8",
    "COM9",
    "LPT1",
    "LPT2",
    "LPT3",
    "LPT4",
    "LPT5",
    "LPT6",
    "LPT7",
    "LPT8",
    "LPT9",
    "COM\u{b9}",
    "COM\u{b2}",
    "COM\u{b3}",
    "LPT\u{b9}",
    "LPT\u{b2}",
    "LPT\u{b3}",
];

fn valid_component(name: &str) -> bool {
    if name.is_empty() || name == "." || name == ".." {
        return false;
    }
    if name
        .chars()
        .any(|c| (c as u32) < 0x20 || "<>:\"/\\|?*".contains(c))
    {
        return false;
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return false;
    }
    let stem = name.split('.').next().unwrap_or("").trim_end();
    !RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem))
}

/// The id shape Sunshine and the clients use: 64 lower-case hex digits.
fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn integer(value: &serde_json::Value) -> Option<u64> {
    if let Some(n) = value.as_u64() {
        return Some(n);
    }
    // JSON writers that only have doubles send 5.0 for 5.
    let f = value.as_f64()?;
    (f >= 0.0 && f.fract() == 0.0 && f <= 9_007_199_254_740_992.0).then_some(f as u64)
}

/// Validates everything the host's paste target will trust: the id, sizes and
/// every path component (no traversal, nothing Windows rejects or reserves,
/// every entry's folder listed before it, no duplicates ignoring case).
pub fn parse_offer(json: &[u8], sid: u64) -> Result<ClientOffer, String> {
    let value: serde_json::Value = serde_json::from_slice(json).map_err(|_| "not JSON")?;
    let obj = value.as_object().ok_or("not a JSON object")?;
    let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("");
    if !valid_id(id) {
        return Err("bad id".into());
    }
    let files = obj
        .get("files")
        .and_then(|v| v.as_array())
        .ok_or("no files")?;
    if files.is_empty() || files.len() > MAX_ENTRIES {
        return Err("bad number of files".into());
    }

    let mut entries = Vec::with_capacity(files.len());
    let mut seen: HashSet<String> = HashSet::new();
    let mut folders: HashSet<String> = HashSet::new();
    let mut total: u64 = 0;
    for file in files {
        let path = file
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or("bad path")?;
        let components: Vec<&str> = path.split('/').collect();
        if !components.iter().all(|c| valid_component(c)) {
            return Err(format!("bad path: {path}"));
        }
        let windows_path = components.join("\\");
        let units: Vec<u16> = windows_path.encode_utf16().collect();
        if units.len() > MAX_PATH_UNITS {
            return Err(format!("path too long: {path}"));
        }
        let key = windows_path.to_lowercase();
        if let Some(parent) = key.rfind('\\').map(|i| &key[..i]) {
            if !folders.contains(parent) {
                return Err(format!("folder not listed first: {path}"));
            }
        }
        if !seen.insert(key.clone()) {
            return Err(format!("duplicate: {path}"));
        }
        let dir = file.get("dir").and_then(|v| v.as_bool()).unwrap_or(false);
        let size = file.get("size").and_then(integer).ok_or("bad size")?;
        if dir && size != 0 {
            return Err("folder with a size".into());
        }
        let mtime = file
            .get("mtime")
            .and_then(|v| v.as_f64())
            .filter(|t| t.is_finite() && *t >= 0.0 && *t < 1e13)
            .map(|t| t as i64)
            .unwrap_or(0);
        if dir {
            folders.insert(key);
        }
        total = total.checked_add(size).ok_or("total too large")?;
        entries.push(Entry {
            path: units,
            size,
            dir,
            mtime,
        });
    }
    Ok(ClientOffer {
        sid,
        id: id.to_string(),
        entries,
        total_size: total,
    })
}

/// One chunk of entry `index`, from `offset`, at most `length` bytes; fewer
/// only at the end of the file.
/// Why a chunk could not be had: the paste target shows a message to match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The client's file is no longer the one it copied (changed or replaced).
    Changed,
    /// The client's file, or its offer, is gone.
    Gone,
    /// The client did not send the chunk in time.
    Timeout,
    /// The stream or the connection to Sunshine went away.
    Network,
    Other,
}

#[derive(Clone, Debug)]
pub struct ChunkError {
    pub failure: Failure,
    pub message: String,
}

impl ChunkError {
    pub(crate) fn new(failure: Failure, message: impl Into<String>) -> Self {
        Self {
            failure,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ChunkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({:?})", self.message, self.failure)
    }
}

/// What Sunshine's answer to a chunk request means. Sunshine passes the
/// client's own word on with 502 (`changed`, `gone`, `not_found`, `range`),
/// answers 504 `timeout` when the client did not post in time and 410
/// `no_session` once the stream has ended.
pub fn classify(status: u16, body: &[u8]) -> Failure {
    let word = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_owned))
        .unwrap_or_default();
    match (status, word.as_str()) {
        (_, "changed") | (_, "range") => Failure::Changed,
        (_, "gone") | (502, "not_found") => Failure::Gone,
        (504, _) | (_, "timeout") => Failure::Timeout,
        (410, _) | (_, "no_session") => Failure::Network,
        _ => Failure::Other,
    }
}

pub async fn fetch_chunk(
    client: &reqwest::Client,
    offer: &ClientOffer,
    index: usize,
    offset: u64,
    length: u64,
) -> Result<Vec<u8>, ChunkError> {
    let url = crate::sunshine::get_sunshine_url()
        .await
        .map_err(|e| ChunkError::new(Failure::Network, e))?;
    let endpoint = format!(
        "{}/api/v1/file-transfer/inbound/{}/{}/{}?offset={}&length={}",
        url.trim_end_matches('/'),
        offer.sid,
        offer.id,
        index,
        offset,
        length
    );
    let transport = |e: reqwest::Error| {
        let failure = if e.is_timeout() {
            Failure::Timeout
        } else {
            Failure::Network
        };
        ChunkError::new(failure, e.to_string())
    };
    let resp = client.get(endpoint).send().await.map_err(transport)?;
    let status = resp.status();
    let body = resp.bytes().await.map_err(transport)?;
    if !status.is_success() {
        return Err(ChunkError::new(
            classify(status.as_u16(), &body),
            format!(
                "HTTP {status} {}",
                String::from_utf8_lossy(&body[..body.len().min(200)])
            ),
        ));
    }
    if body.len() as u64 > length {
        return Err(ChunkError::new(Failure::Other, "chunk longer than asked"));
    }
    Ok(body.to_vec())
}

/// A client's file offer arrived (inline or through a blob).
pub fn apply_client_offer(sid: Option<u64>, payload: &[u8]) {
    if !crate::file_transfer::is_enabled() {
        info!("client file offer ignored: copying files with clients is off");
        return;
    }
    let Some(sid) = sid else {
        warn!("client file offer without a session id; dropped");
        return;
    };
    let offer = match parse_offer(payload, sid) {
        Ok(offer) => offer,
        Err(e) => {
            warn!("client file offer dropped: {e}");
            return;
        }
    };
    let count = offer.entries.len();
    let total = offer.total_size;
    #[cfg(target_os = "windows")]
    {
        match super::inbound_win::set(offer) {
            Ok(()) => info!("client files on the host clipboard ({count} entries, {total} bytes)"),
            Err(e) => warn!("could not put the client's files on the clipboard: {e}"),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = offer;
        info!("client offered {count} files ({total} bytes); pasting them is supported on Windows only");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0b5e3a8c1d2f4a6b8c9d0e1f2a3b4c5d0b5e3a8c1d2f4a6b8c9d0e1f2a3b4c5d";

    #[test]
    fn sunshines_answers_name_the_failure() {
        let word = |w: &str| format!(r#"{{"error":"{w}"}}"#).into_bytes();
        assert_eq!(classify(502, &word("changed")), Failure::Changed);
        assert_eq!(classify(502, &word("range")), Failure::Changed);
        assert_eq!(classify(502, &word("gone")), Failure::Gone);
        assert_eq!(classify(502, &word("not_found")), Failure::Gone);
        assert_eq!(classify(504, &word("timeout")), Failure::Timeout);
        assert_eq!(classify(410, &word("no_session")), Failure::Network);
        assert_eq!(classify(503, &word("busy")), Failure::Other);
        assert_eq!(classify(500, b"not json"), Failure::Other);
        // Sunshine's own 404 is a bad request from us, not a missing file.
        assert_eq!(classify(404, &word("not_found")), Failure::Other);
    }

    #[test]
    fn a_chunk_fits_the_ticket_and_the_read_timeout() {
        // Sunshine: at most 4 MiB, a ticket lasts 30 s + the chunk at 64 KiB/s.
        assert!(CHUNK <= 4 * 1024 * 1024);
        let ticket_secs = 30 + CHUNK.div_ceil(64 * 1024);
        // The transfer client waits this long for the answer to start.
        let read_secs = crate::sunshine::TRANSFER_READ_TIMEOUT.as_secs();
        assert!(
            ticket_secs + 10 <= read_secs,
            "ticket {ticket_secs} s, read {read_secs} s"
        );
        // 1 Mbit/s up (125000 bytes/s) finishes a chunk well before the ticket ends.
        assert!(CHUNK / 125_000 + 1 < ticket_secs - 30);
    }

    fn offer(files: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"id": ID, "type": "clipboard", "files": files}))
            .unwrap()
    }

    fn file(path: &str, size: f64) -> serde_json::Value {
        serde_json::json!({"path": path, "size": size, "dir": false, "mtime": 1700000000})
    }

    fn dir(path: &str) -> serde_json::Value {
        serde_json::json!({"path": path, "size": 0, "dir": true, "mtime": 0})
    }

    fn parses(files: serde_json::Value) -> bool {
        parse_offer(&offer(files), 7).is_ok()
    }

    #[test]
    fn a_valid_offer_parses_with_windows_separators() {
        let parsed = parse_offer(
            &offer(serde_json::json!([
                file("a.txt", 5.0),
                dir("d"),
                dir("d/s"),
                file("d/s/b.bin", 1000.0)
            ])),
            7,
        )
        .unwrap();
        assert_eq!(parsed.sid, 7);
        assert_eq!(parsed.entries.len(), 4);
        assert_eq!(parsed.total_size, 1005);
        assert_eq!(
            String::from_utf16(&parsed.entries[3].path).unwrap(),
            "d\\s\\b.bin"
        );
        assert!(parsed.entries[1].dir);
        assert_eq!(parsed.entries[0].mtime, 1700000000);
    }

    #[test]
    fn unsafe_or_inconsistent_offers_are_refused() {
        assert!(!parses(serde_json::json!([file("../a", 1.0)])));
        assert!(!parses(serde_json::json!([
            dir("d"),
            file("d/../../a", 1.0)
        ])));
        assert!(!parses(serde_json::json!([file("a\\b", 1.0)])));
        assert!(!parses(serde_json::json!([file("C:/a", 1.0)])));
        assert!(!parses(serde_json::json!([file("/a", 1.0)])));
        assert!(!parses(serde_json::json!([file("CON.txt", 1.0)])));
        assert!(!parses(serde_json::json!([file("lpt1", 1.0)])));
        assert!(!parses(serde_json::json!([file("COM\u{b9}.txt", 1.0)])));
        assert!(!parses(serde_json::json!([file("lpt\u{b3}", 1.0)])));
        assert!(!parses(serde_json::json!([file("a.", 1.0)])));
        assert!(!parses(serde_json::json!([file("a ", 1.0)])));
        assert!(!parses(serde_json::json!([file("d/a", 1.0)])));
        assert!(!parses(serde_json::json!([
            file("A.txt", 1.0),
            file("a.TXT", 1.0)
        ])));
        assert!(!parses(serde_json::json!([file("a", -1.0)])));
        assert!(!parses(serde_json::json!([file("a", 1.5)])));
        assert!(!parses(
            serde_json::json!([{"path": "d", "size": 3, "dir": true}])
        ));
        assert!(!parses(serde_json::json!([file(&"x".repeat(260), 1.0)])));
        assert!(parses(serde_json::json!([file(&"x".repeat(259), 1.0)])));
        assert!(!parses(serde_json::json!([])));
        assert!(parse_offer(b"[1]", 7).is_err());
        let bad_id =
            serde_json::to_vec(&serde_json::json!({"id": "../x", "files": [file("a", 1.0)]}))
                .unwrap();
        assert!(parse_offer(&bad_id, 7).is_err());
        let uuid = serde_json::to_vec(
            &serde_json::json!({"id": "0b5e3a8c-1d2f-4a6b-8c9d-0e1f2a3b4c5d", "files": [file("a", 1.0)]}),
        )
        .unwrap();
        assert!(parse_offer(&uuid, 7).is_err(), "a UUID is not an offer id");
    }
}
