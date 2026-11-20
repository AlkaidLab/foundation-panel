//! Memory-adaptive release of idle WebViews. PROTOTYPE: off unless the person (or a measurement) turns it on, and nothing here changes what the panel does while it is off.
//!
//! The panel keeps three kinds of WebView alive after it has been used: the main window hidden to the tray (about 640 MB committed, 175 MB of it video memory), and, for the tray alone, the menu
//! window and the launch splash window that exist "so that the first right click and the first launch do not wait for a WebView2" (about 220 MB with the processes they pull in). This module lets them
//! go when they are idle and the machine would rather have the memory, and brings them back before they are needed:
//!
//! * **hidden main window** (`windows::main_window_sleeping`): destroyed through the same close as the title bar's x after `Policy::hidden_release`. The next open makes it again (the path of a first
//!   open) at the size and place it had.
//! * **tray menu window / splash window**: not made at the start when the policy keeps nothing warm (`Policy::warm_tray_windows`), destroyed when idle (`Policy::tray_idle_release`) while no main window
//!   shows a page. The icon carries the native menu meanwhile (ctx_popup `Health::Released`), so a right click is answered at once; a HOVER on the icon makes the Material menu window, and the launch
//!   announcement makes the splash (it is followed by the display setup, which takes longer than the window does).
//! * **pressure** (`mem_adaptive_core::classify`): one sample of the machine every `TICK`: `GlobalMemoryStatusEx` (available physical memory), `GetPerformanceInfo` (commit headroom) and the system's
//!   `LowMemoryResourceNotification`; elevated and critical pressure shorten the delays.
//!
//! Switches (all off by default): `SUNSHINE_GUI_MEM_MODE=adaptive|low` or `{"mode": "adaptive"}` in `<config>/Sunshine GUI/memory-mode.json`. For measurements: `SUNSHINE_GUI_MEM_HIDDEN_SECS`,
//! `SUNSHINE_GUI_MEM_TRAY_IDLE_SECS` (seconds, replace the delays at every pressure) and `SUNSHINE_GUI_MEM_FAKE_PRESSURE_FILE` (a file holding none|elevated|critical, read at every tick).

use crate::mem_adaptive_core::{self as core, Mode, Pressure};
use log::{debug, info, warn};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, Runtime, WebviewWindow};

/// How often the machine and the idle windows are looked at.
const TICK: Duration = Duration::from_secs(3);
/// A click older than this was not answered by the Material menu (the native menu took it).
const CLICK_STALE: Duration = Duration::from_secs(10);

pub fn mode() -> Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    *MODE.get_or_init(|| {
        let asked = std::env::var("SUNSHINE_GUI_MEM_MODE").ok().map(|text| Mode::parse(&text)).or_else(mode_from_file).unwrap_or(Mode::Off);
        if asked != Mode::Off {
            info!("[memadapt] mode {asked:?}");
        }
        asked
    })
}

fn mode_from_file() -> Option<Mode> {
    let path = dirs::config_dir()?.join("Sunshine GUI").join("memory-mode.json");
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    value.get("mode").and_then(|mode| mode.as_str()).map(Mode::parse)
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

/// What the mode does at the pressure of now, with the measurement overrides applied.
fn policy() -> core::Policy {
    let mut policy = core::policy(mode(), pressure());
    if mode() != Mode::Off {
        if let Some(delay) = core::override_secs(std::env::var("SUNSHINE_GUI_MEM_HIDDEN_SECS").ok().as_deref()) {
            policy.hidden_release = Some(delay);
        }
        if let Some(delay) = core::override_secs(std::env::var("SUNSHINE_GUI_MEM_TRAY_IDLE_SECS").ok().as_deref()) {
            policy.tray_idle_release = Some(delay);
        }
    }
    policy
}

/// The tray menu window and the splash window are made at the start (today's behaviour) unless the mode keeps nothing warm.
pub fn warm_tray_windows() -> bool {
    policy().warm_tray_windows
}

// ---- the clock ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------

/// Starts the poller. Does nothing while the mode is off.
pub fn start(app: &AppHandle) {
    if mode() == Mode::Off {
        return;
    }
    if let Some(sample) = system::sample() {
        info!(
            "[memadapt] {} MB installed, {} MB available: the preset this machine would get is {:?}",
            sample.total_phys >> 20,
            sample.avail_phys >> 20,
            Mode::suggested_for(sample.total_phys)
        );
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
    release_idle_tray(app, policy.tray_idle_release);
}

fn observe_pressure() {
    let before = pressure();
    let fake = std::env::var_os("SUNSHINE_GUI_MEM_FAKE_PRESSURE_FILE")
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| core::parse_fake_pressure(&text));
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
    let since = *held.main_since.get_or_insert_with(Instant::now);
    let hidden = core::held_for(Some(since), Instant::now());
    if hidden < delay {
        return;
    }
    held.main_since = None;
    keep_bounds(&window);
    info!("[memadapt] the main window has been hidden {} s at {:?} pressure: released", hidden.as_secs(), pressure());
    if !crate::windows::release_main_window(app) {
        warn!("[memadapt] the main window could not be released");
    }
}

fn release_idle_tray(app: &AppHandle, delay: Option<Duration>) {
    let Some(delay) = delay else { return };
    // a main window that shows a page (or may be restored from the taskbar) uses the menu window for the menus of its page
    if app.get_webview_window("main").is_some() && crate::windows::main_window_sleeping(app).is_none() {
        return;
    }
    if crate::ctx_popup::idle_for(app).is_some_and(|idle| idle >= delay) {
        crate::ctx_popup::release(app);
    }
    if crate::launch_splash::idle_for(app).is_some_and(|idle| idle >= delay) {
        crate::launch_splash::release(app);
    }
}

/// The pointer is on the tray icon (called for every event of the icon's hover).
pub fn on_tray_hover(app: &AppHandle) {
    if mode() != Mode::Off && crate::ctx_popup::is_released() {
        crate::ctx_popup::prewarm(app);
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
