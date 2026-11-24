//! Managed game sessions (v0.2.9): destroy the window on launch.
//!
//! When the "Minimize launcher when a game starts" setting is ON, the
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
//! (`minimize_on_launch`) and its label ("Minimize launcher when a
//! game starts"). ON now means "the launcher window closes entirely
//! while a game runs" instead of "hides": from the user's point of
//! view the window still disappears when a game starts and comes back
//! when it exits — it is just lighter while gone. OFF keeps the
//! window open and the old frontend polls keep working.

use std::path::Path;
use std::time::Duration;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::db;
use crate::playtime;
use crate::steam;

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
        // The window may already exist again (e.g. the user clicked
        // the tray icon mid-game) — then this just shows it.
        let _ = show_or_recreate_window(&watcher_app);
    });
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
}
