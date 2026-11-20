# Memory: adaptive release of idle windows

The panel keeps WebViews alive after use so that the next use is instant. This is what it lets go of when the machine would rather have the memory, and what it never lets go of.

## What is kept and what is released

| Window | Policy |
|---|---|
| Tray menu window (Material menu) | Never released, in any tier, at any pressure. The tray menu is always the Material menu. A click while the window is still being made waits for it. The native menu is only the last resort when WebView2 refuses the window after every attempt (`ctx_popup::Health::Failed`) or the person chose it in the settings. |
| Main window, hidden to the tray | Released after a delay that depends on the tier and the memory pressure (below), after its page state was kept. |
| Launch splash | Warm at the start in the adaptive tier while memory is plentiful; made on the host's announcement otherwise; released when idle. |

## Tiers, pressure, delays

* Default: `auto`. The installed memory picks the tier: up to 8.5 GB the low tier, above it the adaptive tier. `SUNSHINE_GUI_MEM_MODE=off|adaptive|low|auto` or `{"mode": "..."}` in `<config>/Sunshine GUI/memory-mode.json` overrides it (`off` releases nothing).
* Pressure is sampled every 3 s from `GlobalMemoryStatusEx`, `GetPerformanceInfo` and the system low-memory notification. High: available memory below max(15 %, 1.5 GB); severe: below max(6 %, 512 MB); a level is left at 125 % of its threshold.
* Main window hidden for: adaptive 300 s / 30 s / 5 s (none / high / severe), low 60 s / 20 s / 5 s.

## Coming back without being noticed

1. While the page is used, `inject-viewstate.js` keeps a snapshot (route, scroll positions, selected tabs, open sections, drafts typed by the person; never a password, hidden or file field, nor a field named like a secret, token, PIN, OTP or card) and hands it to the native side (`view_state.rs`). It lives in memory only and is never written to disk. The Web UI pages in the frame report theirs to the shell by `postMessage`.
2. A hover on the tray icon, or the tray menu opening, makes the main window again, hidden, with the snapshot in `window.__PANEL_RESTORE__`. The router starts from the saved stack; the rest is applied as the page draws, and stops the moment the person touches the page.
3. A click then shows a window that is already there, as it shows one that was never released. A click that comes first is shown the moment the page is ready.

## The guard

The time from the request to the shown window is measured for kept windows and for released ones. In a pressure level in which a released window was measurably slower than a kept one (more than the kept median, never below 250 ms, plus a quarter and at least 100 ms; or two slower of the last three), releasing stops for six hours (`memory-guard.json`). `SUNSHINE_GUI_MEM_GUARD=off` records without stopping (measurements).

## One WebView2 environment, and 0x8007139F

`0x8007139F` is `HRESULT_FROM_WIN32(ERROR_INVALID_STATE)`: WebView2 refuses to make a WebView when the options of the new environment differ from the WebViews already running in the browser process of the same user data folder (sources: `docs/authority/source-index.tsv`, `panel-ms-webview2-process-model`, `panel-ms-webview2-env-options-invalid-state`). The panel appended its switches to an inherited `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` on every start, so a panel started by a panel (restart after a crash, the elevated restart, the updater's relaunch) ran with the switches twice while the browser process of its parent still ran with them once. `webview_env.rs` composes the variable idempotently, makes every window from one environment object, retries a window that fails with this code (250, 750, 2000, 5000 ms) and logs the stage, the options in force and the WebView2 processes with their parents on every failure.

Measurement and reproduction scripts: `docs/memory-adaptive-measurements.md` is not kept here; the kit and its README live with the card `panel-mem-adaptive-1120`.
