//! KHInsider game-soundtrack lookup (fallback music source).
//!
//! Flow: search `downloads.khinsider.com` for the game -> open the best
//! album -> pick the theme track -> resolve the direct MP3 URL on the
//! `vgmtreasurechest.com` CDN -> download it into the music cache.
//!
//! KHInsider is behind Cloudflare, which challenges non-browser clients.
//! Every step degrades to `Ok(None)` on any failure (403, parse miss,
//! network error) — the caller falls through to the next source, so a
//! blocked KHInsider never breaks the music chain. On a residential
//! connection the requests usually pass; on a flagged datacenter IP they
//! are challenged and this source is simply skipped.

use crate::http;
use crate::music;

/// One KHInsider track: the direct MP3 URL + a display title.
#[derive(Debug, Clone, serde::Serialize)]
pub struct KhTrack {
    pub url: String,
    pub title: String,
}

/// Search KHInsider for a game and return the best theme track (direct
/// MP3 URL). `Ok(None)` when nothing is found or any step fails.
pub fn search_khinsider_track(game_name: &str) -> Result<Option<KhTrack>, String> {
    let clean = music::sanitize_game_title(game_name);
    let query = http::url_encode(&clean);
    let search_url = format!("https://downloads.khinsider.com/search?search={query}");
    let html = match http::download_bytes(&search_url) {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(_) => return Ok(None),
    };
    let Some(slug) = find_album_slug(&html, &clean) else {
        return Ok(None);
    };
    let album_url = format!("https://downloads.khinsider.com/game-soundtracks/album/{slug}");
    let album_html = match http::download_bytes(&album_url) {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(_) => return Ok(None),
    };
    let Some((track_page, title)) = find_best_track(&album_html, &slug) else {
        return Ok(None);
    };
    let track_html = match http::download_bytes(&track_page) {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(_) => return Ok(None),
    };
    let Some(mp3) = find_direct_mp3(&track_html) else {
        return Ok(None);
    };
    Ok(Some(KhTrack { url: mp3, title }))
}

/// Download a KHInsider track (direct MP3 URL) into the music cache.
/// Returns the cached path.
pub fn download_khinsider_track(
    app: &tauri::AppHandle,
    game_key: &str,
    track: &KhTrack,
) -> Result<String, String> {
    let dir = music::music_dir(app)?;
    if let Some(existing) = music::music_file_for_key(&dir, game_key) {
        return Ok(existing.to_string_lossy().into_owned());
    }
    let dest = dir.join(format!("{game_key}.mp3"));
    http::download_to(&track.url, &dest).map_err(|e| format!("khinsider download failed: {e}"))?;
    // Reject an HTML error page masquerading as an MP3.
    let is_audio = std::fs::read(&dest)
        .map(|b| {
            b.len() > 1024
                && !b.starts_with(b"<!DOCTYPE")
                && !b.starts_with(b"<html")
                && (b.starts_with(&[0xFF, 0xFB])
                    || b.starts_with(&[0xFF, 0xF3])
                    || b.starts_with(&[0xFF, 0xF2])
                    || b.starts_with(b"ID3"))
        })
        .unwrap_or(false);
    if !is_audio {
        let _ = std::fs::remove_file(&dest);
        return Err("khinsider returned a non-audio file".to_string());
    }
    crate::db::save_music(
        app,
        game_key,
        "khinsider",
        &dest.to_string_lossy(),
        &track.title,
    )?;
    Ok(dest.to_string_lossy().into_owned())
}

/// Percent-decode a URL component (`%20` -> space, `+` -> space).
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
                out.push(bytes[i]);
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

/// Find the best album slug on a KHInsider search page. Prefers an album
/// whose name matches the game; falls back to the first album link.
pub fn find_album_slug(html: &str, game_name: &str) -> Option<String> {
    let needle = "href=\"/game-soundtracks/album/";
    let want = normalize(game_name);
    let mut first: Option<String> = None;
    let mut rest = html;
    while let Some(pos) = rest.find(needle) {
        let after = &rest[pos + needle.len()..];
        let end = after.find('"')?;
        let slug = &after[..end];
        // Link text: the next `>` after the href, up to `</a>`.
        let text = after[end..]
            .find('>')
            .and_then(|gt| {
                let t = &after[end + gt + 1..];
                t.find("</a>").map(|e| t[..e].to_string())
            })
            .unwrap_or_default();
        let text = text.trim().to_string();
        if first.is_none() {
            first = Some(slug.to_string());
        }
        let norm = normalize(&text);
        if !want.is_empty() && (norm == want || norm.starts_with(&want) || want.starts_with(&norm))
        {
            return Some(slug.to_string());
        }
        rest = &after[end..];
    }
    first
}

/// Pick the best theme track on an album page. Returns the track-page URL
/// (on downloads.khinsider.com) + a display title. Prefers a track whose
/// name sounds like a main theme; otherwise the first track.
pub fn find_best_track(html: &str, slug: &str) -> Option<(String, String)> {
    let prefix = format!("href=\"/game-soundtracks/album/{slug}/");
    let mut candidates: Vec<(String, String)> = Vec::new();
    let mut rest = html;
    while let Some(pos) = rest.find(&prefix) {
        let after = &rest[pos + prefix.len()..];
        let end = match after.find('"') {
            Some(e) => e,
            None => break,
        };
        let file = &after[..end];
        if file.to_lowercase().ends_with(".mp3") {
            let title = track_title_from_file(file);
            let page =
                format!("https://downloads.khinsider.com/game-soundtracks/album/{slug}/{file}");
            if !candidates.iter().any(|(u, _)| u == &page) {
                candidates.push((page, title));
            }
        }
        rest = &after[end..];
    }
    if candidates.is_empty() {
        return None;
    }
    // Prefer a main-theme-like track.
    for (url, title) in &candidates {
        let lower = title.to_lowercase();
        if lower.contains("main theme")
            || lower.contains("title theme")
            || lower.contains("title screen")
            || lower == "theme"
            || lower.contains("opening theme")
        {
            return Some((url.clone(), title.clone()));
        }
    }
    // Else the first track (usually the main theme).
    candidates.into_iter().next()
}

/// Turn a track filename (`01.%20Enter%20Hallownest.mp3`) into a title.
fn track_title_from_file(file: &str) -> String {
    let decoded = percent_decode(file);
    let base = decoded
        .rsplit('/')
        .next()
        .unwrap_or(&decoded)
        .trim_end_matches(".mp3")
        .trim_end_matches(".MP3");
    // Strip a leading track number ("01. " / "01 - ").
    let mut s = base.trim();
    if s.len() > 3 {
        let b = s.as_bytes();
        if b[0].is_ascii_digit() && b[1].is_ascii_digit() && (b[2] == b'.' || b[2] == b'-') {
            s = s[3..].trim_start_matches(|c| c == ' ' || c == '-');
        }
    }
    s.to_string()
}

/// Find the direct MP3 URL on a KHInsider track page (the
/// `vgmtreasurechest.com` CDN link).
pub fn find_direct_mp3(html: &str) -> Option<String> {
    let marker = "vgmtreasurechest.com/soundtracks/";
    let pos = html.find(marker)?;
    // Walk back to the start of the URL (http:// or https://).
    let before = &html[..pos];
    let start = before
        .rfind("https://")
        .or_else(|| before.rfind("http://"))?;
    let after = &html[start..];
    let end = after.find(".mp3")? + 4;
    let url = &after[..end];
    // The URL must not contain whitespace or quotes.
    if url
        .chars()
        .any(|c| c.is_whitespace() || c == '"' || c == '\'')
    {
        return None;
    }
    Some(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decode_handles_spaces_and_plus() {
        assert_eq!(
            percent_decode("01.%20Enter%20Hallownest.mp3"),
            "01. Enter Hallownest.mp3"
        );
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(percent_decode("plain"), "plain");
    }

    #[test]
    fn find_album_slug_prefers_exact_name_match() {
        let html = r#"<a href="/game-soundtracks/album/hollow-knight">Hollow Knight</a>
            <a href="/game-soundtracks/album/hollow-knight-gamerip">Hollow Knight</a>"#;
        assert_eq!(
            find_album_slug(html, "Hollow Knight"),
            Some("hollow-knight".to_string())
        );
        // Falls back to the first link when nothing matches.
        let html2 = r#"<a href="/game-soundtracks/album/other-game">Other Game</a>"#;
        assert_eq!(
            find_album_slug(html2, "Hollow Knight"),
            Some("other-game".to_string())
        );
        assert_eq!(find_album_slug("no links here", "Hollow Knight"), None);
    }

    #[test]
    fn find_best_track_prefers_main_theme() {
        let html = r#"<a href="/game-soundtracks/album/g/01.%20Enter%20Hallownest.mp3">dl</a>
            <a href="/game-soundtracks/album/g/02.%20Dirtmouth.mp3">dl</a>"#;
        let (url, title) = find_best_track(html, "g").unwrap();
        assert!(url.contains("01."));
        assert_eq!(title, "Enter Hallownest");
        let html2 = r#"<a href="/game-soundtracks/album/g/01.%20Intro.mp3">dl</a>
            <a href="/game-soundtracks/album/g/02.%20Main%20Theme.mp3">dl</a>"#;
        let (url2, title2) = find_best_track(html2, "g").unwrap();
        assert!(url2.contains("02."));
        assert_eq!(title2, "Main Theme");
        assert_eq!(find_best_track("nothing", "g"), None);
    }

    #[test]
    fn find_direct_mp3_extracts_cdn_url() {
        let html = r#"<a href="https://vgmtreasurechest.com/soundtracks/hollow-knight-original-soundtrack/fvjldhcq/01.%20Enter%20Hallownest.mp3">Click here to download as MP3</a>"#;
        assert_eq!(
            find_direct_mp3(html),
            Some("https://vgmtreasurechest.com/soundtracks/hollow-knight-original-soundtrack/fvjldhcq/01.%20Enter%20Hallownest.mp3".to_string())
        );
        assert_eq!(find_direct_mp3("no mp3 here"), None);
    }
}
