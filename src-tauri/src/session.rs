//! Managed game sessions (v0.2.9): destroy the window on launch.
//!
//! When the "Close UI to tray while gaming (saves RAM)" setting is ON, the
//! launcher window is DESTROYED when a game launches — not just hidden.
//! A hidden WebView2 window still costs ~150-195 MB of RAM and a live
//! renderer process; destroying it leaves only the ~4 MB Rust host in
//! the system tray (the tray icon recreates the window on click).
//!
//! The backend owns the whole lifecycle, because the frontend dies
//! with the window:
//! - `begin_managed_launch` starts the playtime session, spawns a
//!   watcher thread, then destroys the window.
//! - The watcher polls for the game to exit (Steam: `RunningAppID`;
//!   exe games: process polling, same seen/checks>=5 rule as the old
//!   frontend polls) and then ends the session and recreates the
//!   window.
//! - `show_or_recreate_window` is also the tray-click handler and
//!   the manual fallback: if recreation ever fails, clicking the tray
//!   icon tries again. Creation is retried up to 3 times; if the
//!   window already exists it is simply shown and focused.
//!
//! Toggle semantics (documented decision): the setting keeps its key
//! (`minimize_on_launch`) and its label ("Close UI to tray while
//! gaming (saves RAM)"). ON now means "the launcher window closes
//! while a game runs" instead of "hides": from the user's point of
//! view the window still disappears when a game starts and comes back
//! when it exits — it is just lighter while gone. OFF keeps the
//! window open and the old frontend polls keep working.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::db;
use crate::playtime;
use crate::steam;

/// Unix seconds, for exit.log timestamps (no chrono dependency).
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Resolve the exit.log path (v0.2.17 — the watchdog thread gets
/// its path here, while the app is still healthy, so it never
/// touches Tauri state after the exit is requested).
pub fn exit_log_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("exit.log"))
}

/// Append one timestamped line to a log file by path, with no
/// Tauri calls at all (v0.2.17 — the watchdog's raw writer: the
/// v0.2.16 watchdog called append_exit_log through the AppHandle
/// and never fired in the VPS repro, so the watchdog now touches
/// nothing but the filesystem). Never panics, never fails the
/// caller: every error is swallowed. The file is truncated once it
/// passes 1 MB instead of growing forever.
pub fn append_raw_log(path: &std::path::Path, line: &str) {
    use std::io::Write;
    let text = format!("[{}] {line}\n", now_secs());
    if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) > 1_000_000 {
        let _ = std::fs::write(path, &text);
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = f.write_all(text.as_bytes());
    }
}

/// Append one timestamped line to exit.log in the app data dir
/// (v0.2.16 — diagnose the lingering process after quit: eprintln
/// goes nowhere in a GUI build on Windows, so exit events go to a
/// file the VPS test can read with Get-Content). Never fails the
/// caller: every error is swallowed.
pub fn append_exit_log(app: &AppHandle, line: &str) {
    let Some(path) = exit_log_path(app) else {
        return;
    };
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    append_raw_log(&path, line);
}

/// Whether a game is currently running (v0.2.11). Set when a launch
/// starts (managed path here, or the `start_playtime_session` command
/// for the window-stays-open path) and cleared when the game exits.
/// The run callback uses it to tell a real quit apart from the
/// window lifecycle: Alt+F4 / window close quits when NO game is
/// running, but while a game runs the v0.2.10 behaviour holds — the
/// exit is prevented, the ~4 MB host stays in the tray, and the
/// window is recreated on game exit.
static GAME_RUNNING: AtomicBool = AtomicBool::new(false);

/// Mark a game as running (launch) or not (game exit).
pub fn set_game_running(running: bool) {
    GAME_RUNNING.store(running, Ordering::SeqCst);
}

/// Whether a game is currently running.
pub fn is_game_running() -> bool {
    GAME_RUNNING.load(Ordering::SeqCst)
}

/// Which side of the v0.2.9 window lifecycle the launcher is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherWindow {
    Open,
    Destroyed,
}

/// "Minimize on launch" ON destroys the window on launch (the ~4 MB
/// host stays in the tray); OFF keeps the window open.
pub fn window_after_launch(minimize_on_launch: bool) -> LauncherWindow {
    if minimize_on_launch {
        LauncherWindow::Destroyed
    } else {
        LauncherWindow::Open
    }
}

/// How many times window recreation is retried before giving up (the
/// tray icon stays as the manual fallback after that).
pub const MAX_RECREATE_ATTEMPTS: u32 = 3;

/// Recreate (or show) the window when a game exits — but only if it
/// is actually gone, and only for a bounded number of tries.
pub fn should_recreate_window(window: LauncherWindow, attempts: u32) -> bool {
    window == LauncherWindow::Destroyed && attempts < MAX_RECREATE_ATTEMPTS
}

/// Show the launcher window if it exists, otherwise build a fresh one
/// with the same config as tauri.conf.json (borderless fullscreen
/// 1920x1080). Creation is retried up to `MAX_RECREATE_ATTEMPTS`
/// times; an existing window is simply shown and focused.
pub fn show_or_recreate_window(app: &AppHandle) -> Result<(), String> {
    let mut attempts = 0;
    loop {
        // The window may have appeared meanwhile (e.g. a tray click
        // while this retry loop was running) — then just show it.
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.show();
            let _ = w.set_focus();
            return Ok(());
        }
        attempts += 1;
        let built = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
            .title("Sonic Launcher")
            .fullscreen(true)
            .decorations(false)
            .resizable(true)
            .inner_size(1920.0, 1080.0)
            .additional_browser_args("--autoplay-policy=no-user-gesture-required")
            .build();
        match built {
            Ok(w) => {
                let _ = w.set_focus();
                return Ok(());
            }
            Err(e) => {
                if !should_recreate_window(LauncherWindow::Destroyed, attempts) {
                    return Err(format!(
                        "cannot recreate launcher window after {attempts} attempts: {e}"
                    ));
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

/// Destroy the launcher window if it exists. Never errors: a missing
/// window is already gone.
pub fn destroy_launcher_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.destroy();
    }
}

/// Whether a runtime `ExitRequested` event should be prevented.
///
/// v0.2.10 — the diagnosed cause of the v0.2.9 live failure (proven
/// from the tauri 2.12.7 / tauri-runtime-wry 2.12.5 sources in
/// Cargo.lock): the runtime's `Destroyed` handler emits
/// `ExitRequested { code: None }` and sets `ControlFlow::Exit`
/// whenever the last window is destroyed, unless the app prevents
/// it. A tray icon does NOT keep the event loop alive — the runtime
/// only checks the window list. Our app used the default
/// `.run(context)` with an empty callback, so nothing ever prevented
/// it: destroying the window on game launch ended the whole process
/// (no host left to recreate the window, no tray icon, no fail-soft
/// notice). `code: Some(_)` comes from an explicit
/// `AppHandle::exit` / `restart` (e.g. the tray "Quit" item) and
/// must be allowed through, or the app could never quit.
///
/// v0.2.11 — Alt+F4 / closing the window quits when NO game is
/// running (the window is borderless, so this is the only visible
/// way out besides the tray / Settings / Power menu). While a game
/// IS running the exit is still prevented: the host must stay alive
/// in the tray to recreate the window on game exit.
pub fn should_prevent_exit(exit_code: Option<i32>, game_running: bool) -> bool {
    exit_code.is_none() && game_running
}

/// Start a managed game session: start the playtime session, spawn
/// the exit watcher, then destroy the launcher window. The watcher
/// ends the session and recreates the window when the game exits.
///
/// `source` is the game's source (`steam` for Steam games, anything
/// else is an exe launch). Steam games are watched via `RunningAppID`,
/// exe games by polling for the exe's process name — the same
/// seen/checks>=5 rule the old frontend polls used (a game that never
/// appears within ~10s also restores the window).
pub fn begin_managed_launch(
    app: &AppHandle,
    source: &str,
    appid: Option<u32>,
    exe_path: Option<String>,
    game_key: &str,
    game_name: &str,
) -> Result<(), String> {
    let session_id = db::start_playtime_session(app, game_key, game_name).ok();
    let is_steam = source == "steam" && appid.is_some();
    let exe_name = exe_path
        .as_deref()
        .and_then(|p| Path::new(p).file_name())
        .map(|n| n.to_string_lossy().into_owned());
    let watcher_app = app.clone();
    std::thread::spawn(move || {
        let mut seen = false;
        let mut checks: u32 = 0;
        loop {
            checks += 1;
            std::thread::sleep(Duration::from_secs(2));
            let running = if is_steam {
                steam::running_appid().is_some()
            } else {
                match &exe_name {
                    Some(name) => playtime::is_process_running(name),
                    None => false,
                }
            };
            if running {
                seen = true;
            } else if seen || checks >= 5 {
                break;
            }
        }
        if let Some(id) = session_id {
            let _ = db::end_playtime_session(&watcher_app, id);
        }
        // The game is gone: clear the flag BEFORE recreating, so a
        // window close from here on is a real quit, not a lifecycle
        // exit. The window may already exist again (e.g. the user
        // clicked the tray icon mid-game) — then this just shows it.
        append_exit_log(
            &watcher_app,
            "GAME_RUNNING=false (watcher: game gone, recreating window)",
        );
        set_game_running(false);
        let _ = show_or_recreate_window(&watcher_app);
    });
    // The game is starting: the window destroy below must NOT end the
    // process — the run callback prevents the exit while this flag
    // is set (see should_prevent_exit).
    append_exit_log(
        app,
        &format!("GAME_RUNNING=true (begin_managed_launch {game_name})"),
    );
    set_game_running(true);
    destroy_launcher_window(app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_with_minimize_on_destroys_the_window() {
        assert_eq!(window_after_launch(true), LauncherWindow::Destroyed);
        assert_eq!(window_after_launch(false), LauncherWindow::Open);
    }

    #[test]
    fn recreate_only_when_destroyed_and_within_attempts() {
        assert!(should_recreate_window(LauncherWindow::Destroyed, 0));
        assert!(should_recreate_window(LauncherWindow::Destroyed, 2));
        assert!(!should_recreate_window(LauncherWindow::Destroyed, 3));
        assert!(!should_recreate_window(LauncherWindow::Destroyed, 4));
        assert!(!should_recreate_window(LauncherWindow::Open, 0));
    }

    #[test]
    fn exit_request_prevented_only_while_a_game_runs() {
        // code None + game running: the last window was destroyed by
        // the managed launch — the host must stay alive in the tray.
        assert!(should_prevent_exit(None, true));
        // code None + NO game running: Alt+F4 / window close — a real
        // quit (v0.2.11), the window is borderless so this is the
        // only visible way out besides tray / Settings / Power.
        assert!(!should_prevent_exit(None, false));
        // code Some: an explicit exit/restart (tray "Quit", the
        // Settings / Power "Exit Launcher") — the app must be allowed
        // to end, or it could never quit.
        assert!(!should_prevent_exit(Some(0), true));
        assert!(!should_prevent_exit(Some(0), false));
        assert!(!should_prevent_exit(Some(1), true));
    }
}
