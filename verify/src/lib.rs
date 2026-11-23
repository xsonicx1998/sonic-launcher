//! Verify crate: the pure (Tauri-free) logic of Phases 2-5,
//! extracted verbatim from src-tauri/src/*.rs, plus SQLite
//! integration tests for the db.rs schema/queries.
//! The Tauri crate cannot compile on this VM (no gdk-3.0);
//! the in-source #[cfg(test)] modules run on the server.

pub mod http_pure {
    pub fn url_encode(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(b as char)
                }
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }
    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn url_encode_keeps_unreserved_and_escapes_spaces() {
            assert_eq!(url_encode("Team Fortress 2"), "Team%20Fortress%202");
            assert_eq!(url_encode("abc-_.~123"), "abc-_.~123");
            assert_eq!(url_encode("Café"), "Caf%C3%A9");
            assert_eq!(url_encode(""), "");
        }
    }
}

pub mod art_pure {
    pub fn steam_cdn_urls(appid: u32) -> (String, String, String) {
        (
            format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/header.jpg"),
            format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/library_hero.jpg"),
            format!(
                "https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/library_600x900.jpg"
            ),
        )
    }
    /// JPEG (FF D8 FF), PNG (89 50 4E 47 0D 0A 1A 0A), WebP (RIFF xxxx WEBP).
    /// Mirror of `art.rs::is_image_bytes` — the download validator.
    pub fn is_image_bytes(b: &[u8]) -> bool {
        if b.len() >= 3 && b[0] == 0xFF && b[1] == 0xD8 && b[2] == 0xFF {
            return true; // JPEG
        }
        if b.len() >= 8 && b[..8] == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] {
            return true; // PNG
        }
        if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
            return true; // WebP
        }
        false
    }

    pub fn parse_sgdb_search(json: &serde_json::Value) -> Vec<(u64, String)> {
        let mut out = Vec::new();
        if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
            for item in data {
                if let (Some(id), Some(name)) = (
                    item.get("id").and_then(|i| i.as_u64()),
                    item.get("name").and_then(|n| n.as_str()),
                ) {
                    out.push((id, name.to_string()));
                }
            }
        }
        out
    }
    pub fn parse_sgdb_image_urls(json: &serde_json::Value) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
            for item in data {
                if let Some(url) = item.get("url").and_then(|u| u.as_str()) {
                    if !url.is_empty() {
                        out.push(url.to_string());
                    }
                }
            }
        }
        out
    }
    fn file_name(game_key: &str, kind: &str) -> String {
        // game_key is `steam-<appid>` / `manual-<id>` / `epic-<slug>` etc. —
        // safe for a file name (letters, digits, dashes).
        let safe: String = game_key
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        format!("{safe}-{kind}.jpg")
    }
    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn steam_cdn_urls_follow_the_known_pattern() {
            let (h, hero, c) = steam_cdn_urls(440);
            assert_eq!(
                h,
                "https://cdn.cloudflare.steamstatic.com/steam/apps/440/header.jpg"
            );
            assert_eq!(
                hero,
                "https://cdn.cloudflare.steamstatic.com/steam/apps/440/library_hero.jpg"
            );
            assert_eq!(
                c,
                "https://cdn.cloudflare.steamstatic.com/steam/apps/440/library_600x900.jpg"
            );
        }

        #[test]
        fn is_image_bytes_accepts_jpeg_png_webp_and_rejects_html() {
            assert!(is_image_bytes(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10]));
            assert!(is_image_bytes(&[
                0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00
            ]));
            assert!(is_image_bytes(b"RIFF\x00\x00\x00\x00WEBPVP8 "));
            assert!(!is_image_bytes(b"<!DOCTYPE html>"));
            assert!(!is_image_bytes(b""));
            assert!(!is_image_bytes(b"RIFF\x00\x00\x00\x00XXXX"));
        }

        #[test]
        fn parse_sgdb_search_reads_ids_and_names() {
            let json: serde_json::Value = serde_json::from_str(
            r#"{"success":true,"data":[{"id":6641,"name":"Team Fortress 2","types":["steam"],"verified":true},{"id":12,"name":"Team Fortress Classic","types":["steam"],"verified":false}]}"#,
        )
        .unwrap();
            let out = parse_sgdb_search(&json);
            assert_eq!(out.len(), 2);
            assert_eq!(out[0], (6641, "Team Fortress 2".to_string()));
            assert_eq!(out[1], (12, "Team Fortress Classic".to_string()));
            assert!(parse_sgdb_search(&serde_json::json!({"success": false})).is_empty());
            assert!(parse_sgdb_search(&serde_json::json!({"data": []})).is_empty());
        }

        #[test]
        fn parse_sgdb_image_urls_reads_url_fields() {
            let json: serde_json::Value = serde_json::from_str(
            r#"{"success":true,"data":[{"id":1,"url":"https://cdn2.steamgriddb.com/grid/a.jpg"},{"id":2,"url":"https://cdn2.steamgriddb.com/grid/b.png"}]}"#,
        )
        .unwrap();
            let out = parse_sgdb_image_urls(&json);
            assert_eq!(out.len(), 2);
            assert_eq!(out[0], "https://cdn2.steamgriddb.com/grid/a.jpg");
            assert!(parse_sgdb_image_urls(&serde_json::json!({"data": [{"id": 1}]})).is_empty());
        }

        #[test]
        fn file_name_sanitizes_game_keys() {
            assert_eq!(file_name("steam-440", "hero"), "steam-440-hero.jpg");
            assert_eq!(
                file_name("weird key/with:chars", "header"),
                "weird_key_with_chars-header.jpg"
            );
        }
    }
}

pub mod music_pure {
    use serde::Serialize;
    use std::path::{Path, PathBuf};
    const AUDIO_EXTS: &[&str] = &["m4a", "opus", "mp3", "ogg", "flac", "wav", "webm"];
    const EXCLUDED_WORDS: &[&str] = &[
        "cover", "remix", "piano", "1 hour", "1hr", "live", "extended", "loop", "8d", "slowed",
        "reverb",
    ];
    #[derive(Debug, Clone, Serialize)]
    pub struct MusicCandidate {
        pub id: String,
        pub title: String,
        pub duration_secs: Option<u64>,
        pub url: String,
        pub source: String,
    }
    pub fn music_file_for_key(dir: &Path, game_key: &str) -> Option<PathBuf> {
        for ext in AUDIO_EXTS {
            let p = dir.join(format!("{game_key}.{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
        None
    }
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
    pub fn pick_theme_candidate(candidates: &[MusicCandidate]) -> Option<&MusicCandidate> {
        candidates
            .iter()
            .find(|c| theme_candidate_ok(&c.title, c.duration_secs))
    }
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

    /// "Paste Music URL" validation: `https://` only, and no whitespace
    /// or newlines anywhere (a newline could smuggle a second flag/URL
    /// into the yt-dlp call). Mirror of `music.rs`.
    pub fn is_valid_music_url(url: &str) -> bool {
        let url = url.trim();
        if url.is_empty() || !url.starts_with("https://") {
            return false;
        }
        !url.chars().any(|c| c.is_whitespace())
    }

    /// YouTube video IDs are `[A-Za-z0-9_-]` (11 chars). Anything else is
    /// rejected before it is ever turned into a URL.
    pub fn is_valid_video_id(id: &str) -> bool {
        !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    }

    /// Parse an official `SHA2-256SUMS` file and return the expected hex
    /// digest for `filename`. Mirror of `music.rs` (yt-dlp integrity).
    pub fn parse_sha256sums(sums: &str, filename: &str) -> Option<String> {
        for line in sums.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let hash = parts.next()?;
            let name = parts.next()?;
            let name = name.trim_start_matches('*');
            if name == filename
                && hash.len() == 64
                && hash.chars().all(|c| c.is_ascii_hexdigit())
            {
                return Some(hash.to_lowercase());
            }
        }
        None
    }

    /// Only a native audio container may land in the music cache.
    pub fn is_allowed_audio_ext(ext: &str) -> bool {
        matches!(
            ext.to_lowercase().as_str(),
            "m4a" | "opus" | "webm" | "ogg" | "mp3"
        )
    }

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
            while let Some(pos) = lower.find(suf) {
                s.replace_range(pos..pos + suf.len(), "");
                lower = s.to_lowercase();
            }
        }
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
                    url: "".into(),
                    source: "youtube".into(),
                },
                MusicCandidate {
                    id: "b".into(),
                    title: "Game Theme 1 Hour".into(),
                    duration_secs: Some(120),
                    url: "".into(),
                    source: "youtube".into(),
                },
                MusicCandidate {
                    id: "c".into(),
                    title: "Game Theme".into(),
                    duration_secs: Some(200),
                    url: "".into(),
                    source: "youtube".into(),
                },
            ];
            assert_eq!(pick_theme_candidate(&cands).unwrap().id, "c");
            let none: Vec<MusicCandidate> = vec![MusicCandidate {
                id: "x".into(),
                title: "Game Theme remix".into(),
                duration_secs: Some(100),
                url: "".into(),
                source: "youtube".into(),
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
            let no_dur = parse_search_line(r#"{"id":"abc","title":"No Duration"}"#).unwrap();
            assert_eq!(no_dur.duration_secs, None);
            assert!(parse_search_line("not json").is_none());
            assert!(parse_search_line(r#"{"title":"missing id"}"#).is_none());
        }

        #[test]
        fn music_url_must_be_https_without_whitespace() {
            assert!(is_valid_music_url("https://www.youtube.com/watch?v=abc"));
            assert!(is_valid_music_url("https://soundcloud.com/a/b"));
            assert!(!is_valid_music_url("http://example.com/a.mp3"));
            assert!(!is_valid_music_url("file:///etc/passwd"));
            assert!(!is_valid_music_url(""));
            assert!(!is_valid_music_url("https://example.com/a b.mp3"));
            assert!(!is_valid_music_url("https://example.com/a.mp3\n--exec"));
            assert!(!is_valid_music_url("javascript:alert(1)"));
        }

        #[test]
        fn video_id_allows_only_youtube_id_chars() {
            assert!(is_valid_video_id("dQw4w9WgXcQ"));
            assert!(is_valid_video_id("abc-123_XYZ"));
            assert!(!is_valid_video_id(""));
            assert!(!is_valid_video_id("abc def"));
            assert!(!is_valid_video_id("abc;rm"));
            assert!(!is_valid_video_id("https://youtube.com/watch?v=abc"));
        }

        #[test]
        fn parse_sha256sums_finds_the_binary_entry() {
            let sums = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  yt-dlp.exe\nabc123  other.bin\n";
            assert_eq!(
                parse_sha256sums(sums, "yt-dlp.exe"),
                Some(
                    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                        .to_string()
                )
            );
            assert_eq!(parse_sha256sums(sums, "missing.exe"), None);
            assert_eq!(parse_sha256sums("", "yt-dlp.exe"), None);
        }

        #[test]
        fn audio_ext_allowlist_rejects_non_audio() {
            for ext in ["m4a", "opus", "webm", "ogg", "mp3", "MP3", "Opus"] {
                assert!(is_allowed_audio_ext(ext), "{ext}");
            }
            for ext in ["exe", "bat", "html", "zip", "mp4", "mkv", ""] {
                assert!(!is_allowed_audio_ext(ext), "{ext}");
            }
        }

        #[test]
        fn sanitize_game_title_strips_symbols_and_editions() {
            assert_eq!(sanitize_game_title("Hades™"), "Hades");
            assert_eq!(
                sanitize_game_title("The Witcher 3: Wild Hunt — Game of the Year Edition"),
                "The Witcher 3 Wild Hunt"
            );
            assert_eq!(sanitize_game_title("Elden Ring GOTY"), "Elden Ring");
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
}

pub mod stores_pure {
    use serde::Serialize;
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    #[derive(Debug, Clone, Serialize)]
    pub struct DetectedGame {
        pub name: String,
        pub exe_path: Option<String>,
        pub launch_uri: Option<String>,
        pub source: String,
        pub appid: Option<u32>,
    }
    #[derive(Debug, Clone, Serialize)]
    pub struct ScannedFound {
        pub name: String,
        pub exe_path: String,
    }
    pub fn parse_epic_item(json_text: &str) -> Option<DetectedGame> {
        let v: serde_json::Value = serde_json::from_str(json_text).ok()?;
        let name = v.get("DisplayName")?.as_str()?;
        let location = v.get("InstallLocation")?.as_str()?;
        let exe = v.get("LaunchExecutable")?.as_str()?;
        if name.is_empty() || location.is_empty() || exe.is_empty() {
            return None;
        }
        let exe_path = Path::new(location).join(exe);
        Some(DetectedGame {
            name: name.to_string(),
            exe_path: Some(exe_path.to_string_lossy().into_owned()),
            launch_uri: None,
            source: "epic".to_string(),
            appid: None,
        })
    }
    pub fn battlenet_product(code: &str) -> Option<(&'static str, &'static str)> {
        match code {
            "wow" => Some(("World of Warcraft", "Wow.exe")),
            "d3" => Some(("Diablo III", "Diablo III.exe")),
            "s1" => Some(("StarCraft", "StarCraft.exe")),
            "s2" => Some(("StarCraft II", "StarCraft II.exe")),
            "hsb" => Some(("Hearthstone", "Hearthstone.exe")),
            "heroes" => Some(("Heroes of the Storm", "Heroes of the Storm.exe")),
            "ow" => Some(("Overwatch", "Overwatch.exe")),
            "w3" => Some(("Warcraft III", "Warcraft III.exe")),
            "destiny2" => Some(("Destiny 2", "destiny2.exe")),
            _ => None,
        }
    }
    pub fn is_installer_exe(stem_lower: &str, siblings: &[String]) -> bool {
        // `setup.exe` is ALWAYS an installer — even when it is the only exe
        // in a folder. Treating a lone installer as a game would add
        // installers to the library.
        if stem_lower == "setup" {
            return true;
        }
        if stem_lower.contains("uninstall")
            || stem_lower.contains("unins")
            || stem_lower.contains("vcredist")
            || stem_lower.contains("dxsetup")
            || stem_lower.contains("redist")
        {
            return true;
        }
        if stem_lower == "update" || stem_lower == "updater" || stem_lower.contains("updater") {
            return true;
        }
        // Helper / non-game exes commonly found inside real game folders.
        const HELPERS: &[&str] = &[
            "crashreporter",
            "crashhandler",
            "unitycrashhandler",
            "crashpad",
            "ue4prereq",
            "ueprereq",
            "dotnet",
            "benchmark",
            "config",
            "easyanticheat_setup",
            "battleye",
        ];
        if HELPERS.iter().any(|h| stem_lower.contains(h)) {
            return true;
        }
        let _ = siblings; // kept for signature compatibility; no longer needed
        false
    }
    pub fn display_name_for_exe(exe_path: &Path) -> String {
        let normalized = exe_path.to_string_lossy().replace('\\', "/");
        let p = Path::new(&normalized);
        let stem = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Game")
            .to_string();
        let generic = ["launcher", "game", "client", "main", "start", "play"];
        if generic.contains(&stem.to_lowercase().as_str()) {
            if let Some(parent) = p
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
            {
                return prettify(parent);
            }
        }
        prettify(&stem)
    }
    fn prettify(s: &str) -> String {
        let mut out = String::new();
        let mut cap_next = true;
        for c in s.chars() {
            if c == '_' || c == '-' || c == '.' {
                out.push(' ');
                cap_next = true;
            } else if cap_next {
                out.extend(c.to_uppercase());
                cap_next = false;
            } else {
                out.push(c);
            }
        }
        out.trim().to_string()
    }
    fn walk_exes(dir: &Path, depth: u32, max_depth: u32, out: &mut Vec<PathBuf>) {
        if depth > max_depth {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            // Never follow symlinks or directory junctions: they can loop
            // forever or escape the configured root folder. `file_type()`
            // does not follow links, so `is_symlink()` catches both.
            if entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
                continue;
            }
            let p = entry.path();
            if p.is_dir() {
                // Skip obvious non-game folders to keep scans fast.
                if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                    let lower = name.to_lowercase();
                    if [
                        "__pycache__",
                        "node_modules",
                        ".git",
                        "redist",
                        "directx",
                        "_commonredist",
                    ]
                    .contains(&lower.as_str())
                    {
                        continue;
                    }
                }
                walk_exes(&p, depth + 1, max_depth, out);
            } else if p
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("exe"))
                == Some(true)
            {
                out.push(p);
            }
        }
    }
    pub fn scan_folders(folders: &[String]) -> Vec<ScannedFound> {
        let mut found: Vec<PathBuf> = Vec::new();
        for folder in folders {
            let dir = PathBuf::from(folder);
            if dir.is_dir() {
                walk_exes(&dir, 0, 4, &mut found);
            }
        }
        // Group exes by parent dir for the `setup.exe` sibling rule.
        let mut by_dir: std::collections::HashMap<PathBuf, Vec<String>> =
            std::collections::HashMap::new();
        for p in &found {
            if let Some(parent) = p.parent() {
                let stem = p
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                by_dir.entry(parent.to_path_buf()).or_default().push(stem);
            }
        }
        let mut seen: HashSet<String> = HashSet::new();
        let mut out = Vec::new();
        for p in found {
            let stem = p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            let siblings = p
                .parent()
                .and_then(|par| by_dir.get(par))
                .cloned()
                .unwrap_or_default();
            if is_installer_exe(&stem, &siblings) {
                continue;
            }
            let key = p.to_string_lossy().to_lowercase();
            if !seen.insert(key) {
                continue;
            }
            out.push(ScannedFound {
                name: display_name_for_exe(&p),
                exe_path: p.to_string_lossy().into_owned(),
            });
        }
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        out
    }
    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parse_epic_item_reads_manifest_fields() {
            let json = r#"{"DisplayName":"Fortnite","InstallLocation":"C:\\Program Files\\Epic Games\\Fortnite","LaunchExecutable":"FortniteGame\\Binaries\\Win64\\FortniteClient-Win64-Shipping.exe","AppName":"Fortnite"}"#;
            let g = parse_epic_item(json).unwrap();
            assert_eq!(g.name, "Fortnite");
            assert_eq!(g.source, "epic");
            assert!(g
                .exe_path
                .unwrap()
                .contains("FortniteClient-Win64-Shipping.exe"));
            assert!(parse_epic_item(r#"{"DisplayName":"NoExe"}"#).is_none());
            assert!(parse_epic_item("not json").is_none());
            assert!(parse_epic_item(
                r#"{"DisplayName":"","InstallLocation":"C:\\X","LaunchExecutable":"x.exe"}"#
            )
            .is_none());
        }

        #[test]
        fn battlenet_product_maps_known_codes_only() {
            assert_eq!(
                battlenet_product("wow"),
                Some(("World of Warcraft", "Wow.exe"))
            );
            assert_eq!(
                battlenet_product("d3"),
                Some(("Diablo III", "Diablo III.exe"))
            );
            assert_eq!(battlenet_product("unknown-code"), None);
            assert_eq!(battlenet_product(""), None);
        }

        #[test]
        fn is_installer_exe_filters_installers_but_keeps_games() {
            let empty: Vec<String> = vec![];
            // Always installers.
            assert!(is_installer_exe("uninstall", &empty));
            assert!(is_installer_exe("unins000", &empty));
            assert!(is_installer_exe("uninstaller", &empty));
            assert!(is_installer_exe("vc_redist.x64", &empty));
            assert!(is_installer_exe("dxsetup", &empty));
            assert!(is_installer_exe("update", &empty));
            assert!(is_installer_exe("updater", &empty));
            assert!(is_installer_exe("gameupdater", &empty));
            // setup.exe is ALWAYS dropped, even when it is the only exe.
            assert!(is_installer_exe("setup", &empty));
            assert!(is_installer_exe(
                "setup",
                &["setup".to_string(), "game.exe".to_string()]
            ));
            // Helper / non-game exes are dropped.
            assert!(is_installer_exe("crashreporter", &empty));
            assert!(is_installer_exe("unitycrashhandler64", &empty));
            assert!(is_installer_exe("ue4prereqsetup", &empty));
            assert!(is_installer_exe("dotnet", &empty));
            assert!(is_installer_exe("benchmark", &empty));
            assert!(is_installer_exe("easyanticheat_setup", &empty));
            assert!(is_installer_exe("battleye", &empty));
            // Real games are never filtered — including Launcher.exe.
            assert!(!is_installer_exe("launcher", &empty));
            assert!(!is_installer_exe("fortniteclient-win64-shipping", &empty));
            assert!(!is_installer_exe("game", &empty));
            assert!(!is_installer_exe("setup_game", &empty));
        }

        #[test]
        fn scan_folders_does_not_follow_symlinks() {
            let root = std::env::temp_dir().join("p4-verify-scan-link");
            let _ = std::fs::remove_dir_all(&root);
            let real = root.join("Real Game");
            std::fs::create_dir_all(&real).unwrap();
            std::fs::write(real.join("RealGame.exe"), b"x").unwrap();
            let outside = std::env::temp_dir().join("p4-verify-scan-outside");
            let _ = std::fs::remove_dir_all(&outside);
            std::fs::create_dir_all(&outside).unwrap();
            std::fs::write(outside.join("Outside.exe"), b"x").unwrap();
            let link = root.join("Link Game");
            #[cfg(windows)]
            let made = std::os::windows::fs::symlink_dir(&outside, &link).is_ok();
            #[cfg(not(windows))]
            let made = std::os::unix::fs::symlink(&outside, &link).is_ok();
            if !made {
                let _ = std::fs::remove_dir_all(&root);
                let _ = std::fs::remove_dir_all(&outside);
                return;
            }
            let found = scan_folders(&[root.to_string_lossy().into_owned()]);
            let names: Vec<&str> = found.iter().map(|f| f.name.as_str()).collect();
            // (This crate's simpler scan names by exe stem: RealGame.exe.)
            assert!(names.contains(&"RealGame"), "{names:?}");
            assert!(!names.contains(&"Outside"), "{names:?}");
            let _ = std::fs::remove_dir_all(&root);
            let _ = std::fs::remove_dir_all(&outside);
        }

        #[test]
        fn display_name_for_exe_prettifies_and_uses_folder_for_generic_stems() {
            assert_eq!(
                display_name_for_exe(Path::new(
                    r"C:\Games\fortnite_client\FortniteClient-Win64-Shipping.exe"
                )),
                "FortniteClient Win64 Shipping"
            );
            assert_eq!(
                display_name_for_exe(Path::new(r"C:\Games\My Cool Game\Launcher.exe")),
                "My Cool Game"
            );
            assert_eq!(
                display_name_for_exe(Path::new(r"C:\Games\Hades\Hades.exe")),
                "Hades"
            );
            assert_eq!(
                display_name_for_exe(Path::new(r"C:\Games\doom_eternal\DOOMx64.exe")),
                "DOOMx64"
            );
        }

        #[test]
        fn scan_folders_finds_games_and_drops_installers() {
            let root = std::env::temp_dir().join("p4-verify-scan");
            let _ = std::fs::remove_dir_all(&root);
            let game = root.join("My Game");
            let other = root.join("Other Game");
            std::fs::create_dir_all(&game).unwrap();
            std::fs::create_dir_all(&other).unwrap();
            std::fs::write(game.join("MyGame.exe"), b"x").unwrap();
            std::fs::write(game.join("setup.exe"), b"x").unwrap(); // dropped: sibling exists
            std::fs::write(game.join("unins000.exe"), b"x").unwrap(); // dropped
            std::fs::write(other.join("Launcher.exe"), b"x").unwrap(); // kept, folder name
            std::fs::write(other.join("sub").join("Deep.exe"), b"x").unwrap_or_else(|_| {
                std::fs::create_dir_all(other.join("sub")).unwrap();
                std::fs::write(other.join("sub").join("Deep.exe"), b"x").unwrap();
            });
            let found = scan_folders(&[root.to_string_lossy().into_owned()]);
            let names: Vec<&str> = found.iter().map(|f| f.name.as_str()).collect();
            assert!(names.contains(&"MyGame"), "{names:?}");
            assert!(names.contains(&"Other Game"), "{names:?}");
            assert!(names.contains(&"Deep"), "{names:?}");
            assert!(!names.iter().any(|n| n.to_lowercase().contains("setup")));
            assert!(!names.iter().any(|n| n.to_lowercase().contains("unins")));
            assert_eq!(found.len(), 3);
            // Missing folder contributes nothing.
            assert!(scan_folders(&["/does/not/exist".to_string()]).is_empty());
            let _ = std::fs::remove_dir_all(&root);
        }
    }
}

pub mod launch_pure {
    /// A Steam appid is strictly ASCII digits. Anything else must never
    /// reach `steam://rungameid/{appid}`.
    pub fn is_valid_appid_str(appid: &str) -> bool {
        !appid.is_empty() && appid.chars().all(|c| c.is_ascii_digit())
    }

    /// Only a real `.exe` may be launched (never a script, shortcut, or
    /// a path that merely happens to exist).
    pub fn is_valid_exe_path(path: &str) -> bool {
        let p = path.trim();
        !p.is_empty() && p.to_lowercase().ends_with(".exe")
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn appid_must_be_ascii_digits_only() {
            assert!(is_valid_appid_str("440"));
            assert!(is_valid_appid_str("367520"));
            assert!(!is_valid_appid_str(""));
            assert!(!is_valid_appid_str("440;rm"));
            assert!(!is_valid_appid_str("44 0"));
            assert!(!is_valid_appid_str(
                "440
"
            ));
            assert!(!is_valid_appid_str("abc"));
            assert!(!is_valid_appid_str("440.exe"));
        }

        #[test]
        fn exe_path_must_end_with_exe() {
            assert!(is_valid_exe_path(r"C:\Games\Hades\Hades.exe"));
            assert!(is_valid_exe_path(r"C:\Games\Game\GAME.EXE"));
            assert!(!is_valid_exe_path(""));
            assert!(!is_valid_exe_path(r"C:\Games\Game\run.bat"));
            assert!(!is_valid_exe_path(r"C:\Games\Game\game.lnk"));
            assert!(!is_valid_exe_path(r"C:\Games\Game\game.exe.bak"));
        }
    }
}

pub mod playtime_pure {
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
}

pub mod db_tests;
