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

use serde::Serialize;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/// Payload of the `launch-fast-exit` event.
#[derive(Debug, Clone, Serialize)]
pub struct FastExit {
    pub game_name: Option<String>,
    pub game_key: Option<String>,
    pub exe_path: String,
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
            Ok(Some(status)) if !status.success() => {
                let _ = app_handle.emit(
                    "launch-fast-exit",
                    FastExit {
                        game_name,
                        game_key,
                        exe_path: exe_owned,
                    },
                );
            }
            _ => {}
        }
    });
    Ok(())
}
