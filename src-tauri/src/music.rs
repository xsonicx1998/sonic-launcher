//! Theme music auto-download + playback support (Phase 3).
//!
//! Flow: yt-dlp.exe is downloaded once (first run) into
//! `%APPDATA%\com.sonic.launcher\bin\`. Theme search runs
//! `yt-dlp "ytsearch5:{game} theme" --dump-json --flat-playlist`; the
//! smart filter picks the first result that is 45–480s and whose title
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
const YTDLP_SUMS_URL: &str =
    "https://github.com/yt-dlp/yt-dlp/releases/latest/download/SHA2-256SUMS";

/// SHA-256 hex digest of `bytes` (integrity check for the yt-dlp binary).
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Parse an official `SHA2-256SUMS` file and return the expected hex digest
/// for `filename` (lines look like `<sha256>  <filename>`).
fn parse_sha256sums(sums: &str, filename: &str) -> Option<String> {
    for line in sums.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?;
        let name = name.trim_start_matches('*');
        if name == filename && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(hash.to_lowercase());
        }
    }
    None
}

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
    /// Direct page URL (YouTube watch link or SoundCloud track link).
    pub url: String,
    /// "youtube" or "soundcloud".
    pub source: String,
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

/// Smart filter: duration must be 45–480s and the title must not contain
/// any excluded word. Pure — unit-tested.
pub fn theme_candidate_ok(title: &str, duration_secs: Option<u64>) -> bool {
    let Some(d) = duration_secs else {
        return false;
    };
    if !(45..=480).contains(&d) {
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
    let url = v
        .get("webpage_url")
        .and_then(|u| u.as_str())
        .or_else(|| v.get("url").and_then(|u| u.as_str()))
        .unwrap_or("")
        .to_string();
    // yt-dlp flat-playlist JSON carries the extractor name.
    let extractor = v
        .get("extractor")
        .and_then(|e| e.as_str())
        .unwrap_or("youtube")
        .to_lowercase();
    let source = if extractor.contains("soundcloud") {
        "soundcloud".to_string()
    } else {
        "youtube".to_string()
    };
    Some(MusicCandidate {
        id,
        title,
        duration_secs,
        url,
        source,
    })
}

/// Clean a game title for search: strip trademark symbols and edition
/// suffixes that break the query (e.g. "™", "Director's Cut",
/// "Definitive Edition", "GOTY").
pub fn sanitize_game_title(title: &str) -> String {
    let mut s: String = title
        .chars()
        .filter(|c| !matches!(c, '™' | '®' | '©' | '℠'))
        .collect();
    const SUFFIXES: &[&str] = &[
        "game of the year edition",
        "definitive edition",
        "director's cut",
        "directors cut",
        "enhanced edition",
        "special edition",
        "complete edition",
        "remastered",
        "remaster",
        "goty",
    ];
    let mut lower = s.to_lowercase();
    for suf in SUFFIXES {
        // Remove the suffix wherever it appears (usually at the end).
        while let Some(pos) = lower.find(suf) {
            s.replace_range(pos..pos + suf.len(), "");
            lower = s.to_lowercase();
        }
    }
    // Collapse leftover double spaces / trailing separators.
    let mut out = String::new();
    let mut last_space = true;
    for c in s.chars() {
        if c.is_whitespace() {
            if !last_space {
                out.push(' ');
            }
            last_space = true;
        } else if c == '-' || c == ':' || c == '—' || c == '–' {
            if !last_space {
                out.push(' ');
            }
            last_space = true;
        } else {
            out.push(c);
            last_space = false;
        }
    }
    out.trim().to_string()
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
        // Integrity: verify the binary against the official SHA2-256SUMS
        // from the same release before saving or running it.
        let sums = http::download_bytes(YTDLP_SUMS_URL)?;
        let sums = String::from_utf8_lossy(&sums);
        let expected = parse_sha256sums(&sums, "yt-dlp.exe")
            .ok_or_else(|| "SHA2-256SUMS has no entry for yt-dlp.exe".to_string())?;
        let actual = sha256_hex(&bytes);
        if actual != expected {
            return Err("yt-dlp.exe SHA-256 mismatch — refusing to save it".to_string());
        }
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

/// Search for a game's theme music. Returns up to 5 results
/// (title + duration + url), smart-filtered: only candidates that are
/// 45–480s with no excluded words are returned, recommended pick first
/// — so "Change Theme Music" lists only viable candidates and the
/// auto-pick agrees with the manual list.
/// `fallback` switches the YouTube query to `soundtrack OST`;
/// `source` "soundcloud" searches SoundCloud instead of YouTube.
/// The game title is sanitized first (trademarks / edition suffixes).
pub fn search_theme_music(
    app: &AppHandle,
    game_name: &str,
    fallback: bool,
    source: &str,
) -> Result<Vec<MusicCandidate>, String> {
    let exe = ytdlp_path(app)?;
    if !exe.is_file() {
        ensure_ytdlp(app)?;
    }
    let clean = sanitize_game_title(game_name);
    let clean = if clean.is_empty() {
        game_name.trim().to_string()
    } else {
        clean
    };
    let query = if source.eq_ignore_ascii_case("soundcloud") {
        format!("scsearch5:{clean} theme")
    } else if fallback {
        format!("ytsearch5:{clean} soundtrack OST")
    } else {
        format!("ytsearch5:{clean} theme")
    };
    // Hardened flags: ignore user config, no playlists, 25 MB cap,
    // https only, and `--` so the query can never be parsed as a flag
    // (blocks `--exec` / flag injection and `file://` redirects).
    let out = run_ytdlp(
        &exe,
        &[
            "--ignore-config",
            "--no-warnings",
            "--no-playlist",
            "--max-filesize",
            "25M",
            "--proto",
            "https",
            "--dump-json",
            "--flat-playlist",
            "--",
            &query,
        ],
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
            // Smart filter: only viable theme candidates are returned.
            if theme_candidate_ok(&c.title, c.duration_secs) {
                out_candidates.push(c);
            }
        }
        if out_candidates.len() >= 5 {
            break;
        }
    }
    // The recommended pick leads the list, so "Change Theme Music" and
    // the auto-pick agree on the best candidate (the first viable
    // result, by construction).
    if let Some(best) = pick_theme_candidate(&out_candidates) {
        let best = best.clone();
        out_candidates.retain(|c| c.id != best.id);
        out_candidates.insert(0, best);
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
    if video_id.is_empty()
        || video_id
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    {
        return Err("invalid video id".to_string());
    }
    let url = format!("https://www.youtube.com/watch?v={video_id}");
    download_audio_from_url(app, game_key, &url, "youtube", video_id)
}

/// Download a theme track from any direct URL (SoundCloud track, a pasted
/// YouTube/SoundCloud/Bandcamp link, ...). Same cache + rules as the
/// YouTube download. `source` is stored in the music table.
pub fn download_theme_music_from_url(
    app: &AppHandle,
    game_key: &str,
    url: &str,
    source: &str,
) -> Result<String, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("music URL is empty".to_string());
    }
    // Strict: https only, and no whitespace or newlines anywhere (a
    // newline could smuggle a second flag/URL into the yt-dlp call).
    if !url.starts_with("https://") {
        return Err("music URL must start with https://".to_string());
    }
    if url.chars().any(|c| c.is_whitespace()) {
        return Err("music URL must not contain whitespace".to_string());
    }
    let fallback_title = url.to_string();
    download_audio_from_url(app, game_key, url, source, &fallback_title)
}

fn download_audio_from_url(
    app: &AppHandle,
    game_key: &str,
    url: &str,
    source: &str,
    fallback_title: &str,
) -> Result<String, String> {
    let dir = music_dir(app)?;
    if let Some(existing) = music_file_for_key(&dir, game_key) {
        return Ok(existing.to_string_lossy().into_owned());
    }
    let exe = ytdlp_path(app)?;
    if !exe.is_file() {
        ensure_ytdlp(app)?;
    }
    let template = dir.join(format!("{game_key}.%(ext)s"));
    // Native audio only (no ffmpeg): m4a / webm / opus / mp3 / ogg.
    // `--` keeps the URL from ever being parsed as a flag.
    let out = run_ytdlp(
        &exe,
        &[
            "--ignore-config",
            "--no-warnings",
            "--no-playlist",
            "--max-filesize",
            "25M",
            "--proto",
            "https",
            "-f",
            "bestaudio[ext=m4a]/bestaudio[ext=webm]/bestaudio[ext=opus]/bestaudio[ext=mp3]/bestaudio[ext=ogg]",
            "-o",
            &template.to_string_lossy(),
            "--",
            url,
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
    // Integrity: only a native audio container may land in the cache.
    const AUDIO_EXTS: &[&str] = &["m4a", "opus", "webm", "ogg", "mp3"];
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if !AUDIO_EXTS.contains(&ext.as_str()) {
        let _ = std::fs::remove_file(&path);
        return Err(format!("downloaded file has a disallowed extension: {ext}"));
    }
    // Best-effort title lookup for the music table.
    let title = match run_ytdlp(
        &exe,
        &[
            "--ignore-config",
            "--no-warnings",
            "--no-playlist",
            "--max-filesize",
            "25M",
            "--proto",
            "https",
            "--get-title",
            "--",
            url,
        ],
    ) {
        Ok(t) if t.status.success() => {
            let s = String::from_utf8_lossy(&t.stdout).trim().to_string();
            if s.is_empty() {
                fallback_title.to_string()
            } else {
                s
            }
        }
        _ => fallback_title.to_string(),
    };
    db::save_music(app, game_key, source, &path.to_string_lossy(), &title)?;
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

/// Pick an ambient audio file (stored in settings, played for games with
/// no theme music). Returns the path, or `None` when cancelled.
pub fn pick_ambient_file(app: &AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let picked = app
        .dialog()
        .file()
        .add_filter("Audio", &["mp3", "m4a", "opus", "ogg", "flac", "wav"])
        .set_title("Choose ambient music")
        .blocking_pick_file();
    let Some(file) = picked else {
        return Ok(None);
    };
    let Some(path) = file.as_path() else {
        return Err("picked file has no local path".to_string());
    };
    Ok(Some(path.to_string_lossy().into_owned()))
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
        assert!(theme_candidate_ok("Doom Eternal Theme", Some(45)));
        assert!(theme_candidate_ok("Doom Eternal Theme", Some(480)));
    }

    #[test]
    fn filter_rejects_out_of_range_durations() {
        assert!(!theme_candidate_ok("Game Theme", Some(44)));
        assert!(!theme_candidate_ok("Game Theme", Some(481)));
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
            r#"{"id":"dQw4w9WgXcQ","title":"Game Theme","duration":187,"ie_key":"Youtube","webpage_url":"https://www.youtube.com/watch?v=dQw4w9WgXcQ","extractor":"youtube"}"#,
        )
        .unwrap();
        assert_eq!(c.id, "dQw4w9WgXcQ");
        assert_eq!(c.duration_secs, Some(187));
        assert_eq!(c.url, "https://www.youtube.com/watch?v=dQw4w9WgXcQ");
        assert_eq!(c.source, "youtube");
        let sc = parse_search_line(
            r#"{"id":"12345","title":"Indie Theme","duration":200,"webpage_url":"https://soundcloud.com/artist/indie-theme","extractor":"soundcloud"}"#,
        )
        .unwrap();
        assert_eq!(sc.source, "soundcloud");
        assert!(sc.url.contains("soundcloud.com"));
        let no_dur = parse_search_line(r#"{"id":"abc","title":"No Duration"}"#).unwrap();
        assert_eq!(no_dur.duration_secs, None);
        assert!(parse_search_line("not json").is_none());
        assert!(parse_search_line(r#"{"title":"missing id"}"#).is_none());
    }

    #[test]
    fn sanitize_game_title_strips_symbols_and_editions() {
        assert_eq!(sanitize_game_title("Hades™"), "Hades");
        assert_eq!(sanitize_game_title("Cyberpunk 2077®"), "Cyberpunk 2077");
        assert_eq!(
            sanitize_game_title("The Witcher 3: Wild Hunt — Game of the Year Edition"),
            "The Witcher 3 Wild Hunt"
        );
        assert_eq!(
            sanitize_game_title("Doom Eternal - Definitive Edition"),
            "Doom Eternal"
        );
        assert_eq!(
            sanitize_game_title("Death Stranding Director's Cut"),
            "Death Stranding"
        );
        assert_eq!(sanitize_game_title("Elden Ring GOTY"), "Elden Ring");
        assert_eq!(
            sanitize_game_title("Hollow Knight Remastered"),
            "Hollow Knight"
        );
        assert_eq!(sanitize_game_title("Astro's Playroom"), "Astro's Playroom");
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
