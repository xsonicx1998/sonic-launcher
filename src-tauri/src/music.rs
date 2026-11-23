//! Theme music auto-download + playback support (Phase 3).
//!
//! Flow: yt-dlp.exe is downloaded once (first run) into
//! `%APPDATA%\com.sonic.launcher\bin\`. Theme search runs
//! `yt-dlp "ytsearch5:{game} theme" --dump-json --flat-playlist`; the
//! smart filter picks the first result that is 60–360s and whose title
//! has none of the excluded words. Downloads are audio-only in the
//! native container — NO ffmpeg is involved. Cached files live in
//! `%APPDATA%\com.sonic.launcher\music\{game_key}.{ext}` and are never
//! re-downloaded. Playback itself is a plain `<audio>` element in the
//! frontend (src via `convertFileSrc`, data-URI fallback).
//!
//! The user's game folders are NEVER scanned for music (locked plan).

use base64::Engine;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use tauri::{AppHandle, Manager};

use crate::db;
use crate::http;

const YTDLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";

/// Audio extensions we accept (cached downloads + manual picks).
const AUDIO_EXTS: &[&str] = &["m4a", "opus", "mp3", "ogg", "flac", "wav", "webm"];

/// Title words that disqualify a search result (case-insensitive
/// substring match — the locked plan's smart filter).
const EXCLUDED_WORDS: &[&str] = &[
    "cover", "remix", "piano", "1 hour", "1hr", "live", "extended", "loop", "8d", "slowed",
    "reverb",
];

/// One yt-dlp search result.
#[derive(Debug, Clone, Serialize)]
pub struct MusicCandidate {
    pub id: String,
    pub title: String,
    pub duration_secs: Option<u64>,
}

/// A cached/selected theme-music file, as exposed to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct MusicInfo {
    pub game_key: String,
    pub source: String,
    pub path: String,
    pub title: String,
}

/// The bin directory: `%APPDATA%\com.sonic.launcher\bin`.
pub fn bin_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("cannot resolve app data dir: {e}"))?
        .join("bin");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create bin dir: {e}"))?;
    Ok(dir)
}

/// The music cache directory: `%APPDATA%\com.sonic.launcher\music`.
pub fn music_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("cannot resolve app data dir: {e}"))?
        .join("music");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create music dir: {e}"))?;
    Ok(dir)
}

/// Full path of the bundled yt-dlp executable.
pub fn ytdlp_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(bin_dir(app)?.join("yt-dlp.exe"))
}

/// Find the cached music file for a game key, if one exists.
pub fn music_file_for_key(dir: &Path, game_key: &str) -> Option<PathBuf> {
    for ext in AUDIO_EXTS {
        let p = dir.join(format!("{game_key}.{ext}"));
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Smart filter: duration must be 60–360s and the title must not contain
/// any excluded word. Pure — unit-tested.
pub fn theme_candidate_ok(title: &str, duration_secs: Option<u64>) -> bool {
    let Some(d) = duration_secs else {
        return false;
    };
    if !(60..=360).contains(&d) {
        return false;
    }
    let lower = title.to_lowercase();
    !EXCLUDED_WORDS.iter().any(|bad| lower.contains(bad))
}

/// Pick the first search result passing the smart filter. Pure.
pub fn pick_theme_candidate(candidates: &[MusicCandidate]) -> Option<&MusicCandidate> {
    candidates
        .iter()
        .find(|c| theme_candidate_ok(&c.title, c.duration_secs))
}

/// Parse one `--dump-json` output line into a candidate. Pure.
pub fn parse_search_line(line: &str) -> Option<MusicCandidate> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let id = v.get("id")?.as_str()?.to_string();
    let title = v.get("title")?.as_str()?.to_string();
    let duration_secs = v.get("duration").and_then(|d| d.as_u64());
    Some(MusicCandidate {
        id,
        title,
        duration_secs,
    })
}

fn run_ytdlp(exe: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    Command::new(exe)
        .args(args)
        .output()
        .map_err(|e| format!("cannot run {}: {e}", exe.display()))
}

/// Download yt-dlp.exe on first run and verify it. Returns the version
/// string (e.g. `2025.01.01`). `Err` with a human-readable message when
/// the download or the version check fails.
pub fn ensure_ytdlp(app: &AppHandle) -> Result<String, String> {
    let exe = ytdlp_path(app)?;
    if !exe.is_file() {
        let bytes = http::download_bytes(YTDLP_URL)?;
        if let Some(parent) = exe.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("cannot create bin dir: {e}"))?;
        }
        std::fs::write(&exe, bytes).map_err(|e| format!("cannot save yt-dlp.exe: {e}"))?;
    }
    let out = run_ytdlp(&exe, &["--version"])?;
    if !out.status.success() {
        return Err(format!(
            "yt-dlp --version failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if version.is_empty() {
        return Err("yt-dlp --version returned no version".to_string());
    }
    Ok(version)
}

/// Search YouTube for a game's theme music. Returns the top 5 results
/// (title + duration), unfiltered — the frontend shows them in
/// "Change Theme Music", and the auto-pick applies the smart filter.
pub fn search_theme_music(app: &AppHandle, game_name: &str) -> Result<Vec<MusicCandidate>, String> {
    let exe = ytdlp_path(app)?;
    if !exe.is_file() {
        ensure_ytdlp(app)?;
    }
    let query = format!("ytsearch5:{game_name} theme");
    let out = run_ytdlp(
        &exe,
        &["--no-warnings", "--dump-json", "--flat-playlist", &query],
    )?;
    if !out.status.success() {
        return Err(format!(
            "theme search failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut out_candidates = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(c) = parse_search_line(line) {
            out_candidates.push(c);
        }
        if out_candidates.len() >= 5 {
            break;
        }
    }
    Ok(out_candidates)
}

/// Download a theme track (audio-only, native container, no ffmpeg) and
/// cache it as `music/{game_key}.{ext}`. Returns the cached path.
/// A cached file is returned immediately without re-downloading.
pub fn download_theme_music(
    app: &AppHandle,
    game_key: &str,
    video_id: &str,
) -> Result<String, String> {
    let dir = music_dir(app)?;
    if let Some(existing) = music_file_for_key(&dir, game_key) {
        return Ok(existing.to_string_lossy().into_owned());
    }
    let exe = ytdlp_path(app)?;
    if !exe.is_file() {
        ensure_ytdlp(app)?;
    }
    let url = format!("https://www.youtube.com/watch?v={video_id}");
    let template = dir.join(format!("{game_key}.%(ext)s"));
    let out = run_ytdlp(
        &exe,
        &[
            "--no-warnings",
            "--no-playlist",
            "-f",
            "bestaudio[ext=m4a]/bestaudio[ext=opus]/bestaudio",
            "-o",
            &template.to_string_lossy(),
            &url,
        ],
    )?;
    if !out.status.success() {
        return Err(format!(
            "theme download failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let Some(path) = music_file_for_key(&dir, game_key) else {
        return Err("download finished but no audio file appeared".to_string());
    };
    // Best-effort title lookup for the music table.
    let title = match run_ytdlp(
        &exe,
        &["--no-warnings", "--no-playlist", "--get-title", &url],
    ) {
        Ok(t) if t.status.success() => {
            let s = String::from_utf8_lossy(&t.stdout).trim().to_string();
            if s.is_empty() {
                video_id.to_string()
            } else {
                s
            }
        }
        _ => video_id.to_string(),
    };
    db::save_music(app, game_key, "youtube", &path.to_string_lossy(), &title)?;
    Ok(path.to_string_lossy().into_owned())
}

/// The cached theme music for a game, if a file still exists on disk.
pub fn get_game_music(app: &AppHandle, game_key: &str) -> Option<MusicInfo> {
    let row = db::get_music(app, game_key).ok()??;
    if !Path::new(&row.path).is_file() {
        return None;
    }
    Some(MusicInfo {
        game_key: row.game_key,
        source: row.source,
        path: row.path,
        title: row.title,
    })
}

/// Let the user pick an audio file for a game (dialog), copy it into the
/// music cache, and use it as that game's theme. Returns the cached path,
/// or `None` when the user cancelled the dialog.
pub fn pick_music_file(app: &AppHandle, game_key: &str) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let picked = app
        .dialog()
        .file()
        .add_filter("Audio", &["mp3", "m4a", "opus", "ogg", "flac", "wav"])
        .set_title("Choose theme music")
        .blocking_pick_file();
    let Some(file) = picked else {
        return Ok(None);
    };
    let Some(src) = file.as_path() else {
        return Err("picked file has no local path".to_string());
    };
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp3")
        .to_lowercase();
    let dir = music_dir(app)?;
    // Remove any previous cached file for this game first.
    if let Some(old) = music_file_for_key(&dir, game_key) {
        let _ = std::fs::remove_file(old);
    }
    let dest = dir.join(format!("{game_key}.{ext}"));
    std::fs::copy(src, &dest).map_err(|e| format!("cannot copy audio file: {e}"))?;
    let title = src
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Theme music")
        .to_string();
    db::save_music(app, game_key, "manual", &dest.to_string_lossy(), &title)?;
    Ok(Some(dest.to_string_lossy().into_owned()))
}

/// Base64 data URI of a game's cached music file (fallback when the
/// asset protocol cannot serve it). `None` when no file is cached.
pub fn get_music_data_uri(app: &AppHandle, game_key: &str) -> Result<Option<String>, String> {
    let Some(info) = get_game_music(app, game_key) else {
        return Ok(None);
    };
    let bytes = std::fs::read(&info.path).map_err(|e| format!("cannot read music file: {e}"))?;
    let mime = match Path::new(&info.path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("m4a")
    {
        "mp3" => "audio/mpeg",
        "ogg" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "webm" => "audio/webm",
        _ => "audio/mp4",
    };
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(Some(format!("data:{mime};base64,{b64}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_accepts_plain_theme_in_range() {
        assert!(theme_candidate_ok(
            "Astro's Playroom - Main Theme",
            Some(180)
        ));
        assert!(theme_candidate_ok("Doom Eternal Theme", Some(60)));
        assert!(theme_candidate_ok("Doom Eternal Theme", Some(360)));
    }

    #[test]
    fn filter_rejects_out_of_range_durations() {
        assert!(!theme_candidate_ok("Game Theme", Some(59)));
        assert!(!theme_candidate_ok("Game Theme", Some(361)));
        assert!(!theme_candidate_ok("Game Theme", None));
        assert!(!theme_candidate_ok("Game Theme", Some(0)));
    }

    #[test]
    fn filter_rejects_excluded_words_case_insensitively() {
        for bad in [
            "Game Theme (Piano Cover)",
            "Game Theme REMIX",
            "Game Theme piano version",
            "Game Theme 1 Hour",
            "Game Theme 1hr loop",
            "Game Theme LIVE",
            "Game Theme Extended",
            "Game Theme loop 10 hours",
            "Game Theme 8D Audio",
            "Game Theme slowed + reverb",
            "Game Theme Reverb",
        ] {
            assert!(
                !theme_candidate_ok(bad, Some(200)),
                "{bad} should be rejected"
            );
        }
    }

    #[test]
    fn pick_returns_first_passing_candidate() {
        let cands = vec![
            MusicCandidate {
                id: "a".into(),
                title: "Game Theme (cover)".into(),
                duration_secs: Some(120),
            },
            MusicCandidate {
                id: "b".into(),
                title: "Game Theme 1 Hour".into(),
                duration_secs: Some(120),
            },
            MusicCandidate {
                id: "c".into(),
                title: "Game Theme".into(),
                duration_secs: Some(200),
            },
        ];
        assert_eq!(pick_theme_candidate(&cands).unwrap().id, "c");
        let none: Vec<MusicCandidate> = vec![MusicCandidate {
            id: "x".into(),
            title: "Game Theme remix".into(),
            duration_secs: Some(100),
        }];
        assert!(pick_theme_candidate(&none).is_none());
    }

    #[test]
    fn parse_search_line_reads_flat_playlist_json() {
        let c = parse_search_line(
            r#"{"id":"dQw4w9WgXcQ","title":"Game Theme","duration":187,"ie_key":"Youtube"}"#,
        )
        .unwrap();
        assert_eq!(c.id, "dQw4w9WgXcQ");
        assert_eq!(c.duration_secs, Some(187));
        let no_dur = parse_search_line(r#"{"id":"abc","title":"No Duration"}"#).unwrap();
        assert_eq!(no_dur.duration_secs, None);
        assert!(parse_search_line("not json").is_none());
        assert!(parse_search_line(r#"{"title":"missing id"}"#).is_none());
    }

    #[test]
    fn music_file_for_key_finds_any_audio_ext() {
        let dir = std::env::temp_dir().join("p3-verify-music");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(music_file_for_key(&dir, "steam-440").is_none());
        std::fs::write(dir.join("steam-440.opus"), b"x").unwrap();
        assert_eq!(
            music_file_for_key(&dir, "steam-440"),
            Some(dir.join("steam-440.opus"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
