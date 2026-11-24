//! Launching games: Steam via `steam://rungameid/<appid>`, manual games by
//! spawning their .exe directly (detached — the launcher does not wait).
//!
//! Fail-soft (exe-guarantee round, v0.2.3): an exe launch is watched
//! for 3 seconds. If the process exits that fast with a NON-ZERO code,
//! the frontend gets a `launch-fast-exit` event suggesting the exe
//! might be the wrong file ("This might not be the right file. Try
//! another?"). Suggest-only: nothing is blocked or reverted, because
//! some legit exes are handoff stubs that exit fast by design (and a
//! fast exit with code 0 is never reported at all).

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

use crate::db;

/// Settings key holding a persisted fast-exit notice (JSON `FastExit`).
/// The live `launch-fast-exit` event can be lost while the launcher
/// window is hidden or destroyed, so the backend also records the
/// notice here; the frontend consumes it on the next (re)start or
/// restore (see `take_pending_fast_exit`).
const PENDING_FAST_EXIT_KEY: &str = "pending_fast_exit";

/// A fast-exit notice stays fresh for 15 minutes. Older notices are
/// dropped so a stale one never pops up on a later startup.
const PENDING_FAST_EXIT_TTL_SECS: u64 = 900;

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Fail-soft decision: report the launch only when the process
/// actually exited (not still running) with a non-zero code. A fast
/// exit with code 0 is never reported (handoff stubs exit 0 by
/// design), and a still-running process is not a failure.
pub fn should_report_fast_exit(exited: bool, success: bool) -> bool {
    exited && !success
}

/// A persisted fast-exit notice is fresh for 15 minutes. A timestamp
/// in the future (clock skew) counts as fresh.
pub fn pending_fast_exit_is_fresh(created_at_unix: u64, now_unix: u64) -> bool {
    created_at_unix.saturating_add(PENDING_FAST_EXIT_TTL_SECS) >= now_unix
}

/// Payload of the `launch-fast-exit` event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FastExit {
    pub game_name: Option<String>,
    pub game_key: Option<String>,
    pub exe_path: String,
    /// Unix seconds when the fast exit was observed (freshness check).
    pub created_at_unix: u64,
}

/// Record a fast-exit notice for the frontend to pick up on the next
/// (re)start or restore. Overwrites any earlier unclaimed notice.
pub fn record_pending_fast_exit(app: &AppHandle, payload: &FastExit) {
    if let Ok(json) = serde_json::to_string(payload) {
        let _ = db::set_setting(app, PENDING_FAST_EXIT_KEY, &json);
    }
}

/// Take (read + delete) the pending fast-exit notice, if there is a
/// fresh one. Consume-on-read: the first check shows it, later checks
/// get nothing, and a stale notice is dropped silently.
pub fn take_pending_fast_exit(app: &AppHandle) -> Option<FastExit> {
    let raw = db::get_setting(app, PENDING_FAST_EXIT_KEY).ok()??;
    let _ = db::clear_setting(app, PENDING_FAST_EXIT_KEY);
    let payload: FastExit = serde_json::from_str(&raw).ok()?;
    if pending_fast_exit_is_fresh(payload.created_at_unix, now_unix()) {
        Some(payload)
    } else {
        None
    }
}

/// Launch a game.
///
/// Exactly one of `appid` (Steam) or `exe_path` (manual .exe) must be
/// given. Returns `Err` with a human-readable message on failure.
pub fn launch_game(
    app: &AppHandle,
    appid: Option<u32>,
    exe_path: Option<String>,
    game_name: Option<String>,
    game_key: Option<String>,
) -> Result<(), String> {
    match (appid, exe_path) {
        (Some(id), _) => launch_steam(id),
        (None, Some(path)) => launch_exe(app, &path, game_name, game_key),
        (None, None) => Err("nothing to launch: no appid or exe path given".to_string()),
    }
}

fn launch_steam(appid: u32) -> Result<(), String> {
    let url = format!("steam://rungameid/{appid}");
    open::that(&url).map_err(|e| format!("cannot open {url}: {e}"))?;
    Ok(())
}

fn launch_exe(
    app: &AppHandle,
    exe_path: &str,
    game_name: Option<String>,
    game_key: Option<String>,
) -> Result<(), String> {
    let path = exe_path.trim();
    if path.is_empty() {
        return Err("exe path is empty".to_string());
    }
    if !Path::new(path).is_file() {
        return Err(format!("exe not found: {path}"));
    }
    // Only a real `.exe` may be launched (never a script, shortcut, or
    // a path that merely happens to exist).
    if !path.to_lowercase().ends_with(".exe") {
        return Err(format!("not an .exe: {path}"));
    }
    // Detached spawn: the child keeps running after the launcher exits,
    // and the launcher never blocks on it.
    let mut cmd = Command::new(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS (0x8) + CREATE_NEW_PROCESS_GROUP (0x200):
        // the game survives the launcher and gets its own console group.
        cmd.creation_flags(0x8 | 0x200);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot launch {path}: {e}"))?;
    // Fail-soft watch: keep the child handle in a background thread.
    // A fast non-zero exit means the exe probably was not the game
    // (wrong file, missing DLL, anti-cheat stub that bailed). The
    // frontend only suggests trying another exe — it never blocks.
    let app_handle = app.clone();
    let exe_owned = path.to_string();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        match child.try_wait() {
            Ok(Some(status)) if should_report_fast_exit(true, status.success()) => {
                let payload = FastExit {
                    game_name,
                    game_key,
                    exe_path: exe_owned,
                    created_at_unix: now_unix(),
                };
                // Live event (fast path — only reaches a window that is
                // open right now) AND the persisted notice (reliable
                // path — the frontend picks it up on the next (re)start
                // or restore, even if this window is hidden or gone).
                let _ = app_handle.emit("launch-fast-exit", payload.clone());
                record_pending_fast_exit(&app_handle, &payload);
            }
            _ => {}
        }
    });
    Ok(())
}
