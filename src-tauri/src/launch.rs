//! Launching games: Steam via `steam://rungameid/<appid>`, manual games by
//! spawning their .exe directly (detached — the launcher does not wait).

use std::path::Path;
use std::process::Command;

/// Launch a game.
///
/// Exactly one of `appid` (Steam) or `exe_path` (manual .exe) must be
/// given. Returns `Err` with a human-readable message on failure.
pub fn launch_game(appid: Option<u32>, exe_path: Option<String>) -> Result<(), String> {
    match (appid, exe_path) {
        (Some(id), _) => launch_steam(id),
        (None, Some(path)) => launch_exe(&path),
        (None, None) => Err("nothing to launch: no appid or exe path given".to_string()),
    }
}

fn launch_steam(appid: u32) -> Result<(), String> {
    let url = format!("steam://rungameid/{appid}");
    open::that(&url).map_err(|e| format!("cannot open {url}: {e}"))?;
    Ok(())
}

fn launch_exe(exe_path: &str) -> Result<(), String> {
    let path = exe_path.trim();
    if path.is_empty() {
        return Err("exe path is empty".to_string());
    }
    if !Path::new(path).is_file() {
        return Err(format!("exe not found: {path}"));
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
    cmd.spawn()
        .map_err(|e| format!("cannot launch {path}: {e}"))?;
    Ok(())
}
