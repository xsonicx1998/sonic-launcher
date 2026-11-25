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
    pub fn steam_cdn_urls(appid: u32) -> (String, String, String, String) {
        (
            format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/header.jpg"),
            format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/library_hero.jpg"),
            format!(
                "https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/library_600x900.jpg"
            ),
            format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/logo.png"),
        )
    }
    /// PNG magic signature (89 50 4E 47 0D 0A 1A 0A).
    /// Mirror of `art.rs::is_png_bytes` — the logo validator (v0.2.8).
    pub fn is_png_bytes(b: &[u8]) -> bool {
        b.len() >= 8 && b[..8] == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
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
    /// How long a failed keyless lookup blocks automatic retries (days).
    pub const ART_LOOKUP_RETRY_DAYS: i64 = 14;
    /// Mirror of `art.rs::art_lookup_blocked`.
    pub fn art_lookup_blocked(failed_at: Option<i64>, now: i64) -> bool {
        match failed_at {
            Some(t) => now.saturating_sub(t) < ART_LOOKUP_RETRY_DAYS * 24 * 3600,
            None => false,
        }
    }
    /// Mirror of `art.rs::art_already_resolved` (v0.2.12): a manual
    /// cover, a cached GOG cover, or a resolved Steam appid means
    /// the art is settled and no store lookup is needed; an
    /// exe-icon fallback is NOT settled and must still be looked up.
    pub fn art_already_resolved(art_source: Option<&str>, resolved_appid: Option<i64>) -> bool {
        resolved_appid.is_some() || matches!(art_source, Some("manual") | Some("gog"))
    }
    /// Curated Steam appids (v0.2.13, + CoD guard v0.2.14).
    /// Mirror of `art.rs::KNOWN_APPIDS` — games whose current
    /// store listing is a renamed variant the strict matcher
    /// rejects, plus "Call of Duty Modern Warfare II": the
    /// matcher picks it, but the 2009 game normalizes to the
    /// same string and a transient failure + 14-day cooldown
    /// would leave it artless, so the override pins 3595230.
    /// Key is the normalized form (Roman "II" is already "2").
    const KNOWN_APPIDS: &[(&str, u32)] = &[
        ("grand theft auto 5", 3240220),
        ("the witcher 3", 292030),
        ("call of duty modern warfare 2", 3595230),
    ];
    /// Mirror of `art.rs::known_appid`.
    pub fn known_appid(name: &str) -> Option<u32> {
        let norm = normalize_name(&sanitize_game_name(name));
        KNOWN_APPIDS
            .iter()
            .find(|(key, _)| *key == norm)
            .map(|(_, appid)| *appid)
    }
    /// Mirror of `art.rs::steam_hit_resolves` (v0.2.13 F1): a Steam
    /// store hit resolves the game only when the CDN download
    /// actually produced the tile.
    pub fn steam_hit_resolves(download_ok: bool) -> bool {
        download_ok
    }
    /// Mirror of `art.rs::hero_fallback_needed` (v0.2.15): the
    /// downloaded hero doubles as a missing tile/capsule when a
    /// Steam listing publishes no header.jpg / library_600x900.jpg
    /// (e.g. Call of Duty: Modern Warfare II, appid 3595230).
    pub fn hero_fallback_needed(target_exists: bool, hero_exists: bool) -> bool {
        !target_exists && hero_exists
    }
    /// Mirror of `art.rs::lookup_failure_records` (v0.2.13 F2):
    /// every lookup failure is recorded except a rate limit, so
    /// "Retry Art" stays available after network errors.
    pub fn lookup_failure_records(rate_limited: bool) -> bool {
        !rate_limited
    }
    const EDITION_SUFFIXES: &[&str] = &[
        "game of the year edition",
        "game of the year",
        "goty edition",
        "goty",
        "director's cut",
        "directors cut",
        "director cut",
        "definitive edition",
        "complete edition",
        "enhanced edition",
        // Bare "enhanced" (v0.2.13): mirror of art.rs — Steam
        // replaced the original "Grand Theft Auto V" listing with
        // "Grand Theft Auto V Enhanced".
        "enhanced",
        "deluxe edition",
        "ultimate edition",
        "special edition",
        "anniversary edition",
        "collector's edition",
        "collectors edition",
        "royal edition",
        "gold edition",
        "remastered",
        "remaster",
    ];
    /// Mirror of `art.rs::sanitize_game_name`.
    pub fn sanitize_game_name(name: &str) -> String {
        let mut s = name.replace(['\u{2122}', '\u{00AE}', '\u{00A9}'], "");
        loop {
            let mut dropped = false;
            for (open, close) in [('[', ']'), ('(', ')')] {
                if let Some(start) = s.find(open) {
                    if let Some(end) = s[start..].find(close) {
                        s.replace_range(start..start + end + 1, " ");
                        dropped = true;
                    }
                }
            }
            if !dropped {
                break;
            }
        }
        let mut s = s
            .replace(['_', '.', ':', '-', ',', '\u{2013}', '\u{2014}'], " ")
            .to_lowercase();
        loop {
            let trimmed = s.trim_end();
            let mut stripped = false;
            for suffix in EDITION_SUFFIXES {
                if let Some(base) = trimmed.strip_suffix(suffix) {
                    if base.is_empty() || base.ends_with(' ') {
                        s = base.trim_end().to_string();
                        stripped = true;
                        break;
                    }
                }
            }
            if !stripped {
                break;
            }
        }
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }
    fn roman_to_arabic(word: &str) -> Option<&'static str> {
        match word {
            "i" => Some("1"),
            "ii" => Some("2"),
            "iii" => Some("3"),
            "iv" => Some("4"),
            "v" => Some("5"),
            "vi" => Some("6"),
            "vii" => Some("7"),
            "viii" => Some("8"),
            "ix" => Some("9"),
            "x" => Some("10"),
            _ => None,
        }
    }
    /// Mirror of `art.rs::normalize_name`.
    pub fn normalize_name(name: &str) -> String {
        let lower = name.to_lowercase();
        let mut tokens: Vec<String> = Vec::new();
        for raw in lower.split(|c: char| !c.is_alphanumeric()) {
            if raw.is_empty() {
                continue;
            }
            if let Some(d) = roman_to_arabic(raw) {
                tokens.push(d.to_string());
            } else {
                tokens.push(raw.to_string());
            }
        }
        tokens.join(" ")
    }
    const JUNK_WORDS: &[&str] = &[
        "soundtrack",
        "ost",
        "demo",
        "beta",
        "playtest",
        "artbook",
        "dlc",
        "upgrade",
    ];
    const JUNK_PHRASES: &[&str] = &["dedicated server", "season pass"];
    /// Mirror of `art.rs::is_junk_name`.
    pub fn is_junk_name(candidate_norm: &str, folder_norm: &str) -> bool {
        for word in JUNK_WORDS {
            if candidate_norm.split(' ').any(|t| t == *word)
                && !folder_norm.split(' ').any(|t| t == *word)
            {
                return true;
            }
        }
        for phrase in JUNK_PHRASES {
            if candidate_norm.contains(phrase) && !folder_norm.contains(phrase) {
                return true;
            }
        }
        false
    }
    /// Mirror of `art.rs::token_similarity`.
    pub fn token_similarity(a_norm: &str, b_norm: &str) -> f64 {
        let a: std::collections::HashSet<&str> =
            a_norm.split(' ').filter(|t| !t.is_empty()).collect();
        let b: std::collections::HashSet<&str> =
            b_norm.split(' ').filter(|t| !t.is_empty()).collect();
        if a.is_empty() || b.is_empty() {
            return 0.0;
        }
        let inter = a.intersection(&b).count() as f64;
        let union = a.union(&b).count() as f64;
        inter / union
    }
    /// Mirror of `art.rs::pick_match`.
    pub fn pick_match(folder_name: &str, candidates: &[String]) -> Option<usize> {
        let folder_norm = normalize_name(&sanitize_game_name(folder_name));
        if folder_norm.is_empty() {
            return None;
        }
        let mut scored: Vec<(usize, f64)> = Vec::new();
        for (i, cand) in candidates.iter().enumerate() {
            let cand_norm = normalize_name(&sanitize_game_name(cand));
            if cand_norm.is_empty() || is_junk_name(&cand_norm, &folder_norm) {
                continue;
            }
            if cand_norm == folder_norm {
                return Some(i);
            }
            scored.push((i, token_similarity(&folder_norm, &cand_norm)));
        }
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let Some(&(top_i, top_s)) = scored.first() else {
            return None;
        };
        if top_s < 0.85 {
            return None;
        }
        if let Some(&(_, second_s)) = scored.get(1) {
            if top_s - second_s < 0.05 {
                return None;
            }
        }
        Some(top_i)
    }
    /// Mirror of `art.rs::parse_storesearch`.
    pub fn parse_storesearch(json: &serde_json::Value) -> Vec<(u32, String)> {
        let mut out = Vec::new();
        let Some(items) = json.get("items").and_then(|i| i.as_array()) else {
            return out;
        };
        for item in items {
            let appid = item
                .get("id")
                .and_then(|i| i.as_u64())
                .and_then(|i| u32::try_from(i).ok());
            let name = item.get("name").and_then(|n| n.as_str());
            let kind = item.get("type").and_then(|t| t.as_str());
            if let (Some(appid), Some(name)) = (appid, name) {
                if kind == Some("app") {
                    out.push((appid, name.to_string()));
                }
            }
        }
        out
    }
    /// Mirror of `art.rs::GogCover`.
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct GogCover {
        pub title: String,
        pub vertical: Option<String>,
        pub horizontal: Option<String>,
    }
    /// Mirror of `art.rs::parse_gog_catalog`.
    pub fn parse_gog_catalog(json: &serde_json::Value) -> Vec<GogCover> {
        let mut out = Vec::new();
        let Some(products) = json.get("products").and_then(|p| p.as_array()) else {
            return out;
        };
        for item in products {
            let kind = item.get("productType").and_then(|t| t.as_str());
            if !matches!(kind, Some("game") | Some("pack")) {
                continue;
            }
            let Some(title) = item.get("title").and_then(|t| t.as_str()) else {
                continue;
            };
            let vertical = item
                .get("coverVertical")
                .and_then(|u| u.as_str())
                .filter(|u| !u.is_empty())
                .map(|u| u.to_string());
            let horizontal = item
                .get("coverHorizontal")
                .and_then(|u| u.as_str())
                .filter(|u| !u.is_empty())
                .map(|u| u.to_string());
            if vertical.is_none() && horizontal.is_none() {
                continue;
            }
            out.push(GogCover {
                title: title.to_string(),
                vertical,
                horizontal,
            });
        }
        out
    }
    /// Mirror of `art.rs::validate_image_url`.
    pub fn validate_image_url(url: &str) -> Result<(), String> {
        let url = url.trim();
        if url.is_empty() {
            return Err("image URL is empty".to_string());
        }
        if !url.starts_with("https://") {
            return Err("image URL must start with https://".to_string());
        }
        if url.chars().any(|c| c.is_whitespace()) {
            return Err("image URL must not contain whitespace".to_string());
        }
        Ok(())
    }
    /// Mirror of `art.rs::is_gog_cover_url`.
    pub fn is_gog_cover_url(url: &str) -> bool {
        url.starts_with("https://images.gog-statics.com/")
    }
    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn steam_cdn_urls_follow_the_known_pattern() {
            let (h, hero, c, logo) = steam_cdn_urls(440);
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
            assert_eq!(
                logo,
                "https://cdn.cloudflare.steamstatic.com/steam/apps/440/logo.png"
            );
        }

        #[test]
        fn is_png_bytes_accepts_only_the_png_signature() {
            assert!(is_png_bytes(&[
                0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00
            ]));
            assert!(!is_png_bytes(&[0x89, 0x50, 0x4E, 0x47]));
            assert!(!is_png_bytes(&[
                0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x00, 0x00
            ]));
            assert!(!is_png_bytes(b"<!DOCTYPE html>"));
            assert!(!is_png_bytes(b""));
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

        #[test]
        fn sanitize_strips_edition_suffixes_brackets_and_marks() {
            assert_eq!(sanitize_game_name("Cyberpunk 2077"), "cyberpunk 2077");
            assert_eq!(
                sanitize_game_name("Cyberpunk 2077 [Repack] (2024)"),
                "cyberpunk 2077"
            );
            assert_eq!(
                sanitize_game_name("The Witcher 3: Wild Hunt - Game of the Year Edition"),
                "the witcher 3 wild hunt"
            );
            assert_eq!(
                sanitize_game_name("Sleeping Dogs™ Definitive Edition"),
                "sleeping dogs"
            );
            assert_eq!(sanitize_game_name("Hades II - Director's Cut"), "hades ii");
            assert_eq!(sanitize_game_name("My_Game.Name GOTY"), "my game name");
            assert_eq!(sanitize_game_name("[Repack]"), "");
            assert_eq!(sanitize_game_name("Battlefield 1942"), "battlefield 1942");
        }

        #[test]
        fn normalize_converts_roman_numerals_and_punctuation() {
            assert_eq!(normalize_name("The Witcher III"), "the witcher 3");
            assert_eq!(normalize_name("Grand Theft Auto V"), "grand theft auto 5");
            assert_eq!(normalize_name("Hades II"), "hades 2");
            assert_eq!(normalize_name("Civilization VI"), "civilization 6");
            assert_eq!(normalize_name("Civil"), "civil");
            assert_eq!(normalize_name("Baldur's Gate 3"), "baldur s gate 3");
        }

        #[test]
        fn junk_filter_drops_soundtracks_and_demos() {
            let folder = normalize_name("Hades II");
            assert!(is_junk_name(
                &normalize_name("Hades II Soundtrack"),
                &folder
            ));
            assert!(is_junk_name(&normalize_name("Hades II Demo"), &folder));
            assert!(is_junk_name(&normalize_name("Hades II OST"), &folder));
            assert!(is_junk_name(
                &normalize_name("Hades II Dedicated Server"),
                &folder
            ));
            assert!(is_junk_name(
                &normalize_name("Hades II Season Pass"),
                &folder
            ));
            assert!(!is_junk_name(&normalize_name("Hades II"), &folder));
            assert!(!is_junk_name(
                &normalize_name("Ghost of Tsushima"),
                &normalize_name("Ghost of Tsushima")
            ));
            let ost_folder = normalize_name("Hades II Soundtrack");
            assert!(!is_junk_name(
                &normalize_name("Hades II Soundtrack"),
                &ost_folder
            ));
        }

        #[test]
        fn token_similarity_is_jaccard_on_token_sets() {
            assert!((token_similarity("a b c", "a b c") - 1.0).abs() < 1e-9);
            assert!(
                (token_similarity("cyberpunk 2077", "cyberpunk 2077 phantom liberty") - 0.5).abs()
                    < 1e-9
            );
            assert_eq!(token_similarity("a b", "c d"), 0.0);
            assert_eq!(token_similarity("", "a"), 0.0);
        }

        #[test]
        fn pick_match_exact_wins_over_earlier_candidates() {
            let cands = vec![
                "Cyberpunk 2077: Phantom Liberty".to_string(),
                "Cyberpunk 2077".to_string(),
            ];
            assert_eq!(pick_match("Cyberpunk 2077", &cands), Some(1));
        }

        #[test]
        fn pick_match_junk_filtered_and_roman_matched() {
            let cands = vec![
                "The Witcher 3 Soundtrack".to_string(),
                "The Witcher III".to_string(),
            ];
            assert_eq!(pick_match("The Witcher 3", &cands), Some(1));
        }

        #[test]
        fn pick_match_tie_returns_none_and_clear_winner_wins() {
            let tied = vec![
                "Legend of Zelda Breath of the Wild Master".to_string(),
                "Legend of Zelda Breath of the Wild Deluxe".to_string(),
            ];
            assert_eq!(
                pick_match("Legend of Zelda Breath of the Wild", &tied),
                None
            );
            let clear = vec![
                "Legend of Zelda Breath of the Wild Master".to_string(),
                "Legend of Zelda Tears of the Kingdom".to_string(),
            ];
            assert_eq!(
                pick_match("Legend of Zelda Breath of the Wild", &clear),
                Some(0)
            );
            assert_eq!(pick_match("Doom", &["Doom Eternal".to_string()]), None);
            assert_eq!(pick_match("Doom", &[] as &[String]), None);
        }

        #[test]
        fn pick_match_grand_theft_auto_v_is_a_tier1_exact_match() {
            // v0.2.13: the live Steam storesearch (2026-11-24) no
            // longer returns the bare "Grand Theft Auto V" (271590) —
            // Rockstar replaced the listing with "Grand Theft Auto V
            // Enhanced" (3240220). The old test used a hypothetical
            // list containing the bare title, so its premise was
            // stale. With bare trailing "enhanced" stripped, the
            // Enhanced listing normalizes to "grand theft auto 5"
            // and Tier-1 matches.
            let cands = vec![
                "Grand Theft Auto V Enhanced".to_string(),
                "Grand Theft Auto: Vice City – The Definitive Edition".to_string(),
            ];
            assert_eq!(pick_match("Grand Theft Auto V", &cands), Some(0));
            // The folder name itself sanitizes the same way.
            assert_eq!(
                sanitize_game_name("Grand Theft Auto V Enhanced"),
                "grand theft auto v"
            );
        }

        #[test]
        fn pick_match_call_of_duty_mw2_live_list() {
            // v0.2.14 probe: the live storesearch (2026-11-25) for
            // "call of duty modern warfare ii" returns the 2022
            // game first (3595230) and the 2009 game second
            // (10180). The 2022 game must win.
            let cands = vec![
                "Call of Duty®: Modern Warfare® II".to_string(),
                "Call of Duty®: Modern Warfare® 2 (2009)".to_string(),
                "Call of Duty®: Modern Warfare® II - Gunslinger Ghost".to_string(),
            ];
            assert_eq!(
                pick_match("Call of Duty Modern Warfare II", &cands),
                Some(0)
            );
        }

        #[test]
        fn pick_match_witcher_3_live_list_has_no_bare_name() {
            // v0.2.13: the live storesearch returns only renamed
            // variants — "Wild Hunt — Remastered" (0.600) and
            // "REDkit" (0.750) — so the strict matcher rightly
            // returns None and the curated override is what finds
            // the cover.
            let cands = vec![
                "The Witcher 3: Wild Hunt — Remastered".to_string(),
                "The Witcher 3 REDkit".to_string(),
            ];
            assert_eq!(pick_match("The Witcher 3", &cands), None);
        }

        #[test]
        fn known_appid_overrides_renamed_steam_listings() {
            // v0.2.13: Steam returns no bare name for these two.
            assert_eq!(known_appid("Grand Theft Auto V"), Some(3240220));
            assert_eq!(known_appid("The Witcher 3"), Some(292030));
            // The sanitizer normalizes edition suffixes first, so an
            // "Enhanced" folder name maps to the same appid.
            assert_eq!(known_appid("Grand Theft Auto V Enhanced"), Some(3240220));
            // v0.2.14: CoD MW2 (2022) is pinned to 3595230 — the
            // matcher picks it, but the 2009 game normalizes to the
            // same string, so the override removes the near-tie and
            // the 14-day cooldown from the path entirely. Both
            // spellings of the folder name map (Roman "II" is
            // normalized to "2" before the lookup).
            assert_eq!(known_appid("Call of Duty Modern Warfare II"), Some(3595230));
            assert_eq!(known_appid("Call of Duty Modern Warfare 2"), Some(3595230));
            // Everything else goes through the store search.
            assert_eq!(known_appid("Cyberpunk 2077"), None);
            assert_eq!(known_appid("Some Random Game"), None);
            assert_eq!(known_appid(""), None);
        }

        #[test]
        fn steam_hit_resolves_only_when_the_download_produced_the_tile() {
            // v0.2.13 F1: a failed CDN download must not resolve the game.
            assert!(steam_hit_resolves(true));
            assert!(!steam_hit_resolves(false));
        }

        #[test]
        fn hero_fallback_needed_only_when_target_missing_and_hero_present() {
            // v0.2.15: CoD MWII (3595230) has no header/capsule on the
            // CDN — the hero is the only art — so the fallback fires.
            assert!(hero_fallback_needed(false, true));
            // A real tile/capsule is never overwritten.
            assert!(!hero_fallback_needed(true, true));
            // No hero, nothing to fall back to.
            assert!(!hero_fallback_needed(false, false));
            assert!(!hero_fallback_needed(true, false));
        }

        #[test]
        fn lookup_failure_records_everything_but_a_rate_limit() {
            // v0.2.13 F2: network failures are recorded so "Retry Art"
            // stays available; a rate limit is transient and is not.
            assert!(lookup_failure_records(false));
            assert!(!lookup_failure_records(true));
        }

        #[test]
        fn art_already_resolved_settles_manual_gog_and_appid_not_icon() {
            // Settled: no store lookup needed.
            assert!(art_already_resolved(Some("manual"), None));
            assert!(art_already_resolved(Some("gog"), None));
            assert!(art_already_resolved(Some("steam"), Some(271590)));
            assert!(art_already_resolved(None, Some(271590)));
            // Not settled: the lookup must still run.
            assert!(!art_already_resolved(Some("icon"), None));
            assert!(!art_already_resolved(None, None));
            assert!(!art_already_resolved(Some("steam"), None));
        }

        #[test]
        fn parse_storesearch_keeps_apps_only() {
            let json: serde_json::Value = serde_json::from_str(
                r#"{"total":3,"items":[
                    {"type":"app","name":"Cyberpunk 2077","id":1091500},
                    {"type":"dlc","name":"Cyberpunk 2077: Phantom Liberty","id":2138330},
                    {"type":"app","name":"No Id Here"},
                    {"name":"No Type","id":42}
                ]}"#,
            )
            .unwrap();
            let out = parse_storesearch(&json);
            assert_eq!(out, vec![(1091500, "Cyberpunk 2077".to_string())]);
            assert!(parse_storesearch(&serde_json::json!({})).is_empty());
            assert!(parse_storesearch(&serde_json::json!({"items": "nope"})).is_empty());
        }

        #[test]
        fn parse_gog_catalog_keeps_games_and_packs_with_covers() {
            let json: serde_json::Value = serde_json::from_str(
                r#"{"products":[
                    {"id":"1","title":"Cyberpunk 2077","productType":"game",
                     "coverVertical":"https://images.gog-statics.com/v.jpg",
                     "coverHorizontal":"https://images.gog-statics.com/h.png"},
                    {"id":"2","title":"Cyberpunk 2077 DLC","productType":"dlc",
                     "coverVertical":"https://images.gog-statics.com/v2.jpg",
                     "coverHorizontal":"https://images.gog-statics.com/h2.png"},
                    {"id":"3","title":"No Covers","productType":"game"},
                    {"id":"4","title":"Kingdom Come II Royal","productType":"pack",
                     "coverVertical":"https://images.gog-statics.com/v3.jpg"}
                ]}"#,
            )
            .unwrap();
            let out = parse_gog_catalog(&json);
            assert_eq!(out.len(), 2);
            assert_eq!(out[0].title, "Cyberpunk 2077");
            assert_eq!(
                out[0].vertical.as_deref(),
                Some("https://images.gog-statics.com/v.jpg")
            );
            assert_eq!(
                out[0].horizontal.as_deref(),
                Some("https://images.gog-statics.com/h.png")
            );
            assert_eq!(out[1].title, "Kingdom Come II Royal");
            assert!(out[1].horizontal.is_none());
            assert!(parse_gog_catalog(&serde_json::json!({})).is_empty());
        }

        #[test]
        fn art_lookup_blocked_honours_the_14_day_cooldown() {
            let now = 1_800_000_000i64;
            assert!(!art_lookup_blocked(None, now));
            assert!(art_lookup_blocked(Some(now - 13 * 24 * 3600), now));
            assert!(!art_lookup_blocked(Some(now - 14 * 24 * 3600), now));
            assert!(!art_lookup_blocked(Some(now - 30 * 24 * 3600), now));
            assert!(art_lookup_blocked(Some(now), now));
        }

        #[test]
        fn validate_image_url_requires_https_and_no_whitespace() {
            assert!(validate_image_url("https://example.com/cover.jpg").is_ok());
            assert!(validate_image_url("http://example.com/cover.jpg").is_err());
            assert!(validate_image_url("https://example.com/a b.jpg").is_err());
            assert!(validate_image_url("https://example.com/a\nb.jpg").is_err());
            assert!(validate_image_url("").is_err());
            assert!(validate_image_url("  https://example.com/c.png  ").is_ok());
        }

        #[test]
        fn is_gog_cover_url_allows_only_the_gog_image_host() {
            assert!(is_gog_cover_url(
                "https://images.gog-statics.com/abc123.jpg"
            ));
            assert!(!is_gog_cover_url(
                "https://images.gog-statics.com.evil.com/abc.jpg"
            ));
            assert!(!is_gog_cover_url("https://cdn.example.com/cover.jpg"));
            assert!(!is_gog_cover_url("http://images.gog-statics.com/abc.jpg"));
        }
    }
}

/// Mirror of `exe_icon.rs` (v0.2.6): pure-Rust PE icon extraction.
/// The file-IO wrapper (`extract_exe_icon_png`) is not mirrored —
/// only the pure parsing, verbatim.
pub mod exe_icon_pure {
    /// Largest side (px) an icon must have to be used.
    pub const MIN_ICON_PX: u32 = 64;
    /// Largest .exe we will read (bytes).
    pub const MAX_EXE_BYTES: usize = 128 * 1024 * 1024;

    const PNG_MAGIC: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

    /// `true` when the bytes start with the PNG magic signature.
    pub fn is_png(b: &[u8]) -> bool {
        b.len() >= PNG_MAGIC.len() && b[..8] == *PNG_MAGIC
    }

    /// Width and height from a PNG's IHDR chunk (`None` when the
    /// bytes are not a PNG or are too short to hold an IHDR).
    pub fn png_dimensions(png: &[u8]) -> Option<(u32, u32)> {
        if !is_png(png) || png.len() < 24 {
            return None;
        }
        if &png[12..16] != b"IHDR" {
            return None;
        }
        let w = u32::from_be_bytes(png[16..20].try_into().ok()?);
        let h = u32::from_be_bytes(png[20..24].try_into().ok()?);
        Some((w, h))
    }

    fn u16le(b: &[u8], off: usize) -> Option<u16> {
        Some(u16::from_le_bytes(b.get(off..off + 2)?.try_into().ok()?))
    }

    fn u32le(b: &[u8], off: usize) -> Option<u32> {
        Some(u32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?))
    }

    struct Section {
        virt_addr: u32,
        virt_size: u32,
        raw_ptr: u32,
        raw_size: u32,
    }

    struct PeInfo {
        rsrc_rva: u32,
        rsrc_size: u32,
        sections: Vec<Section>,
        headers_size: u32,
    }

    fn parse_pe(pe: &[u8]) -> Option<PeInfo> {
        if pe.len() < 0x40 || &pe[0..2] != b"MZ" {
            return None;
        }
        let e_lfanew = u32le(pe, 0x3C)? as usize;
        let nt = pe.get(e_lfanew..)?;
        if nt.len() < 24 || &nt[0..4] != b"PE\0\0" {
            return None;
        }
        let num_sections = u16le(nt, 6)? as usize;
        if num_sections == 0 || num_sections > 96 {
            return None;
        }
        let opt_size = u16le(nt, 20)? as usize;
        let opt = nt.get(24..24 + opt_size)?;
        let magic = u16le(opt, 0)?;
        let dd_base = match magic {
            0x10B => 96,
            0x20B => 112,
            _ => return None,
        };
        if opt.len() < dd_base + 24 {
            return None;
        }
        let rsrc_rva = u32le(opt, dd_base + 16)?;
        let rsrc_size = u32le(opt, dd_base + 20)?;
        let headers_size = u32le(opt, 60)?;
        let sect_off = e_lfanew.checked_add(24)?.checked_add(opt_size)?;
        let mut sections = Vec::new();
        for i in 0..num_sections {
            let s = pe.get(sect_off + i * 40..sect_off + i * 40 + 40)?;
            sections.push(Section {
                virt_addr: u32le(s, 12)?,
                virt_size: u32le(s, 8)?,
                raw_ptr: u32le(s, 20)?,
                raw_size: u32le(s, 16)?,
            });
        }
        Some(PeInfo {
            rsrc_rva,
            rsrc_size,
            sections,
            headers_size,
        })
    }

    fn rva_to_offset(info: &PeInfo, rva: u32) -> Option<usize> {
        if rva < info.headers_size {
            return Some(rva as usize);
        }
        for s in &info.sections {
            if rva >= s.virt_addr {
                let delta = rva.checked_sub(s.virt_addr)?;
                if delta < s.virt_size.max(s.raw_size) {
                    return (s.raw_ptr as usize).checked_add(delta as usize);
                }
            }
        }
        None
    }

    struct Rsrc<'a> {
        pe: &'a [u8],
        info: &'a PeInfo,
        data: &'a [u8],
    }

    impl<'a> Rsrc<'a> {
        fn dir_header(&self, off: usize) -> Option<(usize, usize)> {
            let _ = self.data.get(off..off + 16)?;
            Some((
                u16le(self.data, off + 12)? as usize,
                u16le(self.data, off + 14)? as usize,
            ))
        }

        fn dir_entry(&self, off: usize, idx: usize) -> Option<(u32, u32)> {
            let e = self.data.get(off + 16 + idx * 8..off + 16 + idx * 8 + 8)?;
            Some((u32le(e, 0)?, u32le(e, 4)?))
        }

        fn find_subdir(&self, dir_off: usize, name_id: u16) -> Option<usize> {
            let (named, ids) = self.dir_header(dir_off)?;
            let total = named.checked_add(ids)?;
            for i in 0..total {
                let (name, target) = self.dir_entry(dir_off, i)?;
                if name & 0x8000_0000 != 0 {
                    continue;
                }
                if name as u16 == name_id && target & 0x8000_0000 != 0 {
                    return Some((target & 0x7FFF_FFFF) as usize);
                }
            }
            None
        }

        fn read_data_entry(&self, data_off: usize) -> Option<(usize, usize)> {
            let e = self.data.get(data_off..data_off + 16)?;
            let rva = u32le(e, 0)?;
            let size = u32le(e, 4)? as usize;
            let off = rva_to_offset(self.info, rva)?;
            self.pe.get(off..off.checked_add(size)?)?;
            Some((off, size))
        }

        fn find_resource(&self, type_id: u16, name_id: u16) -> Option<(usize, usize)> {
            let type_dir = self.find_subdir(0, type_id)?;
            let name_dir = self.find_subdir(type_dir, name_id)?;
            let (named, ids) = self.dir_header(name_dir)?;
            let total = named.checked_add(ids)?;
            for i in 0..total {
                let (_name, target) = self.dir_entry(name_dir, i)?;
                if target & 0x8000_0000 == 0 {
                    return self.read_data_entry(target as usize);
                }
            }
            None
        }

        fn list_names(&self, type_id: u16) -> Vec<u16> {
            let mut out = Vec::new();
            let Some(type_dir) = self.find_subdir(0, type_id) else {
                return out;
            };
            let Some((named, ids)) = self.dir_header(type_dir) else {
                return out;
            };
            let Some(total) = named.checked_add(ids) else {
                return out;
            };
            for i in 0..total {
                if let Some((name, _)) = self.dir_entry(type_dir, i) {
                    if name & 0x8000_0000 == 0 {
                        out.push(name as u16);
                    }
                }
            }
            out
        }
    }

    struct IconEntry {
        width: u32,
        height: u32,
        bit_count: u16,
        bytes_in_res: u32,
        id: u16,
    }

    fn parse_group(data: &[u8]) -> Option<Vec<IconEntry>> {
        if data.len() < 6 {
            return None;
        }
        let count = u16le(data, 4)? as usize;
        let max = data.len().saturating_sub(6) / 14;
        let count = count.min(max);
        let mut out = Vec::new();
        for i in 0..count {
            let e = data.get(6 + i * 14..6 + i * 14 + 14)?;
            let w = e[0] as u32;
            let h = e[1] as u32;
            out.push(IconEntry {
                width: if w == 0 { 256 } else { w },
                height: if h == 0 { 256 } else { h },
                bit_count: u16le(e, 6)?,
                bytes_in_res: u32le(e, 8)?,
                id: u16le(e, 12)?,
            });
        }
        Some(out)
    }

    /// Extract the largest embedded icon as PNG bytes, or `None`
    /// when the file is not a PE, has no icon resources, there is
    /// no PNG icon entry, or the largest PNG is smaller than
    /// [`MIN_ICON_PX`] on its largest side. Never panics on
    /// malformed input.
    pub fn extract_largest_icon_png(pe: &[u8]) -> Option<Vec<u8>> {
        let info = parse_pe(pe)?;
        if info.rsrc_rva == 0 || info.rsrc_size == 0 {
            return None;
        }
        let rsrc_off = rva_to_offset(&info, info.rsrc_rva)?;
        let rsrc = pe.get(rsrc_off..rsrc_off.checked_add(info.rsrc_size as usize)?)?;
        let r = Rsrc {
            pe,
            info: &info,
            data: rsrc,
        };
        let mut best: Option<IconEntry> = None;
        for name in r.list_names(14) {
            let Some((off, size)) = r.find_resource(14, name) else {
                continue;
            };
            let Some(group_bytes) = pe.get(off..off.checked_add(size)?) else {
                continue;
            };
            let Some(entries) = parse_group(group_bytes) else {
                continue;
            };
            for e in entries {
                // Only PNG entries compete: a larger BMP must not
                // shadow a smaller usable PNG.
                let Some((doff, dsize)) = r.find_resource(3, e.id) else {
                    continue;
                };
                let Some(data) = pe.get(doff..doff.checked_add(dsize)?) else {
                    continue;
                };
                if !is_png(data) {
                    continue;
                }
                let better = match &best {
                    None => true,
                    Some(b) => {
                        (e.width * e.height, e.bit_count, e.bytes_in_res)
                            > (b.width * b.height, b.bit_count, b.bytes_in_res)
                    }
                };
                if better {
                    best = Some(e);
                }
            }
        }
        let best = best?;
        let (off, size) = r.find_resource(3, best.id)?;
        let bytes = pe.get(off..off.checked_add(size)?)?;
        if !is_png(bytes) {
            return None;
        }
        let (w, h) = png_dimensions(bytes)?;
        if w.max(h) < MIN_ICON_PX {
            return None;
        }
        Some(bytes.to_vec())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Minimal PNG bytes with the given IHDR dimensions.
        fn fake_png(w: u32, h: u32) -> Vec<u8> {
            let mut v = Vec::new();
            v.extend_from_slice(b"\x89PNG\r\n\x1a\n");
            v.extend_from_slice(&13u32.to_be_bytes());
            v.extend_from_slice(b"IHDR");
            v.extend_from_slice(&w.to_be_bytes());
            v.extend_from_slice(&h.to_be_bytes());
            v.extend_from_slice(&[8, 6, 0, 0, 0]);
            v.extend_from_slice(&[0, 0, 0, 0]);
            v.extend_from_slice(&0u32.to_be_bytes());
            v.extend_from_slice(b"IEND");
            v.extend_from_slice(&[0, 0, 0, 0]);
            v
        }

        type GroupEntry = (u8, u8, u16, u16, Vec<u8>);

        /// Build a minimal PE32+ with one .rsrc section holding the
        /// given RT_GROUP_ICON groups: (group name id, entries).
        fn build_pe(groups: &[(u16, Vec<GroupEntry>)]) -> Vec<u8> {
            let n_groups = groups.len();
            let n_icons: usize = groups.iter().map(|(_, e)| e.len()).sum();
            let root_size = 16 + 2 * 8;
            let t14_size = 16 + n_groups * 8;
            let t3_size = 16 + n_icons * 8;
            let lang_size = 16 + 8;
            let t14_off = root_size;
            let t3_off = t14_off + t14_size;
            let g_lang_off = t3_off + t3_size;
            let i_lang_off = g_lang_off + n_groups * lang_size;
            let data_entries_off = i_lang_off + n_icons * lang_size;
            let n_data = n_groups + n_icons;
            let raw_off = data_entries_off + n_data * 16;
            let mut raw: Vec<u8> = Vec::new();
            let mut group_raw_off: Vec<usize> = Vec::new();
            let mut icon_raw_off: Vec<usize> = Vec::new();
            let mut icon_id_order: Vec<u16> = Vec::new();
            for (_, entries) in groups {
                group_raw_off.push(raw.len());
                let mut g = Vec::new();
                g.extend_from_slice(&0u16.to_le_bytes());
                g.extend_from_slice(&1u16.to_le_bytes());
                g.extend_from_slice(&(entries.len() as u16).to_le_bytes());
                for (w, h, bits, id, bytes) in entries {
                    g.push(*w);
                    g.push(*h);
                    g.push(0);
                    g.push(0);
                    g.extend_from_slice(&1u16.to_le_bytes());
                    g.extend_from_slice(&bits.to_le_bytes());
                    g.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                    g.extend_from_slice(&id.to_le_bytes());
                    icon_id_order.push(*id);
                }
                raw.extend_from_slice(&g);
            }
            for (_, entries) in groups {
                for (_, _, _, _, bytes) in entries {
                    icon_raw_off.push(raw.len());
                    raw.extend_from_slice(bytes);
                }
            }
            let rsrc_len = raw_off + raw.len();
            let mut rsrc = vec![0u8; rsrc_len];
            let w16 = |rsrc: &mut Vec<u8>, off: usize, v: u16| {
                rsrc[off..off + 2].copy_from_slice(&v.to_le_bytes());
            };
            let w32 = |rsrc: &mut Vec<u8>, off: usize, v: u32| {
                rsrc[off..off + 4].copy_from_slice(&v.to_le_bytes());
            };
            w16(&mut rsrc, 12, 0);
            w16(&mut rsrc, 14, 2);
            w32(&mut rsrc, 16, 14);
            w32(&mut rsrc, 20, (t14_off as u32) | 0x8000_0000);
            w32(&mut rsrc, 24, 3);
            w32(&mut rsrc, 28, (t3_off as u32) | 0x8000_0000);
            w16(&mut rsrc, t14_off + 12, 0);
            w16(&mut rsrc, t14_off + 14, n_groups as u16);
            for (gi, (name, _)) in groups.iter().enumerate() {
                let e = t14_off + 16 + gi * 8;
                w32(&mut rsrc, e, *name as u32);
                w32(
                    &mut rsrc,
                    e + 4,
                    ((g_lang_off + gi * lang_size) as u32) | 0x8000_0000,
                );
            }
            w16(&mut rsrc, t3_off + 12, 0);
            w16(&mut rsrc, t3_off + 14, n_icons as u16);
            for (k, id) in icon_id_order.iter().enumerate() {
                let e = t3_off + 16 + k * 8;
                w32(&mut rsrc, e, *id as u32);
                w32(
                    &mut rsrc,
                    e + 4,
                    ((i_lang_off + k * lang_size) as u32) | 0x8000_0000,
                );
            }
            for gi in 0..n_groups {
                let d = g_lang_off + gi * lang_size;
                w16(&mut rsrc, d + 12, 0);
                w16(&mut rsrc, d + 14, 1);
                w32(&mut rsrc, d + 16, 0x409);
                w32(&mut rsrc, d + 20, (data_entries_off + gi * 16) as u32);
            }
            for k in 0..n_icons {
                let d = i_lang_off + k * lang_size;
                w16(&mut rsrc, d + 12, 0);
                w16(&mut rsrc, d + 14, 1);
                w32(&mut rsrc, d + 16, 0x409);
                w32(
                    &mut rsrc,
                    d + 20,
                    (data_entries_off + (n_groups + k) * 16) as u32,
                );
            }
            let mut gi = 0;
            for (raw_i, len) in group_raw_off
                .iter()
                .map(|o| raw_off + o)
                .zip(groups.iter().map(|(_, e)| 6 + e.len() * 14))
            {
                let d = data_entries_off + gi * 16;
                w32(&mut rsrc, d, (0x1000 + raw_i) as u32);
                w32(&mut rsrc, d + 4, len as u32);
                gi += 1;
            }
            let mut k = 0;
            for (_, entries) in groups {
                for (_, _, _, _, bytes) in entries {
                    let d = data_entries_off + (n_groups + k) * 16;
                    w32(&mut rsrc, d, (0x1000 + raw_off + icon_raw_off[k]) as u32);
                    w32(&mut rsrc, d + 4, bytes.len() as u32);
                    k += 1;
                }
            }
            rsrc[raw_off..].copy_from_slice(&raw);
            let mut pe = vec![0u8; 0x200];
            pe[0..2].copy_from_slice(b"MZ");
            pe[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
            pe[0x80..0x84].copy_from_slice(b"PE\0\0");
            pe[0x86..0x88].copy_from_slice(&1u16.to_le_bytes());
            pe[0x94..0x96].copy_from_slice(&240u16.to_le_bytes());
            pe[0x98..0x9A].copy_from_slice(&0x20Bu16.to_le_bytes());
            pe[0x98 + 60..0x98 + 64].copy_from_slice(&0x200u32.to_le_bytes());
            pe[0x98 + 108..0x98 + 112].copy_from_slice(&16u32.to_le_bytes());
            pe[0x98 + 112 + 16..0x98 + 112 + 20].copy_from_slice(&0x1000u32.to_le_bytes());
            pe[0x98 + 112 + 20..0x98 + 112 + 24].copy_from_slice(&(rsrc_len as u32).to_le_bytes());
            let sect = 0x80 + 24 + 240;
            pe[sect..sect + 5].copy_from_slice(b".rsrc");
            pe[sect + 8..sect + 12].copy_from_slice(&(rsrc_len as u32).to_le_bytes());
            pe[sect + 12..sect + 16].copy_from_slice(&0x1000u32.to_le_bytes());
            pe[sect + 16..sect + 20].copy_from_slice(&(rsrc_len as u32).to_le_bytes());
            pe[sect + 20..sect + 24].copy_from_slice(&0x200u32.to_le_bytes());
            pe.extend_from_slice(&rsrc);
            pe
        }

        #[test]
        fn is_png_checks_the_magic_signature() {
            assert!(is_png(&fake_png(256, 256)));
            assert!(!is_png(b"BM........"));
            assert!(!is_png(b""));
            assert!(!is_png(&[0x89, 0x50, 0x4E, 0x47]));
        }

        #[test]
        fn png_dimensions_reads_ihdr() {
            assert_eq!(png_dimensions(&fake_png(256, 128)), Some((256, 128)));
            assert_eq!(png_dimensions(&fake_png(64, 64)), Some((64, 64)));
            assert_eq!(png_dimensions(b"BM........"), None);
            assert_eq!(png_dimensions(&fake_png(256, 256)[..20]), None);
        }

        #[test]
        fn largest_png_entry_is_picked() {
            let big = fake_png(256, 256);
            let small_bmp = vec![b'B', b'M', 0, 0, 0, 0];
            let pe = build_pe(&[(
                1,
                vec![(0, 0, 32, 100, big.clone()), (32, 32, 24, 101, small_bmp)],
            )]);
            assert_eq!(extract_largest_icon_png(&pe), Some(big));
        }

        #[test]
        fn bmp_only_icons_return_none() {
            let bmp = vec![b'B', b'M', 0, 0, 0, 0];
            let pe = build_pe(&[(1, vec![(0, 0, 32, 100, bmp)])]);
            assert_eq!(extract_largest_icon_png(&pe), None);
        }

        #[test]
        fn largest_png_entry_is_picked_over_larger_bmp() {
            // The largest entry is BMP-only: the largest PNG is
            // promoted instead — a usable icon beats no icon.
            let bmp = vec![b'B', b'M', 0, 0, 0, 0];
            let png64 = fake_png(64, 64);
            let pe = build_pe(&[(
                1,
                vec![(0, 0, 32, 100, bmp), (64, 64, 32, 101, png64.clone())],
            )]);
            assert_eq!(extract_largest_icon_png(&pe), Some(png64));
        }

        #[test]
        fn too_small_png_returns_none_and_64px_passes() {
            let pe = build_pe(&[(1, vec![(32, 32, 32, 100, fake_png(32, 32))])]);
            assert_eq!(extract_largest_icon_png(&pe), None);
            let pe64 = build_pe(&[(1, vec![(64, 64, 32, 100, fake_png(64, 64))])]);
            assert!(extract_largest_icon_png(&pe64).is_some());
        }

        #[test]
        fn malformed_pe_returns_none() {
            assert_eq!(extract_largest_icon_png(b""), None);
            assert_eq!(extract_largest_icon_png(b"not a pe file"), None);
            assert_eq!(extract_largest_icon_png(b"MZ"), None);
            let mut bad = vec![0u8; 64];
            bad[0..2].copy_from_slice(b"MZ");
            bad[0x3C..0x40].copy_from_slice(&0x1000u32.to_le_bytes());
            assert_eq!(extract_largest_icon_png(&bad), None);
            let mut pe = build_pe(&[(1, vec![(0, 0, 32, 100, fake_png(256, 256))])]);
            pe[0x98 + 112 + 16..0x98 + 112 + 20].copy_from_slice(&0u32.to_le_bytes());
            assert_eq!(extract_largest_icon_png(&pe), None);
            assert_eq!(extract_largest_icon_png(&build_pe(&[])), None);
        }

        #[test]
        fn truncated_resource_returns_none_without_panicking() {
            let pe = build_pe(&[(1, vec![(0, 0, 32, 100, fake_png(256, 256))])]);
            for cut in [10, 0x100, 0x1B0, 0x200 + 20, 0x200 + 60, pe.len() - 5] {
                assert_eq!(
                    extract_largest_icon_png(&pe[..cut]),
                    None,
                    "cut at {cut} should return None"
                );
            }
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
            if name == filename && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
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

    /// Phrases in yt-dlp stderr that mean YouTube is bot-blocking this
    /// IP. When one matches, the session circuit-breaker skips all
    /// further YouTube lookups (v0.2.11). Mirror of `music.rs`.
    pub fn is_bot_block_error(stderr: &str) -> bool {
        let lower = stderr.to_lowercase();
        [
            "sign in to confirm you're not a bot",
            "confirm you're not a bot",
            "http error 429",
            "too many requests",
            "http error 403",
        ]
        .iter()
        .any(|p| lower.contains(p))
    }

    /// Hard timeout for a single yt-dlp call, seconds (v0.2.11:
    /// 8–10 s). Mirror of `music.rs` YTDLP_TIMEOUT_SECS.
    pub const YTDLP_TIMEOUT_SECS: u64 = 8;

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
        fn bot_block_error_matches_youtube_block_phrases() {
            assert!(is_bot_block_error(
                "ERROR: Sign in to confirm you're not a bot. Use --cookies-from-browser or --cookies"
            ));
            assert!(is_bot_block_error(
                "ERROR: HTTP Error 429: Too Many Requests"
            ));
            assert!(is_bot_block_error("ERROR: HTTP Error 403: Forbidden"));
            assert!(is_bot_block_error("too many requests, retrying in 5s"));
            assert!(!is_bot_block_error("ERROR: Video unavailable"));
            assert!(!is_bot_block_error(
                "ERROR: Unable to download video data: HTTP Error 404"
            ));
            assert!(!is_bot_block_error(""));
        }

        #[test]
        fn ytdlp_timeout_is_within_the_8_to_10s_budget() {
            assert!((8..=10).contains(&YTDLP_TIMEOUT_SECS));
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
                    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string()
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
    use std::collections::{HashMap, HashSet};
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
        // HARD denylist (v0.2.3): uninstallers / deleters / cleaners,
        // installers / prerequisites, crash / update helpers, servers.
        // `launcher` is NEVER filtered.
        const DENY: &[&str] = &[
            "uninstall",
            "unins",
            "uninst",
            "remove",
            "remover",
            "delete",
            "cleanup",
            "cleaner",
            "wipe",
            "purge",
            "reset",
            "repair",
            "modify",
            "install",
            "installer",
            "autorun",
            "bootstrap",
            "redist",
            "vcredist",
            "vc_redist",
            "dxsetup",
            "directx",
            "oalinst",
            "physx",
            "dotnet",
            "ue4prereq",
            "ueprereq",
            "crashreporter",
            "crashhandler",
            "unitycrashhandler",
            "crashpad",
            "bugsplat",
            "update",
            "updater",
            "benchmark",
            "config",
            "easyanticheat_setup",
            "battleye",
            "beservice",
            "dedicated",
            "server",
        ];
        if DENY.iter().any(|d| stem_lower.contains(d)) {
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
    // Exe picking: scoring (exe-guarantee round, v0.2.3). Mirror of the
    // same functions in src-tauri/src/stores.rs — keep in sync. The
    // Windows registry enumeration lives only in stores.rs (cfg
    // windows); its pure helpers are mirrored below.
    #[derive(Debug, Clone, Serialize)]
    pub struct RankedExe {
        pub exe_path: String,
        pub score: i32,
        pub source: String,
    }
    fn normalize_name(s: &str) -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_lowercase()
    }
    fn roman_to_arabic(word_lower: &str) -> Option<&'static str> {
        match word_lower {
            "i" => Some("1"),
            "ii" => Some("2"),
            "iii" => Some("3"),
            "iv" => Some("4"),
            "v" => Some("5"),
            "vi" => Some("6"),
            "vii" => Some("7"),
            "viii" => Some("8"),
            "ix" => Some("9"),
            "x" => Some("10"),
            _ => None,
        }
    }
    fn acronym_variants(folder_name: &str) -> (String, String) {
        let words: Vec<&str> = folder_name
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty() && *w != "s" && *w != "S")
            .collect();
        let mut raw = String::new();
        let mut normalized = String::new();
        for w in words {
            let lower = w.to_lowercase();
            if let Some(digit) = roman_to_arabic(&lower) {
                raw.push(lower.chars().next().unwrap_or('?'));
                normalized.push_str(digit);
            } else if lower.chars().all(|c| c.is_ascii_digit()) {
                raw.push_str(&lower);
                normalized.push_str(&lower);
            } else {
                let initial = lower.chars().next().unwrap_or('?');
                raw.push(initial);
                normalized.push(initial);
            }
        }
        (raw, normalized)
    }
    pub fn acronym_matches(stem_lower: &str, folder_name: &str) -> bool {
        let stem = normalize_name(stem_lower);
        if stem.is_empty() {
            return false;
        }
        let (raw, normalized) = acronym_variants(folder_name);
        (!raw.is_empty() && raw.len() >= 2 && (stem == raw || stem.starts_with(&raw)))
            || (!normalized.is_empty()
                && normalized.len() >= 2
                && (stem == normalized || stem.starts_with(&normalized)))
    }
    /// Bundled known-games table: normalized game title -> relative
    /// Windows exe paths (lowercase, `/` separators). Generated at build
    /// time by `scripts/generate_known_games.py` from the Discord
    /// detectable-apps API plus a small curated overrides file. Paths
    /// are matched by suffix. Loaded once per scan / store detection
    /// and dropped right after, so it costs 0 MB of idle RAM.
    /// (Mirror of `src-tauri/src/stores.rs` — keep in sync.)
    pub type KnownGames = HashMap<String, Vec<String>>;

    /// Parse the bundled known-games JSON. Malformed input yields an
    /// empty table — the scan continues on the other signals. Pure.
    pub fn parse_known_games(json: &str) -> KnownGames {
        serde_json::from_str(json).unwrap_or_default()
    }

    /// Load the bundled table. Unlike the Tauri crate (which embeds it
    /// with `include_str!`), the verify crate reads it from disk so the
    /// crate still compiles before the file is generated; a missing or
    /// unreadable file yields an empty table, and the generated-file
    /// tests below fail loudly on that.
    pub fn load_known_games() -> KnownGames {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src-tauri/known_games.json");
        std::fs::read_to_string(path)
            .map(|s| parse_known_games(&s))
            .unwrap_or_default()
    }

    /// Does this exe match a listed path for the folder's game in the
    /// known-games table? Pure — unit-tested.
    pub fn known_game_match(known: &KnownGames, folder_name: &str, exe: &Path) -> bool {
        let Some(paths) = known.get(&normalize_name(folder_name)) else {
            return false;
        };
        let cand = normalize_name(&exe.to_string_lossy());
        paths.iter().any(|rel| cand.ends_with(&normalize_name(rel)))
    }
    fn path_score(exe: &Path) -> i32 {
        let p = exe.to_string_lossy().replace('\\', "/").to_lowercase();
        let mut score = 0;
        if p.contains("binaries/win64") || p.contains("bin/x64") {
            score += 60;
        } else if p.contains("win64") {
            score += 40;
        } else if p.contains("/x64/") || p.contains("/x64") {
            score += 30;
        }
        if exe
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_lowercase().contains("shipping"))
            .unwrap_or(false)
        {
            score += 30;
        }
        const PENALIZED: &[&str] = &[
            "/engine/",
            "/tools/",
            "/redist",
            "/support/",
            "/plugins/",
            "/mods/",
            "/editor/",
            "/_commonredist/",
        ];
        if PENALIZED.iter().any(|seg| p.contains(seg)) {
            score -= 80;
        }
        score.min(90)
    }
    fn has_unity_data_dir(exe: &Path) -> bool {
        let Some(stem) = exe.file_stem().and_then(|s| s.to_str()) else {
            return false;
        };
        let Some(parent) = exe.parent() else {
            return false;
        };
        parent.join(format!("{stem}_Data")).is_dir()
    }
    pub fn score_exe(
        exe: &Path,
        folder_name: &str,
        shortcut: bool,
        registry: bool,
        known: &KnownGames,
    ) -> (i32, &'static str) {
        let stem = exe
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        let mut parts: Vec<(i32, &'static str)> = Vec::new();
        if shortcut {
            parts.push((200, "shortcut"));
        }
        if known_game_match(known, folder_name, exe) {
            parts.push((180, "known game"));
        }
        if registry {
            parts.push((150, "registry"));
        }
        if has_unity_data_dir(exe) {
            parts.push((100, "unity data"));
        }
        if acronym_matches(&stem, folder_name) {
            parts.push((90, "acronym"));
        }
        // v0.2.14 (mirror of stores.rs): an exe whose stem IS the
        // folder name (spaceless) is the game's own binary.
        // Generic folder names excluded (same list as
        // `game_name_for_group`).
        let generic = ["launcher", "game", "client", "main", "start", "play"];
        if normalize_name(&stem) == normalize_name(folder_name)
            && !generic.contains(&normalize_name(folder_name).as_str())
        {
            parts.push((90, "exact name"));
        }
        let path = path_score(exe);
        if path != 0 {
            parts.push((path, "path"));
        }
        let words: Vec<&str> = folder_name
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() >= 3)
            .collect();
        if words.iter().any(|w| stem.contains(&w.to_lowercase())) {
            parts.push((50, "name"));
        }
        let size = std::fs::metadata(exe).map(|m| m.len()).unwrap_or(0);
        if size > 0 && size < 1_000_000 {
            parts.push((-20, "tiny"));
        }
        let score: i32 = parts.iter().map(|(p, _)| p).sum();
        let source = parts
            .iter()
            .filter(|(p, _)| *p > 0)
            .max_by_key(|(p, _)| *p)
            .map(|(_, label)| *label)
            .unwrap_or("largest");
        (score, source)
    }
    pub fn confidence_of(score: i32) -> &'static str {
        if score >= 150 {
            "high"
        } else if score >= 60 {
            "medium"
        } else {
            "low"
        }
    }
    pub fn rank_exes(
        exes: &[PathBuf],
        folder_name: &str,
        shortcuts: &HashSet<String>,
        registry: &HashSet<String>,
        known: &KnownGames,
    ) -> Vec<RankedExe> {
        let mut scored: Vec<(RankedExe, u64)> = exes
            .iter()
            .map(|exe| {
                let key = exe.to_string_lossy().to_lowercase();
                let (score, source) = score_exe(
                    exe,
                    folder_name,
                    shortcuts.contains(&key),
                    registry.contains(&key),
                    known,
                );
                let size = std::fs::metadata(exe).map(|m| m.len()).unwrap_or(0);
                (
                    RankedExe {
                        exe_path: exe.to_string_lossy().into_owned(),
                        score,
                        source: source.to_string(),
                    },
                    size,
                )
            })
            .collect();
        scored.sort_by(|a, b| {
            b.0.score
                .cmp(&a.0.score)
                .then(b.1.cmp(&a.1))
                .then(a.0.exe_path.cmp(&b.0.exe_path))
        });
        scored.into_iter().map(|(r, _)| r).collect()
    }
    pub fn parse_lnk_target(bytes: &[u8]) -> Option<String> {
        if bytes.len() < 76 {
            return None;
        }
        if u32::from_le_bytes(bytes[0..4].try_into().ok()?) != 0x0000_004c {
            return None;
        }
        let flags = u32::from_le_bytes(bytes[20..24].try_into().ok()?);
        let has_id_list = flags & 0x0000_0001 != 0;
        let has_link_info = flags & 0x0000_0002 != 0;
        if !has_link_info {
            return None;
        }
        let mut off = 76usize;
        if has_id_list {
            if off + 2 > bytes.len() {
                return None;
            }
            let id_size = u16::from_le_bytes(bytes[off..off + 2].try_into().ok()?) as usize;
            off += 2 + id_size + 2;
        }
        if off + 28 > bytes.len() {
            return None;
        }
        let info_size = u32::from_le_bytes(bytes[off..off + 4].try_into().ok()?) as usize;
        if info_size < 28 || off + info_size > bytes.len() {
            return None;
        }
        let limit = off + info_size;
        let local_off = u32::from_le_bytes(bytes[off + 12..off + 16].try_into().ok()?) as usize;
        let unicode_off = u32::from_le_bytes(bytes[off + 24..off + 28].try_into().ok()?) as usize;
        let unicode_off2 = if info_size >= 36 {
            u32::from_le_bytes(bytes[off + 28..off + 32].try_into().ok()?) as usize
        } else {
            0
        };
        let read_ansi = |start: usize| -> Option<String> {
            if start == 0 || off + start >= limit {
                return None;
            }
            let mut end = off + start;
            while end < limit && bytes[end] != 0 {
                end += 1;
            }
            let s = String::from_utf8(bytes[off + start..end].to_vec()).ok()?;
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        };
        let read_unicode = |start: usize| -> Option<String> {
            if start == 0 || off + start + 1 >= limit {
                return None;
            }
            let mut units = Vec::new();
            let mut i = off + start;
            while i + 1 < limit {
                let u = u16::from_le_bytes(bytes[i..i + 2].try_into().ok()?);
                if u == 0 {
                    break;
                }
                units.push(u);
                i += 2;
            }
            let s = String::from_utf16(&units).ok()?;
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        };
        if let Some(s) = read_unicode(unicode_off) {
            return Some(s);
        }
        if let Some(s) = read_unicode(unicode_off2) {
            return Some(s);
        }
        read_ansi(local_off)
    }
    pub fn collect_shortcut_targets() -> HashSet<String> {
        let mut out = HashSet::new();
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Ok(profile) = std::env::var("USERPROFILE") {
            dirs.push(PathBuf::from(&profile).join("Desktop"));
        }
        if let Ok(public) = std::env::var("PUBLIC") {
            dirs.push(PathBuf::from(&public).join("Desktop"));
        }
        if let Ok(appdata) = std::env::var("APPDATA") {
            dirs.push(PathBuf::from(&appdata).join(r"Microsoft\Windows\Start Menu\Programs"));
        }
        if let Ok(progdata) = std::env::var("ProgramData") {
            dirs.push(PathBuf::from(&progdata).join(r"Microsoft\Windows\Start Menu\Programs"));
        }
        for dir in dirs {
            collect_lnks(&dir, 0, 3, &mut out);
        }
        out
    }
    fn collect_lnks(dir: &Path, depth: u32, max_depth: u32, out: &mut HashSet<String>) {
        if depth > max_depth {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
                continue;
            }
            let p = entry.path();
            if p.is_dir() {
                collect_lnks(&p, depth + 1, max_depth, out);
            } else if p
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("lnk"))
                == Some(true)
            {
                if let Ok(bytes) = std::fs::read(&p) {
                    if let Some(target) = parse_lnk_target(&bytes) {
                        let t = target.trim().to_string();
                        if t.to_lowercase().ends_with(".exe") && Path::new(&t).is_file() {
                            out.insert(t.to_lowercase());
                        }
                    }
                }
            }
        }
    }
    pub fn strip_icon_index(icon: &str) -> &str {
        let s = icon.trim();
        // Strip the index first, then any surrounding quotes: the value is
        // usually `"C:\Game\Game.exe",0` (quotes INSIDE the index).
        let s = match s.rfind(',') {
            Some(i) if !s[i + 1..].is_empty() && s[i + 1..].chars().all(|c| c.is_ascii_digit()) => {
                &s[..i]
            }
            _ => s,
        };
        s.trim_matches('"')
    }
    pub fn icon_points_inside(icon: &str, folder: &str) -> bool {
        let icon_norm = icon.replace('\\', "/").to_lowercase();
        let folder_norm = folder.replace('\\', "/").to_lowercase();
        let folder_norm = folder_norm.trim_end_matches('/');
        icon_norm.ends_with(".exe") && icon_norm.starts_with(&format!("{folder_norm}/"))
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
            // Hard denylist (v0.2.3): uninstallers, deleters, cleaners,
            // installers, prerequisites, servers.
            for stem in [
                "uninst",
                "remove",
                "remover",
                "delete",
                "cleanup",
                "cleaner",
                "wipe",
                "purge",
                "reset",
                "repair",
                "modify",
                "install",
                "installer",
                "autorun",
                "bootstrap",
                "directx",
                "oalinst",
                "physx",
                "bugsplat",
                "beservice",
                "dedicated",
                "server",
            ] {
                assert!(is_installer_exe(stem, &empty), "{stem}");
            }
            // Real games are never filtered — including Launcher.exe.
            assert!(!is_installer_exe("launcher", &empty));
            assert!(!is_installer_exe("fortniteclient-win64-shipping", &empty));
            assert!(!is_installer_exe("game", &empty));
            assert!(!is_installer_exe("setup_game", &empty));
            assert!(!is_installer_exe("remnant2", &empty));
            assert!(!is_installer_exe("code", &empty));
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

        #[test]
        fn acronym_matches_solves_cryptic_names() {
            assert!(acronym_matches("gta5", "Grand Theft Auto V"));
            assert!(acronym_matches("rdr2", "Red Dead Redemption 2"));
            assert!(acronym_matches("bg3", "Baldur's Gate 3"));
            assert!(acronym_matches("bg3_dx11", "Baldur's Gate 3"));
            assert!(acronym_matches("cod", "Call of Duty"));
            assert!(acronym_matches("tesv", "The Elder Scrolls V"));
            assert!(acronym_matches("cs2", "Counter-Strike 2"));
            assert!(!acronym_matches("witcher3", "The Witcher 3"));
            assert!(!acronym_matches("control", "Control"));
            assert!(!acronym_matches("doom", "DOOM Eternal"));
        }

        #[test]
        fn known_game_match_covers_famous_mismatches() {
            let known = parse_known_games(
                r#"{"cyberpunk2077":["bin/x64/cyberpunk2077.exe"],"grandtheftautov":["gta5.exe"],"reddeadredemption2":["rdr2.exe"],"baldursgate3":["bin/bg3.exe","bin/bg3_dx11.exe"],"thewitcher3":["bin/x64/witcher3.exe"],"thewitcher3wildhunt":["bin/x64/witcher3.exe"]}"#,
            );
            assert!(known_game_match(
                &known,
                "Cyberpunk 2077",
                Path::new(r"C:\Games\Cyberpunk 2077\bin\x64\Cyberpunk2077.exe")
            ));
            assert!(known_game_match(
                &known,
                "Grand Theft Auto V",
                Path::new(r"C:\Games\Grand Theft Auto V\GTA5.exe")
            ));
            assert!(known_game_match(
                &known,
                "Red Dead Redemption 2",
                Path::new(r"C:\Games\Red Dead Redemption 2\RDR2.exe")
            ));
            assert!(known_game_match(
                &known,
                "Baldur's Gate 3",
                Path::new(r"C:\Games\Baldur's Gate 3\bin\bg3.exe")
            ));
            assert!(known_game_match(
                &known,
                "Baldur's Gate 3",
                Path::new(r"C:\Games\Baldur's Gate 3\bin\bg3_dx11.exe")
            ));
            assert!(known_game_match(
                &known,
                "The Witcher 3: Wild Hunt",
                Path::new(r"C:\Games\The Witcher 3\bin\x64\witcher3.exe")
            ));
            assert!(!known_game_match(
                &known,
                "Some Random Game",
                Path::new(r"C:\Games\Some Random Game\game.exe")
            ));
        }

        #[test]
        fn generated_known_games_has_no_denylisted_exes_and_no_duplicates() {
            let known = load_known_games();
            assert!(
                !known.is_empty(),
                "known_games.json is missing or empty — run scripts/generate_known_games.py"
            );
            let mut pairs = 0;
            for (title, paths) in &known {
                assert!(!title.is_empty(), "empty title in known_games.json");
                let mut seen: HashSet<&str> = HashSet::new();
                for path in paths {
                    assert!(
                        seen.insert(path.as_str()),
                        "duplicate exe {path} for {title}"
                    );
                    assert!(path.ends_with(".exe"), "non-exe entry {path} for {title}");
                    let stem = path
                        .rsplit('/')
                        .next()
                        .unwrap_or(path.as_str())
                        .strip_suffix(".exe")
                        .unwrap_or(path.as_str());
                    assert!(
                        !is_installer_exe(stem, &[]),
                        "denylisted exe {path} for {title} in known_games.json"
                    );
                    pairs += 1;
                }
            }
            assert!(pairs > 10_000, "expected >10k exe entries, got {pairs}");
        }

        #[test]
        fn generated_known_games_covers_curated_titles() {
            let known = load_known_games();
            assert!(
                !known.is_empty(),
                "known_games.json is missing or empty — run scripts/generate_known_games.py"
            );
            let cp = known.get("cyberpunk2077").expect("cyberpunk2077 missing");
            assert!(cp.iter().any(|p| p == "bin/x64/cyberpunk2077.exe"));
            let gta = known
                .get("grandtheftautov")
                .expect("grandtheftautov missing");
            assert!(gta.iter().any(|p| p == "gta5.exe"));
            let bg3 = known.get("baldursgate3").expect("baldursgate3 missing");
            assert!(bg3.iter().any(|p| p == "bin/bg3.exe"));
            assert!(bg3.iter().any(|p| p == "bin/bg3_dx11.exe"));
            let witcher = known.get("thewitcher3").expect("thewitcher3 missing");
            assert!(witcher.iter().any(|p| p == "bin/x64/witcher3.exe"));
            assert!(
                known.len() > 10_000,
                "expected >10k titles, got {}",
                known.len()
            );
        }

        #[test]
        fn score_exe_ranks_known_game_and_unity_first() {
            let empty: HashSet<String> = HashSet::new();
            let known = parse_known_games(
                r#"{"cyberpunk2077":["bin/x64/cyberpunk2077.exe"],"grandtheftautov":["gta5.exe"],"baldursgate3":["bin/bg3.exe","bin/bg3_dx11.exe"]}"#,
            );
            let root = std::env::temp_dir().join("p4-verify-score-cp");
            let _ = std::fs::remove_dir_all(&root);
            let game = root.join("Cyberpunk 2077");
            std::fs::create_dir_all(game.join("bin").join("x64")).unwrap();
            std::fs::write(game.join("bin").join("x64").join("Cyberpunk2077.exe"), b"x").unwrap();
            std::fs::write(game.join("REDprelauncher.exe"), b"x").unwrap();
            let ranked = rank_exes(
                &[
                    game.join("REDprelauncher.exe"),
                    game.join("bin").join("x64").join("Cyberpunk2077.exe"),
                ],
                "Cyberpunk 2077",
                &empty,
                &empty,
                &known,
            );
            assert_eq!(ranked.len(), 2);
            assert!(
                ranked[0].exe_path.ends_with("Cyberpunk2077.exe"),
                "{ranked:?}"
            );
            assert_eq!(ranked[0].source, "known game");
            assert_eq!(confidence_of(ranked[0].score), "high");
            // GTA V: the known table wins (it is also an acronym match).
            let gta = root.join("Grand Theft Auto V");
            std::fs::create_dir_all(&gta).unwrap();
            std::fs::write(gta.join("GTA5.exe"), b"x").unwrap();
            std::fs::write(gta.join("PlayGTAV.exe"), b"xxxxxxxxxx").unwrap();
            let ranked = rank_exes(
                &[gta.join("PlayGTAV.exe"), gta.join("GTA5.exe")],
                "Grand Theft Auto V",
                &empty,
                &empty,
                &known,
            );
            assert!(ranked[0].exe_path.ends_with("GTA5.exe"), "{ranked:?}");
            assert_eq!(ranked[0].source, "known game");
            // Call of Duty: `cod.exe` may or may not be in the generated
            // table (Discord lists CoD titles); either way the pick is
            // the same exe — only the winning-signal label differs.
            let cod = root.join("Call of Duty");
            std::fs::create_dir_all(&cod).unwrap();
            std::fs::write(cod.join("cod.exe"), b"x").unwrap();
            std::fs::write(cod.join("PlayCOD.exe"), b"xxxxxxxxxx").unwrap();
            let ranked = rank_exes(
                &[cod.join("PlayCOD.exe"), cod.join("cod.exe")],
                "Call of Duty",
                &empty,
                &empty,
                &known,
            );
            assert!(ranked[0].exe_path.ends_with("cod.exe"), "{ranked:?}");
            assert!(matches!(
                ranked[0].source.as_str(),
                "known game" | "acronym"
            ));
            let hk = root.join("Hollow Knight");
            std::fs::create_dir_all(hk.join("Hollow Knight_Data")).unwrap();
            std::fs::write(hk.join("Hollow Knight.exe"), b"x").unwrap();
            std::fs::write(hk.join("Launcher.exe"), b"xxxxxxxxxx").unwrap();
            let ranked = rank_exes(
                &[hk.join("Launcher.exe"), hk.join("Hollow Knight.exe")],
                "Hollow Knight",
                &empty,
                &empty,
                &known,
            );
            assert!(
                ranked[0].exe_path.ends_with("Hollow Knight.exe"),
                "{ranked:?}"
            );
            assert_eq!(ranked[0].source, "unity data");
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn score_exe_exact_name_clears_false_warning_without_promoting_to_high() {
            // v0.2.14 (Round 2): Horizon Zero Dawn and Minecraft
            // showed a false "Not sure this is the right file"
            // warning — a bare exe stem equal to its folder name
            // earned only +50 (name) - 20 (tiny stub) = 30 ("low").
            // The new +90 "exact name" signal lifts both to
            // "medium" (no warning) while "high" (>= 150) stays
            // reserved for external evidence.
            let empty: HashSet<String> = HashSet::new();
            let known: KnownGames = HashMap::new();
            let root = std::env::temp_dir().join("p4-verify-score-exact-name");
            let _ = std::fs::remove_dir_all(&root);
            // Horizon stub: 90 + 50 - 20 = 120 -> medium.
            let hz = root.join("Horizon Zero Dawn");
            std::fs::create_dir_all(&hz).unwrap();
            std::fs::write(hz.join("HorizonZeroDawn.exe"), b"x").unwrap();
            let (score, source) = score_exe(
                &hz.join("HorizonZeroDawn.exe"),
                "Horizon Zero Dawn",
                false,
                false,
                &known,
            );
            assert_eq!((score, source), (120, "exact name"));
            assert_eq!(confidence_of(score), "medium");
            // Minecraft, real-size binary: 90 + 50 = 140 -> medium.
            let mc = root.join("Minecraft");
            std::fs::create_dir_all(&mc).unwrap();
            std::fs::write(mc.join("Minecraft.exe"), vec![0u8; 1_000_000]).unwrap();
            let (score, source) =
                score_exe(&mc.join("Minecraft.exe"), "Minecraft", false, false, &known);
            assert_eq!((score, source), (140, "exact name"));
            assert_eq!(confidence_of(score), "medium");
            // Generic folder names are excluded: a Launcher.exe in
            // a "Launcher" folder proves nothing (50 - 20 = 30).
            let lg = root.join("Launcher");
            std::fs::create_dir_all(&lg).unwrap();
            std::fs::write(lg.join("Launcher.exe"), b"x").unwrap();
            let (score, source) =
                score_exe(&lg.join("Launcher.exe"), "Launcher", false, false, &known);
            assert_eq!((score, source), (30, "name"));
            assert_eq!(confidence_of(score), "low");
            // Genuinely uncertain picks keep the warning: Fail Game
            // (fail.exe) and CoD MW2 (cod.exe, cryptic stem that
            // matches neither the acronym nor the folder name).
            let fg = root.join("Fail Game");
            std::fs::create_dir_all(&fg).unwrap();
            std::fs::write(fg.join("fail.exe"), b"x").unwrap();
            let (score, _) = score_exe(&fg.join("fail.exe"), "Fail Game", false, false, &known);
            assert_eq!(score, 30);
            assert_eq!(confidence_of(score), "low");
            let cod = root.join("Call of Duty Modern Warfare II");
            std::fs::create_dir_all(&cod).unwrap();
            std::fs::write(cod.join("cod.exe"), b"x").unwrap();
            let (score, _) = score_exe(
                &cod.join("cod.exe"),
                "Call of Duty Modern Warfare II",
                false,
                false,
                &known,
            );
            assert_eq!(score, -20);
            assert_eq!(confidence_of(score), "low");
            let _ = std::fs::remove_dir_all(&root);
        }

        /// Build a minimal synthetic `.lnk`: 76-byte header, optional
        /// IDList, then a LinkInfo block with the given target path.
        fn make_lnk(path: &str, with_id_list: bool, unicode: bool) -> Vec<u8> {
            let mut b = vec![0u8; 76];
            b[0..4].copy_from_slice(&0x0000_004cu32.to_le_bytes());
            let mut flags = 0x0000_0002u32; // HasLinkInfo
            if with_id_list {
                flags |= 0x0000_0001; // HasLinkTargetIDList
            }
            b[20..24].copy_from_slice(&flags.to_le_bytes());
            if with_id_list {
                b.extend_from_slice(&2u16.to_le_bytes()); // IDListSize = 2
                b.extend_from_slice(&2u16.to_le_bytes()); // one item, size 2
                b.extend_from_slice(&0u16.to_le_bytes()); // TerminalID
            }
            let ansi = path.as_bytes();
            let header_size: usize = if unicode { 36 } else { 28 };
            let uni_len = if unicode {
                path.encode_utf16().count() * 2 + 2
            } else {
                0
            };
            let info_size = header_size + ansi.len() + 1 + uni_len;
            let mut info = vec![0u8; info_size];
            info[0..4].copy_from_slice(&(info_size as u32).to_le_bytes());
            info[4..8].copy_from_slice(&1u32.to_le_bytes());
            info[12..16].copy_from_slice(&(header_size as u32).to_le_bytes());
            if unicode {
                let uni_off = header_size + ansi.len() + 1;
                info[24..28].copy_from_slice(&(uni_off as u32).to_le_bytes());
            }
            info[header_size..header_size + ansi.len()].copy_from_slice(ansi);
            if unicode {
                let uni_off = header_size + ansi.len() + 1;
                let mut j = uni_off;
                for u in path.encode_utf16() {
                    info[j..j + 2].copy_from_slice(&u.to_le_bytes());
                    j += 2;
                }
            }
            b.extend_from_slice(&info);
            b
        }

        #[test]
        fn parse_lnk_target_reads_local_base_path() {
            let target = r"C:\Games\Hades\Hades.exe";
            assert_eq!(
                parse_lnk_target(&make_lnk(target, false, false)).as_deref(),
                Some(target)
            );
            assert_eq!(
                parse_lnk_target(&make_lnk(target, true, false)).as_deref(),
                Some(target)
            );
            assert_eq!(
                parse_lnk_target(&make_lnk(target, true, true)).as_deref(),
                Some(target)
            );
            assert_eq!(parse_lnk_target(&[]), None);
            assert_eq!(parse_lnk_target(&[0u8; 100]), None);
            assert_eq!(parse_lnk_target(&make_lnk("", false, false)), None);
        }

        #[test]
        fn registry_helpers_strip_index_and_check_folder() {
            assert_eq!(strip_icon_index(r"C:\Game\Game.exe,0"), r"C:\Game\Game.exe");
            assert_eq!(
                strip_icon_index(r#""C:\Game\Game.exe",0"#),
                r"C:\Game\Game.exe"
            );
            assert_eq!(strip_icon_index(r"C:\Game\Game.exe"), r"C:\Game\Game.exe");
            assert_eq!(
                strip_icon_index(r"C:\Game\My, Game.exe"),
                r"C:\Game\My, Game.exe"
            );
            assert!(icon_points_inside(
                r"C:\Games\Hades\Hades.exe",
                r"C:\Games\Hades"
            ));
            assert!(icon_points_inside(
                r"C:\Games\Hades\bin\Hades.exe",
                r"C:\Games\Hades"
            ));
            assert!(!icon_points_inside(r"C:\Other\Game.exe", r"C:\Games\Hades"));
            assert!(!icon_points_inside(
                r"C:\Games\Hades\readme.txt",
                r"C:\Games\Hades"
            ));
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

    /// Fail-soft decision: report the launch only when the process
    /// actually exited (not still running) with a non-zero code. A
    /// fast exit with code 0 is never reported (handoff stubs exit 0
    /// by design), and a still-running process is not a failure.
    pub fn should_report_fast_exit(exited: bool, success: bool) -> bool {
        exited && !success
    }

    /// A persisted fast-exit notice stays fresh for 15 minutes. Older
    /// notices are dropped so a stale one never pops up on a later
    /// startup. A timestamp in the future (clock skew) counts as fresh.
    pub fn pending_fast_exit_is_fresh(created_at_unix: u64, now_unix: u64) -> bool {
        created_at_unix.saturating_add(900) >= now_unix
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

        #[test]
        fn fast_exit_reported_only_for_exited_nonzero() {
            assert!(should_report_fast_exit(true, false));
            assert!(!should_report_fast_exit(true, true));
            assert!(!should_report_fast_exit(false, false));
            assert!(!should_report_fast_exit(false, true));
        }

        #[test]
        fn pending_fast_exit_fresh_for_15_minutes() {
            let now = 1_700_000_000u64;
            assert!(pending_fast_exit_is_fresh(now, now));
            assert!(pending_fast_exit_is_fresh(now - 899, now));
            assert!(pending_fast_exit_is_fresh(now - 900, now));
            assert!(!pending_fast_exit_is_fresh(now - 901, now));
            assert!(!pending_fast_exit_is_fresh(0, now));
            // A timestamp in the future (clock skew) counts as fresh.
            assert!(pending_fast_exit_is_fresh(now + 60, now));
        }
    }
}

/// v0.2.9 window lifecycle (destroy on launch): the pure state
/// machine behind session.rs, so the decisions are unit-testable.
/// The Tauri window itself cannot be tested here.
pub mod session_pure {
    /// Which side of the v0.2.9 window lifecycle the launcher is in.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum LauncherWindow {
        Open,
        Destroyed,
    }

    /// "Minimize on launch" ON destroys the window on launch (the
    /// ~4 MB host stays in the tray); OFF keeps the window open.
    pub fn window_after_launch(minimize_on_launch: bool) -> LauncherWindow {
        if minimize_on_launch {
            LauncherWindow::Destroyed
        } else {
            LauncherWindow::Open
        }
    }

    /// How many times window recreation is retried before giving up
    /// (the tray icon stays as the manual fallback after that).
    pub const MAX_RECREATE_ATTEMPTS: u32 = 3;

    /// Recreate (or show) the window when a game exits — but only if
    /// it is actually gone, and only for a bounded number of tries.
    pub fn should_recreate_window(window: LauncherWindow, attempts: u32) -> bool {
        window == LauncherWindow::Destroyed && attempts < MAX_RECREATE_ATTEMPTS
    }

    /// v0.2.10: prevent the runtime's exit request when the last
    /// window is destroyed (code None) — the tray host must stay
    /// alive to recreate the window on game exit. An explicit exit
    /// (code Some, e.g. the tray "Quit" item) goes through.
    /// v0.2.11: the same exit is a real quit when NO game is running
    /// (Alt+F4 / window close — the window is borderless, so this is
    /// the only visible way out besides tray / Settings / Power).
    pub fn should_prevent_exit(exit_code: Option<i32>, game_running: bool) -> bool {
        exit_code.is_none() && game_running
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
            // code None + game running: managed launch destroyed the
            // window — the host must stay alive in the tray.
            assert!(should_prevent_exit(None, true));
            // code None + NO game running: Alt+F4 / window close — a
            // real quit (v0.2.11).
            assert!(!should_prevent_exit(None, false));
            // code Some: explicit exit (tray "Quit", Settings / Power
            // "Exit Launcher") — always goes through.
            assert!(!should_prevent_exit(Some(0), true));
            assert!(!should_prevent_exit(Some(0), false));
            assert!(!should_prevent_exit(Some(1), true));
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
