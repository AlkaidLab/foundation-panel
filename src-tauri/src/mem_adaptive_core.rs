//! The decisions of the memory-adaptive release (mem_adaptive.rs), without the system: which mode is asked for, how much memory pressure a sample of the machine means, and what each
//! mode does about the windows that are idle at that pressure. Pure, so that the thresholds are tested with numbers.
//!
//! PROTOTYPE: every mode but `Off` is opt-in; nothing here runs unless the person (or a measurement) asked for it.
//!
//! Sources of the thresholds:
//! * Microsoft Learn, Win32 `GlobalMemoryStatusEx` / "Memory Performance Information" (ms.date 2018): `ullAvailPhys` is the physical memory that can be reused at once, `ullTotalPhys` the installed
//!   memory; `GetPerformanceInfo` gives `CommitLimit - CommitTotal` (in pages), the commit that can still be promised.
//! * Microsoft Learn, `CreateMemoryResourceNotification` (ms.date 2018): "If available memory is low, the application can reduce its working set." The object is system-wide and signals only when
//!   `LowMemoryResourceNotification` is true; the page gives no byte figure, so it is used as a hard "critical" signal and not as the only one.
//! * Microsoft Learn, WebView2 "Performance best practices" (ms.date 2026-01-12): release what is not used; share environments; `MemoryUsageTargetLevel = Low` on inactive WebViews (lifecycle.rs).

use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// today's behaviour, nothing released
    Off,
    /// the windows stay warm while memory is plentiful; idle ones go when the machine is short of memory
    Adaptive,
    /// the preset for machines short of memory: nothing is kept warm for a first use; a hover warms the tray menu, idle windows go quickly
    Low,
}

impl Mode {
    pub fn parse(text: &str) -> Mode {
        match text.trim().to_ascii_lowercase().as_str() {
            "adaptive" | "auto" => Mode::Adaptive,
            "low" | "low-memory" | "lowmemory" => Mode::Low,
            _ => Mode::Off,
        }
    }

    /// The mode a preset is chosen for: the installed memory in bytes. At most 8 GB the low-memory preset is the default of a first start (the prototype only offers the number).
    pub fn suggested_for(total_phys: u64) -> Mode {
        if total_phys == 0 {
            Mode::Off
        } else if total_phys <= 8 * GB + GB / 2 {
            // 8 GB machines report 7.4 to 8.0 GB as installed memory once the firmware has taken its share
            Mode::Low
        } else {
            Mode::Adaptive
        }
    }
}

const MB: u64 = 1 << 20;
const GB: u64 = 1 << 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pressure {
    None,
    Elevated,
    Critical,
}

/// One look at the machine. `commit_headroom` is `u64::MAX` when it is not known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub total_phys: u64,
    pub avail_phys: u64,
    pub commit_headroom: u64,
    /// `QueryMemoryResourceNotification(LowMemoryResourceNotification)` is signalled
    pub low_signal: bool,
}

/// Available physical memory below which the pressure is elevated: 15 % of the installed memory, never less than 1.5 GB (an 8 GB machine: 1.5 GB, 16 GB: 2.4 GB, 32 GB: 4.8 GB).
pub fn elevated_below(total_phys: u64) -> u64 {
    (total_phys / 100 * 15).max(GB + GB / 2)
}

/// Available physical memory below which it is critical: 6 % of the installed memory, never less than 512 MB.
pub fn critical_below(total_phys: u64) -> u64 {
    (total_phys / 100 * 6).max(512 * MB)
}

const COMMIT_ELEVATED: u64 = 2 * GB;
const COMMIT_CRITICAL: u64 = 768 * MB;
/// A level that is already held is left only when the machine is this much (percent) clear of its threshold.
const LEAVE_MARGIN_PERCENT: u64 = 125;

fn reaches(sample: &Sample, level: Pressure, percent: u64) -> bool {
    let (phys, commit) = match level {
        Pressure::None => return true,
        Pressure::Elevated => (elevated_below(sample.total_phys), COMMIT_ELEVATED),
        Pressure::Critical => (critical_below(sample.total_phys), COMMIT_CRITICAL),
    };
    sample.avail_phys < phys / 100 * percent
        || sample.commit_headroom < commit / 100 * percent
        || (level == Pressure::Critical && sample.low_signal)
}

/// The pressure a sample means, given the pressure before it: a level is entered at its threshold and left 25 % beyond it, so a machine that hovers at a threshold does not flap.
pub fn classify(sample: &Sample, before: Pressure) -> Pressure {
    let percent = |level: Pressure| if before >= level { LEAVE_MARGIN_PERCENT } else { 100 };
    if reaches(sample, Pressure::Critical, percent(Pressure::Critical)) {
        Pressure::Critical
    } else if reaches(sample, Pressure::Elevated, percent(Pressure::Elevated)) {
        Pressure::Elevated
    } else {
        Pressure::None
    }
}

/// What a mode does at a pressure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    /// the main window, hidden to the tray (not minimized), is destroyed after this long; None keeps it
    pub hidden_release: Option<Duration>,
    /// the tray menu window and the launch splash window are made at the start (a first right click and a first launch find them there)
    pub warm_tray_windows: bool,
    /// the tray menu window and the splash window, idle and with no main window up, are destroyed after this long; None keeps them
    pub tray_idle_release: Option<Duration>,
}

const fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

pub fn policy(mode: Mode, pressure: Pressure) -> Policy {
    match (mode, pressure) {
        (Mode::Off, _) => Policy { hidden_release: None, warm_tray_windows: true, tray_idle_release: None },
        (Mode::Adaptive, Pressure::None) => Policy { hidden_release: Some(secs(300)), warm_tray_windows: true, tray_idle_release: None },
        (Mode::Adaptive, Pressure::Elevated) => Policy { hidden_release: Some(secs(30)), warm_tray_windows: false, tray_idle_release: Some(secs(60)) },
        (Mode::Adaptive, Pressure::Critical) => Policy { hidden_release: Some(secs(5)), warm_tray_windows: false, tray_idle_release: Some(secs(15)) },
        (Mode::Low, Pressure::None) => Policy { hidden_release: Some(secs(60)), warm_tray_windows: false, tray_idle_release: Some(secs(60)) },
        (Mode::Low, Pressure::Elevated) => Policy { hidden_release: Some(secs(20)), warm_tray_windows: false, tray_idle_release: Some(secs(30)) },
        (Mode::Low, Pressure::Critical) => Policy { hidden_release: Some(secs(5)), warm_tray_windows: false, tray_idle_release: Some(secs(10)) },
    }
}

/// Measurement overrides (SUNSHINE_GUI_MEM_HIDDEN_SECS / SUNSHINE_GUI_MEM_TRAY_IDLE_SECS): a number of seconds replaces the delay at every pressure; 0 or text keeps the policy's.
pub fn override_secs(text: Option<&str>) -> Option<Duration> {
    let seconds: u64 = text?.trim().parse().ok()?;
    (seconds > 0).then(|| Duration::from_secs(seconds))
}

/// A simulated pressure (SUNSHINE_GUI_MEM_FAKE_PRESSURE_FILE holds a word): used by measurements to drive the policy without taking the machine's memory.
pub fn parse_fake_pressure(text: &str) -> Option<Pressure> {
    match text.trim().to_ascii_lowercase().as_str() {
        "none" | "ok" => Some(Pressure::None),
        "elevated" | "low" => Some(Pressure::Elevated),
        "critical" => Some(Pressure::Critical),
        _ => None,
    }
}

/// How long something has been in a state, as the poller sees it: the instant it was first seen in it (None when it is not in it now).
pub fn held_for(first_seen: Option<std::time::Instant>, now: std::time::Instant) -> Duration {
    first_seen.map_or(Duration::ZERO, |at| now.saturating_duration_since(at))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(total_gb: u64, avail_mb: u64) -> Sample {
        Sample { total_phys: total_gb * GB, avail_phys: avail_mb * MB, commit_headroom: u64::MAX, low_signal: false }
    }

    #[test]
    fn modes_are_parsed_from_words_and_anything_else_is_off() {
        assert_eq!(Mode::parse("adaptive"), Mode::Adaptive);
        assert_eq!(Mode::parse(" LOW "), Mode::Low);
        assert_eq!(Mode::parse("low-memory"), Mode::Low);
        assert_eq!(Mode::parse(""), Mode::Off);
        assert_eq!(Mode::parse("yes"), Mode::Off);
    }

    #[test]
    fn the_preset_is_chosen_by_installed_memory() {
        assert_eq!(Mode::suggested_for(0), Mode::Off);
        assert_eq!(Mode::suggested_for(4 * GB), Mode::Low);
        assert_eq!(Mode::suggested_for(7_900 * MB), Mode::Low);
        assert_eq!(Mode::suggested_for(8 * GB), Mode::Low);
        assert_eq!(Mode::suggested_for(16 * GB), Mode::Adaptive);
        assert_eq!(Mode::suggested_for(32 * GB), Mode::Adaptive);
    }

    #[test]
    fn thresholds_follow_the_installed_memory_with_floors() {
        assert_eq!(elevated_below(8 * GB), GB + GB / 2, "8 GB: the floor");
        assert_eq!(elevated_below(32 * GB), 32 * GB / 100 * 15);
        assert_eq!(critical_below(4 * GB), 512 * MB, "4 GB: the floor");
        assert_eq!(critical_below(64 * GB), 64 * GB / 100 * 6);
        for total in [4, 8, 16, 32, 64] {
            assert!(critical_below(total * GB) < elevated_below(total * GB));
        }
    }

    #[test]
    fn plenty_of_memory_is_no_pressure() {
        assert_eq!(classify(&sample(16, 6_000), Pressure::None), Pressure::None);
        assert_eq!(classify(&sample(8, 3_000), Pressure::None), Pressure::None);
    }

    #[test]
    fn short_memory_is_elevated_then_critical() {
        assert_eq!(classify(&sample(16, 2_000), Pressure::None), Pressure::Elevated);
        assert_eq!(classify(&sample(8, 1_400), Pressure::None), Pressure::Elevated);
        assert_eq!(classify(&sample(16, 800), Pressure::None), Pressure::Critical);
        assert_eq!(classify(&sample(8, 400), Pressure::None), Pressure::Critical);
    }

    #[test]
    fn the_system_low_memory_signal_is_critical_whatever_the_figures_say() {
        let mut s = sample(16, 6_000);
        s.low_signal = true;
        assert_eq!(classify(&s, Pressure::None), Pressure::Critical);
        s.low_signal = false;
        assert_eq!(classify(&s, Pressure::Critical), Pressure::None, "plenty of memory leaves any level");
    }

    #[test]
    fn little_commit_headroom_counts_as_pressure_with_memory_free() {
        let mut s = sample(16, 6_000);
        s.commit_headroom = GB + GB / 2;
        assert_eq!(classify(&s, Pressure::None), Pressure::Elevated);
        s.commit_headroom = 500 * MB;
        assert_eq!(classify(&s, Pressure::None), Pressure::Critical);
    }

    #[test]
    fn a_level_is_left_only_with_a_margin() {
        // 16 GB: elevated below 2457 MB; held until 25 % beyond that (3071 MB)
        assert_eq!(classify(&sample(16, 2_600), Pressure::None), Pressure::None);
        assert_eq!(classify(&sample(16, 2_600), Pressure::Elevated), Pressure::Elevated, "held");
        assert_eq!(classify(&sample(16, 3_200), Pressure::Elevated), Pressure::None, "left");
        // critical drops to elevated first, not to none
        assert_eq!(classify(&sample(16, 1_300), Pressure::Critical), Pressure::Elevated);
    }

    #[test]
    fn off_changes_nothing_at_any_pressure() {
        for pressure in [Pressure::None, Pressure::Elevated, Pressure::Critical] {
            let p = policy(Mode::Off, pressure);
            assert_eq!((p.hidden_release, p.warm_tray_windows, p.tray_idle_release), (None, true, None));
        }
    }

    #[test]
    fn adaptive_keeps_everything_warm_while_memory_is_plentiful() {
        let p = policy(Mode::Adaptive, Pressure::None);
        assert!(p.warm_tray_windows);
        assert_eq!(p.tray_idle_release, None);
        assert_eq!(p.hidden_release, Some(Duration::from_secs(300)));
    }

    #[test]
    fn pressure_only_shortens_delays_and_never_lengthens_them() {
        for mode in [Mode::Adaptive, Mode::Low] {
            let levels = [Pressure::None, Pressure::Elevated, Pressure::Critical].map(|p| policy(mode, p));
            for pair in levels.windows(2) {
                assert!(pair[1].hidden_release <= pair[0].hidden_release);
                assert!(pair[1].tray_idle_release <= pair[0].tray_idle_release || pair[0].tray_idle_release.is_none());
                assert!(!pair[1].warm_tray_windows || pair[0].warm_tray_windows);
            }
        }
    }

    #[test]
    fn low_never_warms_the_tray_windows() {
        for pressure in [Pressure::None, Pressure::Elevated, Pressure::Critical] {
            assert!(!policy(Mode::Low, pressure).warm_tray_windows);
        }
    }

    #[test]
    fn overrides_are_whole_seconds_above_zero() {
        assert_eq!(override_secs(Some("90")), Some(Duration::from_secs(90)));
        assert_eq!(override_secs(Some(" 15 ")), Some(Duration::from_secs(15)));
        assert_eq!(override_secs(Some("0")), None);
        assert_eq!(override_secs(Some("soon")), None);
        assert_eq!(override_secs(None), None);
    }

    #[test]
    fn fake_pressure_words() {
        assert_eq!(parse_fake_pressure("critical\n"), Some(Pressure::Critical));
        assert_eq!(parse_fake_pressure("Elevated"), Some(Pressure::Elevated));
        assert_eq!(parse_fake_pressure("none"), Some(Pressure::None));
        assert_eq!(parse_fake_pressure("?"), None);
    }

    #[test]
    fn held_for_counts_from_the_first_sighting() {
        let base = std::time::Instant::now();
        assert_eq!(held_for(None, base), Duration::ZERO);
        assert_eq!(held_for(Some(base), base + Duration::from_secs(7)), Duration::from_secs(7));
    }
}
