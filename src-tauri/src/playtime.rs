//! Playtime tracking (Phase 5).
//!
//! The frontend starts a session when a game launches and ends it when
//! the game exits (Steam: `RunningAppID` polling; exe games: process
//! polling via `is_process_running`). Sessions are stored in the
//! `playtime` table (see db.rs); totals are the sum of finished
//! sessions plus any still-open session's elapsed time.

use tauri::AppHandle;

use crate::db;

/// Start a play session for a game. Returns the session row id.
pub fn start_session(app: &AppHandle, game_key: &str, game_name: &str) -> Result<i64, String> {
    db::start_playtime_session(app, game_key, game_name)
}

/// End a play session (fills in `ended_at` and `minutes`).
pub fn end_session(app: &AppHandle, session_id: i64) -> Result<(), String> {
    db::end_playtime_session(app, session_id)
}

/// Close play sessions left open by a previous run (tray quit or crash
/// mid-game). Stale sessions credit zero minutes — see
/// db::close_stale_playtime_sessions. Returns how many were closed.
pub fn close_stale_sessions(app: &AppHandle) -> Result<i64, String> {
    db::close_stale_playtime_sessions(app)
}

/// Total tracked minutes for one game (finished sessions + open session
/// elapsed time so far).
pub fn get_playtime(app: &AppHandle, game_key: &str) -> Result<i64, String> {
    db::playtime_total_minutes(app, game_key)
}

/// Unix time of the last finished play session for a game, or 0 when
/// never played (v0.2.11 — the hero "Last played ..." line only shows
/// when this is non-zero).
pub fn get_last_played(app: &AppHandle, game_key: &str) -> i64 {
    db::last_played_unix(app, game_key).unwrap_or(0)
}

/// Per-game playtime totals, most-played first.
pub fn list_playtime(app: &AppHandle) -> Result<Vec<db::PlaytimeTotal>, String> {
    db::list_playtime_totals(app)
}

/// Whether a process with this exe file name is currently running
/// (e.g. `Wow.exe`). Used to detect when a non-Steam game exits.
/// Case-insensitive on Windows; sysinfo matches by process name.
pub fn is_process_running(exe_name: &str) -> bool {
    let exe_name = exe_name.trim();
    if exe_name.is_empty() {
        return false;
    }
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    let target = exe_name.to_lowercase();
    sys.processes().values().any(|p| {
        let name = p.name().to_string_lossy().to_lowercase();
        name == target
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_process_running_handles_empty_names() {
        assert!(!is_process_running(""));
        assert!(!is_process_running("   "));
        assert!(!is_process_running("definitely-not-a-real-process-xyz.exe"));
    }
}
