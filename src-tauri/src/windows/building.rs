//! Where a window is built: never in a handler of the main thread.
//!
//! `WebviewWindowBuilder::build` (tauri 2.12, "Known issues") deadlocks on Windows when it is called from a synchronous command or an event handler: the main thread
//! waits for the WebView2 controller, whose completion is delivered to the same thread. The tray menu's page answers through a synchronous command (`tray_popup_act`),
//! so a window that had to be made there (the big screen, which is made on demand) froze the main thread inside the popup's own WebView2 callback; the window it had already
//! created, with `fullscreen(true)` and no page, stayed on screen drawn by the system from its style bits: a flat grey client area under the old title bar with the title
//! "Sunshine Desktop". (The panel log of the failing run ends with "创建桌面 UI 窗口..." and no "创建成功"; a main window built at a second launch, outside any WebView2
//! callback, was fine, and so is the in-app 大屏 button, an async command.)
//!
//! The tray page's command is async now, and the big screen is built on a worker thread (`run_off_main`) whichever handler asks for it, so the main thread is free to
//! deliver the WebView2 callbacks. `Making` makes one window per label at a time: a second request while the first is being built (a double click on the tray's tile) is dropped,
//! the window that comes shows itself when its page has loaded.

use std::cell::Cell;
use std::collections::HashSet;
use std::sync::Mutex;

use log::info;
use tauri::{AppHandle, Runtime};

thread_local! {
    static ON_WORKER: Cell<bool> = const { Cell::new(false) };
}

/// True on a thread of `run_off_main`: a window may be built here.
pub(crate) fn on_worker() -> bool {
    ON_WORKER.with(Cell::get)
}

/// Runs `job` on a worker thread. `what` names the window in panel.log.
pub(crate) fn run_off_main<R: Runtime>(app: &AppHandle<R>, what: &'static str, job: impl FnOnce(AppHandle<R>) + Send + 'static) {
    let app = app.clone();
    info!("[window] {what}: made on a worker thread");
    tauri::async_runtime::spawn_blocking(move || {
        ON_WORKER.with(|flag| flag.set(true));
        job(app);
        ON_WORKER.with(|flag| flag.set(false));
    });
}

/// The labels whose window is being built right now.
#[derive(Default)]
pub(crate) struct Making {
    labels: HashSet<String>,
}

impl Making {
    /// True when the caller may build `label`; false when another call is already building it.
    pub(crate) fn begin(&mut self, label: &str) -> bool {
        self.labels.insert(label.to_owned())
    }

    pub(crate) fn finish(&mut self, label: &str) {
        self.labels.remove(label);
    }

    #[cfg_attr(not(test), allow(dead_code))] // the rebuild guards assert the set through it
    pub(crate) fn is_making(&self, label: &str) -> bool {
        self.labels.contains(label)
    }
}

static MAKING: Mutex<Option<Making>> = Mutex::new(None);

fn making<T>(f: impl FnOnce(&mut Making) -> T) -> T {
    let mut guard = MAKING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    f(guard.get_or_insert_with(Making::default))
}

/// Builds the window `label` with `build`, once at a time. Returns what `build` returned; Ok(()) when another call is building the label.
pub(crate) fn build_once<E: std::fmt::Display>(label: &str, build: impl FnOnce() -> Result<(), E>) -> Result<(), String> {
    if !making(|m| m.begin(label)) {
        info!("[window] {label}: already being made, the request is dropped");
        return Ok(());
    }
    let result = build().map_err(|e| e.to_string());
    making(|m| m.finish(label));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_window_per_label_is_built_at_a_time() {
        let mut m = Making::default();
        assert!(m.begin("desktop"));
        assert!(m.is_making("desktop"));
        assert!(!m.begin("desktop"), "the second request while the first builds is dropped");
        assert!(m.begin("main"), "labels are independent");
        m.finish("desktop");
        assert!(!m.is_making("desktop"));
        assert!(m.begin("desktop"), "after the window is built the label is free again (a window closed since is built anew)");
    }

    #[test]
    fn a_failed_build_frees_the_label_and_a_request_during_a_build_builds_nothing() {
        let failed = build_once("t-fail", || Err::<(), _>("no webview"));
        assert_eq!(failed, Err("no webview".to_owned()));
        assert!(build_once("t-fail", || Ok::<(), String>(())).is_ok(), "the label is free after the failure");
        let mut second_built = false;
        let outer = build_once("t-nest", || {
            let inner = build_once("t-nest", || {
                second_built = true;
                Ok::<(), String>(())
            });
            assert!(inner.is_ok());
            Ok::<(), String>(())
        });
        assert!(outer.is_ok());
        assert!(!second_built, "a request that comes while the label is being built returns Ok and builds nothing");
    }

    #[test]
    fn the_worker_flag_is_set_only_on_a_worker() {
        assert!(!on_worker());
        std::thread::spawn(|| {
            ON_WORKER.with(|flag| flag.set(true));
            assert!(on_worker());
        })
        .join()
        .unwrap();
        assert!(!on_worker());
    }

    /// The tray menu's page answers through an async command (a synchronous one runs inside the page's own WebView2 callback) and the big screen is built on a worker.
    #[test]
    fn the_tray_menu_page_does_not_build_windows_inside_its_callback() {
        let code = |source: &str| -> String { source.lines().filter(|line| !line.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n") };
        let tray = code(include_str!("../tray.rs"));
        assert!(tray.contains("pub async fn tray_popup_act"), "tray_popup_act is an async command");
        assert!(!tray.contains("pub fn tray_popup_act"), "tray_popup_act is not a synchronous command");
        let windows = code(include_str!("../windows.rs"));
        let desktop = windows.split("pub fn open_desktop_window").nth(1).and_then(|rest| rest.split("\n}\n").next()).expect("open_desktop_window");
        assert!(desktop.contains("building::run_off_main") && desktop.contains("building::build_once"), "the big screen is built on a worker, once at a time");
        assert!(!desktop.contains("create_desktop_window(app).map_err"), "the old direct build is gone");
    }
}
