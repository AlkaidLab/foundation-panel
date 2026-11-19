//! Answers of the two probes that start PowerShell (the signature of the registered driver and the host helper's
//! `status`), kept so that a status read does not start a process every time.
//!
//! Each answer is stored with a fingerprint of everything it was derived from (the driver file's size and time, the
//! registered installations, the helper script's size and time). A read with the same fingerprint reuses the answer; a
//! different one probes again at once, so an installation changed outside the panel is still noticed. The answer is
//! also written to a small file next to the panel's settings (valid for a day), so the first read after the panel
//! starts does not start PowerShell either. A command that changes the installation calls `forget_all()` through
//! `status::fresh()`, before and after it runs. Failed or unreadable probes are never kept.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::probe_cache::ProbeCache;

/// How long one process keeps an answer before it checks the fingerprint against a new probe.
const MEMO_TTL: Duration = Duration::from_secs(5 * 60);
/// How old a stored answer may be when the panel starts.
const STORED_MAX_AGE_SECS: u64 = 24 * 60 * 60;
const STORE_FILE: &str = "probe-cache.json";

/// Creation flags of a probe that nobody waits for on screen: no window, below normal priority.
#[cfg(target_os = "windows")]
pub(super) const BACKGROUND_PROBE_FLAGS: u32 = windows::Win32::System::Threading::CREATE_NO_WINDOW.0
    | windows::Win32::System::Threading::BELOW_NORMAL_PRIORITY_CLASS.0;

fn store_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|dir| dir.join("Sunshine GUI").join(STORE_FILE))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn read_store() -> serde_json::Map<String, serde_json::Value> {
    let Some(path) = store_path() else {
        return Default::default();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_store(store: &serde_json::Map<String, serde_json::Value>) {
    let Some(path) = store_path() else { return };
    let Ok(text) = serde_json::to_string(store) else {
        return;
    };
    let temporary = path.with_extension("json.tmp");
    // Best effort: a store that cannot be written only means the next start probes once more.
    if std::fs::write(&temporary, text).is_ok() && std::fs::rename(&temporary, &path).is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
}

fn recall<T: DeserializeOwned>(name: &str, fingerprint: &str) -> Option<T> {
    let store = read_store();
    let entry = store.get(name)?;
    let age = now_secs().saturating_sub(entry.get("at")?.as_u64()?);
    if entry.get("fp")?.as_str()? != fingerprint || age > STORED_MAX_AGE_SECS {
        return None;
    }
    serde_json::from_value(entry.get("value")?.clone()).ok()
}

fn keep<T: Serialize>(name: &str, fingerprint: &str, value: &T) {
    let Ok(value) = serde_json::to_value(value) else {
        return;
    };
    let mut store = read_store();
    store.insert(
        name.to_string(),
        serde_json::json!({ "fp": fingerprint, "at": now_secs(), "value": value }),
    );
    write_store(&store);
}

fn forget(name: &str) {
    let mut store = read_store();
    if store.remove(name).is_some() {
        write_store(&store);
    }
}

/// Size and modification time of a file: changes when the file is replaced.
pub(super) fn file_identity(path: &std::path::Path) -> String {
    match std::fs::metadata(path) {
        Ok(meta) => {
            let modified = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |elapsed| elapsed.as_nanos());
            format!("{}:{}:{}", path.display(), meta.len(), modified)
        }
        Err(_) => format!("{}:absent", path.display()),
    }
}

pub(super) struct Memo<T> {
    name: &'static str,
    cache: ProbeCache<(String, T)>,
}

impl<T> Memo<T> {
    pub(super) const fn new(name: &'static str) -> Self {
        Self {
            name,
            cache: ProbeCache::new(name, MEMO_TTL),
        }
    }

    pub(super) fn forget(&self) {
        self.cache.invalidate();
        forget(self.name);
    }
}

impl<T: Clone + Serialize + DeserializeOwned> Memo<T> {
    /// The answer for `fingerprint`: the kept one, the stored one, or what `probe` returns now (`None` when the
    /// probe failed; nothing is kept then).
    pub(super) async fn get<F, Fut>(&self, fingerprint: &str, probe: F) -> Option<T>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Option<T>>,
    {
        for _ in 0..2 {
            let loaded = self
                .cache
                .get(|| async {
                    if let Some(value) = recall::<T>(self.name, fingerprint) {
                        return Ok((fingerprint.to_string(), value));
                    }
                    match probe().await {
                        Some(value) => {
                            keep(self.name, fingerprint, &value);
                            Ok((fingerprint.to_string(), value))
                        }
                        None => Err(String::new()),
                    }
                })
                .await;
            match loaded {
                Ok((kept_for, value)) if kept_for == fingerprint => return Some(value),
                // Kept for another fingerprint: what it was derived from has changed.
                Ok(_) => self.cache.invalidate(),
                Err(_) => return None,
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn run<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(future)
    }

    fn unique(prefix: &str) -> &'static str {
        // The store belongs to the real settings directory: a unique name per test keeps each test private.
        Box::leak(format!("{prefix}-{}-{}", std::process::id(), now_secs()).into_boxed_str())
    }

    #[test]
    fn the_same_fingerprint_probes_once_and_a_new_one_probes_again() {
        run(async {
            let memo: Memo<u32> = Memo::new(unique("memo-test-same"));
            let runs = AtomicUsize::new(0);
            let probe = || async {
                runs.fetch_add(1, Ordering::SeqCst);
                Some(7u32)
            };
            assert_eq!(memo.get("one", probe).await, Some(7));
            assert_eq!(memo.get("one", probe).await, Some(7));
            assert_eq!(runs.load(Ordering::SeqCst), 1);
            assert_eq!(memo.get("two", probe).await, Some(7));
            assert_eq!(runs.load(Ordering::SeqCst), 2);
            memo.forget();
            assert_eq!(memo.get("two", probe).await, Some(7));
            assert_eq!(runs.load(Ordering::SeqCst), 3);
            memo.forget();
        });
    }

    #[test]
    fn a_failed_probe_is_not_kept() {
        run(async {
            let memo: Memo<u32> = Memo::new(unique("memo-test-failure"));
            assert_eq!(memo.get("f", || async { None }).await, None);
            assert_eq!(memo.get("f", || async { Some(1u32) }).await, Some(1));
            memo.forget();
        });
    }

    #[test]
    fn a_file_identity_changes_with_the_file() {
        let directory = std::env::temp_dir().join(format!("memo-identity-{}", now_secs()));
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("driver.sys");
        assert!(file_identity(&file).ends_with(":absent"));
        std::fs::write(&file, b"one").unwrap();
        let first = file_identity(&file);
        std::fs::write(&file, b"three").unwrap();
        assert_ne!(first, file_identity(&file));
        let _ = std::fs::remove_dir_all(&directory);
    }
}
