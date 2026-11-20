//! Memory-adaptive release of the idle main window and the launch splash. On by default on every machine; `SUNSHINE_GUI_MEM_MODE=off|adaptive|low|auto` (or `{"mode": "..."}` in
//! `<config>/Sunshine GUI/memory-mode.json`) overrides the choice the controller makes.
//!
//! What the panel keeps alive after it has been used: the main window hidden to the tray (about 640 MB committed, 175 MB of it video memory), the small window of the tray menu and the launch splash
//! window ("so that the first right click and the first launch do not wait for a WebView2"). This module lets the main window and the splash go when they are idle and the machine would rather have
//! the memory, and brings the main window back before it is needed. The tray menu window is never let go: the tray menu is always the Material menu, in every tier and at every pressure, and the window
//! shares the WebView2 environment of the others (webview_env.rs), so keeping it is cheap.
//!
//! * **tier** (`Setting::resolve`): the installed memory picks it unless it is set: up to 8.5 GB the low tier (the main window goes after 60 s hidden), above it the adaptive tier (after 300 s, and
//!   sooner under pressure).
//! * **pressure** (`mem_adaptive_core::classify`): one sample of the machine every `TICK`: `GlobalMemoryStatusEx` (available physical memory), `GetPerformanceInfo` (commit headroom) and the system's
//!   `LowMemoryResourceNotification`; elevated and critical pressure shorten the delays.
//! * **hidden main window** (`windows::main_window_sleeping`): destroyed through the same close as the title bar's x after `Policy::hidden_release`, after the page's state was kept (view_state.rs).
//! * **coming back without being noticed**: a hover on the tray icon, or the tray menu opening, makes the main window again, hidden, from the snapshot of the one that went (route, scroll, tabs, sections,
//!   drafts: inject-viewstate.js). A click then shows a window that is already there, the way it shows one that was never let go. If the click came first, the window is shown the moment its page is ready.
//! * **the guard** (`mem_adaptive_core::Guard`): the time from the request to the shown window is measured for kept windows and for windows that were let go. In a situation (a pressure level) in which
//!   the released one was measurably slower than the kept one, releasing stops for six hours (kept in `memory-guard.json`), whatever the delay says.
//!
//! Measurements: `SUNSHINE_GUI_MEM_HIDDEN_SECS`, `SUNSHINE_GUI_MEM_TRAY_IDLE_SECS` (seconds, replace the delays at every pressure), `SUNSHINE_GUI_MEM_FAKE_PRESSURE_FILE` (a file holding
//! none|elevated|critical, read at every tick) and `SUNSHINE_GUI_MEM_GUARD=off` (the guard records but does not stop releasing).

use crate::mem_adaptive_core::{self as core, Guard, Mode, Pressure, Setting, Verdict};
use log::{debug, info, warn};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, Runtime, WebviewWindow};

/// How often the machine and the idle windows are looked at.
const TICK: Duration = Duration::from_secs(3);
/// A click older than this was not answered by the Material menu.
const CLICK_STALE: Duration = Duration::from_secs(10);
/// A main window made ahead of a click is left alone this long before the next release delay starts.
const PREWARM_GRACE: Duration = Duration::from_secs(25);

// ---- the tier ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------

pub fn setting() -> Setting {
    static SETTING: OnceLock<Setting> = OnceLock::new();
    *SETTING.get_or_init(|| std::env::var("SUNSHINE_GUI_MEM_MODE").ok().map(|text| Setting::parse(&text)).or_else(setting_from_file).unwrap_or(Setting::Auto))
}

fn setting_from_file() -> Option<Setting> {
    let path = dirs::config_dir()?.join("Sunshine GUI").join("memory-mode.json");
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    value.get("mode").and_then(|mode| mode.as_str()).map(Setting::parse)
}

/// The tier of this machine, decided once: the setting, or the preset for the installed memory.
pub fn mode() -> Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    *MODE.get_or_init(|| {
        let total = system::sample().map_or(0, |sample| sample.total_phys);
        let mode = setting().resolve(total);
        info!("[memadapt] tier {mode:?} (setting {:?}, {} MB installed)", setting(), total >> 20);
        mode
    })
}

static PRESSURE: AtomicU8 = AtomicU8::new(0);

pub fn pressure() -> Pressure {
    match PRESSURE.load(Ordering::Relaxed) {
        2 => Pressure::Critical,
        1 => Pressure::Elevated,
        _ => Pressure::None,
    }
}

fn set_pressure(pressure: Pressure) {
    PRESSURE.store(pressure as u8, Ordering::Relaxed);
}

/// What the tier does at the pressure of now, with the measurement overrides applied.
fn policy() -> core::Policy {
    let mut policy = core::policy(mode(), pressure());
    if mode() != Mode::Off {
        if let Some(delay) = core::override_secs(std::env::var("SUNSHINE_GUI_MEM_HIDDEN_SECS").ok().as_deref()) {
            policy.hidden_release = Some(delay);
        }
        if let Some(delay) = core::override_secs(std::env::var("SUNSHINE_GUI_MEM_TRAY_IDLE_SECS").ok().as_deref()) {
            policy.splash_idle_release = Some(delay);
        }
    }
    policy
}

/// The splash window is made at the start (a first launch finds it there) unless the tier keeps nothing warm for it.
pub fn warm_splash() -> bool {
    policy().warm_splash
}

// ---- the clock ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------

/// Starts the poller. Does nothing while the tier is off.
pub fn start(app: &AppHandle) {
    if mode() == Mode::Off {
        info!("[memadapt] off: nothing is released");
        return;
    }
    load_guard();
    if let Some(sample) = system::sample() {
        info!("[memadapt] {} MB installed, {} MB available", sample.total_phys >> 20, sample.avail_phys >> 20);
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut hidden = Held::default();
        loop {
            tokio::time::sleep(TICK).await;
            tick(&app, &mut hidden);
        }
    });
}

#[derive(Default)]
struct Held {
    main_since: Option<Instant>,
}

fn tick(app: &AppHandle, held: &mut Held) {
    observe_pressure();
    let policy = policy();
    release_hidden_main(app, held, policy.hidden_release);
    release_idle_splash(app, policy.splash_idle_release);
}

fn observe_pressure() {
    let before = pressure();
    let fake = std::env::var_os("SUNSHINE_GUI_MEM_FAKE_PRESSURE_FILE").and_then(|path| std::fs::read_to_string(path).ok()).and_then(|text| core::parse_fake_pressure(&text));
    let sample = system::sample();
    let now = fake.or_else(|| sample.map(|sample| core::classify(&sample, before))).unwrap_or(Pressure::None);
    if now != before {
        match sample {
            Some(sample) => info!(
                "[memadapt] pressure {before:?} -> {now:?}{}: {} MB of {} MB physical available, commit headroom {} MB, system low-memory signal {}",
                if fake.is_some() { " (simulated)" } else { "" },
                sample.avail_phys >> 20,
                sample.total_phys >> 20,
                if sample.commit_headroom == u64::MAX { u64::MAX } else { sample.commit_headroom >> 20 },
                sample.low_signal
            ),
            None => info!("[memadapt] pressure {before:?} -> {now:?} (simulated, the system gave no sample)"),
        }
        set_pressure(now);
    }
}

fn release_hidden_main(app: &AppHandle, held: &mut Held, delay: Option<Duration>) {
    let sleeping = delay.and_then(|_| crate::windows::main_window_sleeping(app));
    let (Some(delay), Some(window)) = (delay, sleeping) else {
        held.main_since = None;
        return;
    };
    // a window made ahead of a click is given time to be clicked
    let warming = {
        let state = prewarm_state();
        state.building || state.ready_at.is_some_and(|at| at.elapsed() < PREWARM_GRACE)
    };
    if warming {
        held.main_since = None;
        return;
    }
    let since = *held.main_since.get_or_insert_with(Instant::now);
    let hidden = core::held_for(Some(since), Instant::now());
    if hidden < delay {
        return;
    }
    if !release_allowed() {
        // the guard: the last reopen after a release in this situation was slower than a kept window's
        debug!("[memadapt] release held back by the guard at {:?} pressure", pressure());
        return;
    }
    held.main_since = None;
    keep_bounds(&window);
    info!("[memadapt] the main window has been hidden {} s at {:?} pressure: released", hidden.as_secs(), pressure());
    crate::view_state::arm_restore();
    prewarm_state().ready_at = None;
    RELEASE_PRESSURE.store(pressure() as u8, Ordering::Relaxed);
    RELEASED_MAIN.store(true, Ordering::SeqCst);
    if !crate::windows::release_main_window(app) {
        warn!("[memadapt] the main window could not be released");
        RELEASED_MAIN.store(false, Ordering::SeqCst);
        crate::view_state::disarm_restore();
    }
}

fn release_idle_splash(app: &AppHandle, delay: Option<Duration>) {
    let Some(delay) = delay else { return };
    // a main window that shows a page does not need the splash let go (it is a memory release of the idle machine)
    if app.get_webview_window("main").is_some() && crate::windows::main_window_sleeping(app).is_none() {
        return;
    }
    if crate::launch_splash::idle_for(app).is_some_and(|idle| idle >= delay) {
        crate::launch_splash::release(app);
    }
}

// ---- coming back ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------

/// The main window was let go and has not been shown again yet.
static RELEASED_MAIN: AtomicBool = AtomicBool::new(false);
/// The pressure level the last release happened at (the situation the guard judges it by).
static RELEASE_PRESSURE: AtomicU8 = AtomicU8::new(0);
#[derive(Default)]
struct Prewarm {
    building: bool,
    ready_at: Option<Instant>,
    requested: bool,
    target_url: Option<String>,
}

static PREWARM: Mutex<Prewarm> = Mutex::new(Prewarm {
    building: false, ready_at: None, requested: false, target_url: None,
});

fn prewarm_state() -> std::sync::MutexGuard<'static, Prewarm> {
    PREWARM.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Queue activation atomically with the end of a prewarm. Keep the most recent
/// explicit URL if a plain activation arrives before construction finishes.
pub fn defer_open(target_url: &mut Option<String>) -> bool {
    let mut prewarm = prewarm_state();
    if !prewarm.building {
        return false;
    }
    prewarm.requested = true;
    if target_url.is_some() {
        prewarm.target_url = target_url.take();
    }
    true
}

/// A pointer gesture, a tray menu or a shortcut can precede activation.
/// The single-instance callback can also begin construction while handling its arguments.
pub fn prewarm_main<R: Runtime>(app: &AppHandle<R>, why: &str) {
    if !RELEASED_MAIN.load(Ordering::SeqCst) || app.get_webview_window("main").is_some() || crate::windows::main_window_closing() {
        return;
    }
    {
        let mut prewarm = prewarm_state();
        if prewarm.building {
            return;
        }
        prewarm.building = true;
    }
    info!("[memadapt] {why}: the main window is made ahead of the click");
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let started = Instant::now();
        let ready = match crate::windows::create_main_window_hidden(&app) {
            Ok(()) => {
                info!("[memadapt] the main window was made ahead in {} ms", started.elapsed().as_millis());
                true
            }
            Err(error) => {
                warn!("[memadapt] the main window could not be made ahead: {error}");
                false
            }
        };
        let (requested, target_url) = {
            let mut prewarm = prewarm_state();
            prewarm.building = false;
            prewarm.ready_at = ready.then(Instant::now);
            (std::mem::take(&mut prewarm.requested), prewarm.target_url.take())
        };
        if requested {
            crate::windows::activate_main_window(&app, target_url);
        }
    });
}

// ---- the guard ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------

static GUARD: Mutex<Option<Guard>> = Mutex::new(None);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reopen {
    /// the window was hidden and is shown again (never let go)
    Kept,
    /// the window was let go and made ahead of the click
    Prewarmed,
    /// the window was let go and the click had to make it
    Cold,
    /// the first window of the run
    First,
}

static OPEN_REQUEST: Mutex<Option<(Instant, Reopen)>> = Mutex::new(None);
/// An ask for the window older than this is a stale one (the window was never shown for it).
const OPEN_REQUEST_TTL: Duration = Duration::from_secs(30);

fn guard<T>(f: impl FnOnce(&mut Guard) -> T) -> T {
    let mut slot = GUARD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    f(slot.get_or_insert_with(Guard::default))
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

fn guard_path() -> Option<std::path::PathBuf> {
    Some(dirs::config_dir()?.join("Sunshine GUI").join("memory-guard.json"))
}

fn load_guard() {
    let Some(text) = guard_path().and_then(|path| std::fs::read_to_string(path).ok()) else { return };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')) else { return };
    let Some(list) = value.get("blocked_until").and_then(|list| list.as_array()) else { return };
    let mut blocks = [0u64; 3];
    for (slot, item) in blocks.iter_mut().zip(list) {
        *slot = item.as_u64().unwrap_or(0);
    }
    guard(|guard| *guard = Guard::with_blocks(blocks));
    if blocks.iter().any(|&until| until > now_secs()) {
        info!("[memadapt] the guard holds releases back in some situations until {blocks:?} (seconds since the epoch)");
    }
}

fn save_guard() {
    let blocks = guard(|guard| guard.blocks());
    let Some(path) = guard_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(error) = std::fs::write(path, serde_json::json!({ "blocked_until": blocks }).to_string()) {
        debug!("[memadapt] the guard could not be kept: {error}");
    }
}

fn guard_enabled() -> bool {
    !std::env::var("SUNSHINE_GUI_MEM_GUARD").is_ok_and(|value| value.eq_ignore_ascii_case("off"))
}

/// Whether the window may be let go at the pressure of now.
fn release_allowed() -> bool {
    !guard_enabled() || guard(|guard| guard.cold_open_qualified() && guard.allows_release(pressure(), now_secs()))
}

/// The person asked for the main window (a click, a hotkey, a second start): the clock of the reopen starts. Only a window that is hidden or absent counts.
pub fn note_open_request(existing_hidden: bool, exists: bool) {
    if exists && !existing_hidden {
        return;
    }
    let kind = if RELEASED_MAIN.load(Ordering::SeqCst) {
        let prewarm = prewarm_state();
        if prewarm.building || prewarm.ready_at.is_some() { Reopen::Prewarmed } else { Reopen::Cold }
    } else if exists {
        Reopen::Kept
    } else {
        Reopen::First
    };
    let mut request = OPEN_REQUEST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    // a second ask for the same window (the click that waits for a window being made, and then the activation that shows it) keeps the first clock
    if request.is_some_and(|(at, _)| at.elapsed() < OPEN_REQUEST_TTL) {
        return;
    }
    *request = Some((Instant::now(), kind));
}

/// The main window is on screen: the reopen is timed and judged.
pub fn note_main_shown() {
    let Some((at, kind)) = OPEN_REQUEST.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take() else { return };
    let ms = at.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
    match kind {
        Reopen::First => {
            let limit = guard(|guard| {
                guard.record_cold_open(ms);
                guard.limit_ms()
            });
            info!("[memadapt] first cold window: {ms} ms against {limit} ms; main release requires a qualified cold open");
        }
        Reopen::Kept => {
            guard(|guard| guard.record_kept(ms));
            info!("[memadapt] reopen kept window: {ms} ms");
        }
        Reopen::Prewarmed | Reopen::Cold => {
            let situation = match RELEASE_PRESSURE.load(Ordering::Relaxed) {
                2 => Pressure::Critical,
                1 => Pressure::Elevated,
                _ => Pressure::None,
            };
            let (verdict, limit) = guard(|guard| (guard.record_released(situation, ms, now_secs()), guard.limit_ms()));
            info!(
                "[memadapt] reopen after a release ({}): {ms} ms against {limit} ms for a kept window: {verdict:?}{}",
                if kind == Reopen::Prewarmed { "made ahead" } else { "made by the click" },
                if verdict == Verdict::Blocked && guard_enabled() { "; releasing stops in this situation for six hours" } else { "" }
            );
            if verdict == Verdict::Blocked && guard_enabled() {
                save_guard();
            }
            RELEASED_MAIN.store(false, Ordering::SeqCst);
            prewarm_state().ready_at = None;
            crate::view_state::clear_carry();
        }
    }
}

// ---- timings --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------

static CLICK_AT: Mutex<Option<Instant>> = Mutex::new(None);

/// A right click on the tray icon arrived (the instant the Material menu's time is counted from).
pub fn note_click() {
    *CLICK_AT.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Instant::now());
}

/// The Material menu is on screen: logs the time since the right click (it includes a wait for the window when the click came while it was being made).
pub fn note_menu_shown() {
    let at = CLICK_AT.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
    if let Some(at) = at.filter(|at| at.elapsed() < CLICK_STALE) {
        info!("[memadapt] right click -> Material menu on screen in {} ms", at.elapsed().as_millis());
    }
}

// ---- the main window's place -------------------------------------------------------------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Bounds {
    position: (i32, i32),
    size: (u32, u32),
    maximized: bool,
}

static BOUNDS: Mutex<Option<Bounds>> = Mutex::new(None);

fn keep_bounds<R: Runtime>(window: &WebviewWindow<R>) {
    let (Ok(position), Ok(size)) = (window.outer_position(), window.inner_size()) else { return };
    let maximized = window.is_maximized().unwrap_or(false);
    *BOUNDS.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Bounds { position: (position.x, position.y), size: (size.width, size.height), maximized });
}

/// A window that is made again after a release gets the place and size the released one had (once; only if that place is still on a monitor).
pub fn restore_bounds<R: Runtime>(window: &WebviewWindow<R>) {
    let Some(bounds) = BOUNDS.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take() else { return };
    let on_a_monitor = window.available_monitors().map(|monitors| {
        monitors.iter().any(|monitor| {
            let (p, s) = (monitor.position(), monitor.size());
            bounds.position.0 + 80 > p.x && bounds.position.0 + 80 < p.x + s.width as i32 && bounds.position.1 + 40 > p.y && bounds.position.1 + 40 < p.y + s.height as i32
        })
    });
    if !on_a_monitor.unwrap_or(false) {
        debug!("[memadapt] the place of the released window is on no monitor any more: the default place is kept");
        return;
    }
    let _ = window.set_size(PhysicalSize::new(bounds.size.0, bounds.size.1));
    let _ = window.set_position(PhysicalPosition::new(bounds.position.0, bounds.position.1));
    if bounds.maximized {
        let _ = window.maximize();
    }
    info!("[memadapt] the window is made again at {:?} {:?}{}", bounds.position, bounds.size, if bounds.maximized { " (maximized)" } else { "" });
}

// ---- the machine ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod system {
    use crate::mem_adaptive_core::Sample;
    use std::sync::OnceLock;

    /// One look: available physical memory (`GlobalMemoryStatusEx`, Microsoft Learn "MEMORYSTATUSEX": `ullAvailPhys`), the commit that can still be promised (`GetPerformanceInfo`:
    /// `CommitLimit - CommitTotal` pages) and the system's low-memory notification (`CreateMemoryResourceNotification`, `QueryMemoryResourceNotification`).
    pub fn sample() -> Option<Sample> {
        use windows::Win32::System::ProcessStatus::{GetPerformanceInfo, PERFORMANCE_INFORMATION};
        use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        unsafe {
            let mut status: MEMORYSTATUSEX = std::mem::zeroed();
            status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
            GlobalMemoryStatusEx(&mut status).ok()?;
            let mut performance: PERFORMANCE_INFORMATION = std::mem::zeroed();
            let size = std::mem::size_of::<PERFORMANCE_INFORMATION>() as u32;
            performance.cb = size;
            let commit_headroom = if GetPerformanceInfo(&mut performance, size).is_ok() {
                (performance.CommitLimit.saturating_sub(performance.CommitTotal) as u64).saturating_mul(performance.PageSize as u64)
            } else {
                u64::MAX
            };
            Some(Sample { total_phys: status.ullTotalPhys, avail_phys: status.ullAvailPhys, commit_headroom, low_signal: low_memory_signalled() })
        }
    }

    fn low_memory_signalled() -> bool {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Memory::{CreateMemoryResourceNotification, LowMemoryResourceNotification, QueryMemoryResourceNotification};
        static NOTIFICATION: OnceLock<isize> = OnceLock::new();
        let handle = *NOTIFICATION.get_or_init(|| unsafe { CreateMemoryResourceNotification(LowMemoryResourceNotification).map(|handle| handle.0 as isize).unwrap_or(0) });
        if handle == 0 {
            return false;
        }
        let mut state = windows::core::BOOL(0);
        unsafe { QueryMemoryResourceNotification(HANDLE(handle as *mut core::ffi::c_void), &mut state).is_ok() && state.as_bool() }
    }
}

#[cfg(not(target_os = "windows"))]
mod system {
    use crate::mem_adaptive_core::Sample;

    pub fn sample() -> Option<Sample> {
        None
    }
}
