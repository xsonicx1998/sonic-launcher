//! Game art auto-download (Phase 2).
//!
//! Primary source: the free Steam CDN (no key needed) — per appid:
//! - tile:    `.../steam/apps/{appid}/library_600x900.jpg`
//!            (portrait box art; `header.jpg` as fallback)
//! - hero:    `.../steam/apps/{appid}/library_hero.jpg`
//! - logo:    `.../steam/apps/{appid}/logo.png` (v0.2.8, PNG only;
//!            a 404 is recorded and not re-fetched for 14 days)
//! - header:  `.../steam/apps/{appid}/header.jpg` (16:9 card +
//!            tile fallback when the capsule is missing)
//!
//! Optional source: SteamGridDB, when the user saved their own API key
//! in Settings (stored in the settings table, never hardcoded, never
//! logged). On any SteamGridDB failure we fall back to the Steam CDN (or
//! to no art for non-Steam games).
//!
//! Keyless source (v0.2.5): for scanned games with no Steam appid and
//! no saved SGDB key, the folder name is resolved through the free
//! Steam store search, then the free GOG Catalog as a secondary
//! fallback — no key, no login. Matching is strict (exact match, or
//! >= 0.85 similarity with a >= 0.05 margin; junk like soundtracks
//! and demos is filtered), hits and misses are cached in SQLite
//! (`resolved_appid` / `art_lookup_failed_at` / `art_source` on
//! `scanned_games`), and lookups run in a background worker after a
//! scan finishes. Only a numeric Steam id is ever taken from the
//! store search JSON; GOG covers come from GOG's own image host
//! only. A manual cover (picked file, typed AppID, pasted image
//! URL) is never overwritten by an automatic lookup.
//!
//! Last resort (v0.2.6): the game's own .exe icon, extracted
//! with pure-Rust PE parsing (see `exe_icon.rs`) — only PNG
//! icons >= 64 px, shown centered on the Sonic gradient card,
//! never full-bleed. Cached in the art dir as `<key>-icon.png`
//! and recorded as art_source "icon", so it is never
//! re-extracted on every launch and never overwrites a manual
//! cover or a resolved store lookup.
//!
//! Files are cached in `%APPDATA%\com.sonic.launcher\art\` and never
//! re-downloaded while the cache file exists. The frontend gets both the
//! local path (for `convertFileSrc`) and a base64 data URI (fallback when
//! the asset protocol cannot serve the file).

use base64::Engine;
use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

use crate::db;
use crate::http;

/// Art for one game, as exposed to the frontend.
#[derive(Debug, Clone, Default, Serialize)]
pub struct GameArt {
    /// Local path of the tile image (Steam header or SGDB grid) —
    /// the fallback when the portrait capsule is missing.
    pub tile_path: Option<String>,
    /// Local path of the hero background image.
    pub hero_path: Option<String>,
    /// Local path of the portrait capsule image (Steam 600x900 or
    /// SGDB grid) — the tile since v0.2.8.
    pub capsule_path: Option<String>,
    /// Local path of the Steam game logo (logo.png, v0.2.8).
    /// `None` when the game has no Steam logo (or no Steam appid).
    pub logo_path: Option<String>,
    /// Base64 data URI of the tile (fallback when convertFileSrc fails).
    pub tile_data_uri: Option<String>,
    /// Base64 data URI of the hero (fallback when convertFileSrc fails).
    pub hero_data_uri: Option<String>,
    /// Base64 data URI of the portrait capsule (the tile, v0.2.8).
    pub capsule_data_uri: Option<String>,
    /// Base64 data URI of the logo (v0.2.8).
    pub logo_data_uri: Option<String>,
    /// `true` when the tile is the game's own .exe icon (v0.2.6):
    /// the frontend renders it centered on the Sonic gradient
    /// card instead of stretched full-bleed like a real cover.
    pub is_icon: bool,
}

/// The art cache directory: `%APPDATA%\com.sonic.launcher\art`.
pub fn art_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("cannot resolve app data dir: {e}"))?
        .join("art");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create art dir: {e}"))?;
    Ok(dir)
}

/// The four Steam CDN URLs for one appid. Pure — unit-tested.
pub fn steam_cdn_urls(appid: u32) -> (String, String, String, String) {
    (
        format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/header.jpg"),
        format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/library_hero.jpg"),
        format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/library_600x900.jpg"),
        format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/logo.png"),
    )
}

fn safe_key(game_key: &str) -> String {
    // game_key is `steam-<appid>` / `manual-<id>` / `epic-<slug>` etc. —
    // safe for a file name (letters, digits, dashes).
    game_key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn file_name(game_key: &str, kind: &str) -> String {
    format!("{}-{kind}.jpg", safe_key(game_key))
}

/// Cache file for an extracted .exe icon (v0.2.6): PNG, not JPG,
/// so the icon's transparency survives.
fn icon_file_name(game_key: &str) -> String {
    format!("{}-icon.png", safe_key(game_key))
}

/// Cache file for a Steam game logo (v0.2.8): PNG, so the logo's
/// transparency survives.
fn logo_file_name(game_key: &str) -> String {
    format!("{}-logo.png", safe_key(game_key))
}

/// PNG magic signature (89 50 4E 47 0D 0A 1A 0A). Pure — unit-tested.
pub fn is_png_bytes(b: &[u8]) -> bool {
    b.len() >= 8 && b[..8] == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
}

fn mime_for(path: &PathBuf) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        _ => "image/jpeg",
    }
}

/// Read a file as a base64 data URI. `None` when the file is missing or
/// unreadable — the caller treats that as "no art".
fn data_uri(path: &PathBuf) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.is_empty() {
        return None;
    }
    let mime = mime_for(path);
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    Some(format!("data:{mime};base64,{b64}"))
}

fn path_str(p: &PathBuf) -> Option<String> {
    if p.is_file() {
        Some(p.to_string_lossy().into_owned())
    } else {
        None
    }
}

fn build_art(tile: &PathBuf, hero: &PathBuf, capsule: &PathBuf, logo: &PathBuf) -> GameArt {
    GameArt {
        tile_path: path_str(tile),
        hero_path: path_str(hero),
        capsule_path: path_str(capsule),
        logo_path: path_str(logo),
        tile_data_uri: data_uri(tile),
        hero_data_uri: data_uri(hero),
        capsule_data_uri: data_uri(capsule),
        logo_data_uri: data_uri(logo),
        is_icon: false,
    }
}

/// Build the art for an extracted .exe icon (v0.2.6): the icon is
/// the tile (and the capsule); there is no hero — the frontend
/// keeps the gradient background instead of stretching the icon.
fn build_icon_art(icon: &PathBuf) -> GameArt {
    GameArt {
        tile_path: path_str(icon),
        hero_path: None,
        capsule_path: path_str(icon),
        logo_path: None,
        tile_data_uri: data_uri(icon),
        hero_data_uri: None,
        capsule_data_uri: data_uri(icon),
        logo_data_uri: None,
        is_icon: true,
    }
}

/// Last-resort cover art (v0.2.6): the game's own .exe icon.
/// A cached icon file is reused without re-parsing the exe; a
/// fresh extraction is cached in the art dir and recorded as
/// art_source "icon" for scanned games. Returns `None` when
/// there is no exe path or the exe has no usable PNG icon.
fn try_exe_icon(app: &AppHandle, game_key: &str, exe_path: Option<&str>) -> Option<GameArt> {
    let Ok(dir) = art_dir(app) else {
        return None;
    };
    let icon = dir.join(icon_file_name(game_key));
    if let Ok(meta) = std::fs::metadata(&icon) {
        if meta.len() > 0 {
            return Some(build_icon_art(&icon));
        }
    }
    let exe = exe_path?;
    let bytes = crate::exe_icon::extract_exe_icon_png(std::path::Path::new(exe))?;
    std::fs::write(&icon, &bytes).ok()?;
    if let Some(id) = db::scanned_id_from_key(game_key) {
        let _ = db::set_art_source(app, id, "icon");
    }
    Some(build_icon_art(&icon))
}

// ---------------------------------------------------------------------------
// SteamGridDB (optional, user key only).
// ---------------------------------------------------------------------------

const SGDB_API: &str = "https://www.steamgriddb.com/api/v2";

/// Parse a SteamGridDB search-autocomplete response into
/// `(game_id, name)` pairs. Pure — unit-tested.
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

/// Parse a SteamGridDB grids/heroes response into image URLs. Pure.
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

fn sgdb_auth(key: &str) -> String {
    format!("Bearer {key}")
}

/// Search SteamGridDB for a game by name. Returns `(game_id, name)` pairs.
fn sgdb_search(key: &str, name: &str) -> Result<Vec<(u64, String)>, String> {
    let url = format!(
        "{SGDB_API}/search/autocomplete/{}",
        crate::http::url_encode(name)
    );
    let json = crate::http::download_json(&url, Some(&sgdb_auth(key)))?;
    Ok(parse_sgdb_search(&json))
}

/// Fetch the first grid (portrait) and first hero image URL for a
/// SteamGridDB game id.
fn sgdb_images(key: &str, game_id: u64) -> Result<(Option<String>, Option<String>), String> {
    let auth = sgdb_auth(key);
    let grids = crate::http::download_json(
        &format!("{SGDB_API}/grids/game/{game_id}?limit=1"),
        Some(&auth),
    )
    .map(|j| parse_sgdb_image_urls(&j).into_iter().next())
    .unwrap_or(None);
    let heroes = crate::http::download_json(
        &format!("{SGDB_API}/heroes/game/{game_id}?limit=1"),
        Some(&auth),
    )
    .map(|j| parse_sgdb_image_urls(&j).into_iter().next())
    .unwrap_or(None);
    Ok((grids, heroes))
}

// ---------------------------------------------------------------------------
// Keyless cover lookup (v0.2.5): Steam store search, then GOG Catalog.
// ---------------------------------------------------------------------------

/// How long a failed keyless lookup blocks automatic retries (days).
/// The user can always force a retry from the ••• menu ("Retry Art").
pub const ART_LOOKUP_RETRY_DAYS: i64 = 14;

/// `true` when a failed lookup at `failed_at` (unix seconds) still
/// blocks an automatic retry at `now`. Pure — unit-tested.
pub fn art_lookup_blocked(failed_at: Option<i64>, now: i64) -> bool {
    match failed_at {
        Some(t) => now.saturating_sub(t) < ART_LOOKUP_RETRY_DAYS * 24 * 3600,
        None => false,
    }
}

/// Edition phrases stripped from the end of a game name before a
/// store search. Longest first — "game of the year edition" must be
/// tried before "game of the year".
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
    // Bare "enhanced" (v0.2.13): Steam replaced the original
    // "Grand Theft Auto V" listing with "Grand Theft Auto V
    // Enhanced" — without this, the strict matcher scores the
    // Enhanced listing 0.800 and rejects it. After "enhanced
    // edition" so the longer phrase is tried first.
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

/// Clean a folder/game name for a store search: drop ™®©, bracketed
/// tags like `[Repack]` / `(2024)`, edition suffixes, and turn
/// `_` `.` `:` `-` `,` into spaces. Pure — unit-tested.
pub fn sanitize_game_name(name: &str) -> String {
    let mut s = name.replace(['\u{2122}', '\u{00AE}', '\u{00A9}'], "");
    // Drop [...] and (...) groups (repack tags, years, notes).
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
    // Strip trailing edition phrases, longest first, repeatedly
    // ("Definitive Edition" after "Game of the Year Edition").
    loop {
        let trimmed = s.trim_end();
        let mut stripped = false;
        for suffix in EDITION_SUFFIXES {
            if let Some(base) = trimmed.strip_suffix(suffix) {
                // Only strip a whole trailing phrase: the char before
                // it must be a space (or the string start).
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

/// A whole word that is a Roman numeral, as an Arabic digit.
/// Only whole words count — `vi` inside `civil` is not a numeral.
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

/// Normalize a name for comparison: lowercase, punctuation to word
/// breaks, Roman numerals to digits, single spaces. Pure.
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

/// Single-word junk markers: a candidate whose name has one of these
/// words is not the game itself (soundtrack, demo, ...).
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
/// Multi-word junk markers (checked as phrases).
const JUNK_PHRASES: &[&str] = &["dedicated server", "season pass"];

/// `true` when a candidate title is junk for this game — its name
/// carries a soundtrack/demo/server/... marker that the folder name
/// itself does not have. Words match whole tokens only, so "ghost"
/// never trips the "ost" marker. Pure — unit-tested.
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

/// Token-set (Jaccard) similarity of two normalized names, 0.0–1.0.
/// Pure — unit-tested.
pub fn token_similarity(a_norm: &str, b_norm: &str) -> f64 {
    let a: std::collections::HashSet<&str> = a_norm.split(' ').filter(|t| !t.is_empty()).collect();
    let b: std::collections::HashSet<&str> = b_norm.split(' ').filter(|t| !t.is_empty()).collect();
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(&b).count() as f64;
    let union = a.union(&b).count() as f64;
    inter / union
}

/// Pick the best candidate for a folder name, or `None`:
/// - Tier 1: a candidate whose normalized name equals the folder's —
///   picked immediately, whatever else is in the list.
/// - Tier 2: best token similarity >= 0.85, beating #2 by >= 0.05.
/// A missing cover is better than the wrong cover, so ties and
/// near-ties return `None`. Pure — unit-tested.
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
            return Some(i); // Tier 1: exact match wins immediately.
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
            return None; // too close to call
        }
    }
    Some(top_i)
}

/// Parse a Steam storesearch response into `(appid, name)` pairs,
/// keeping only `type == "app"`. All fields are `Option` — a Steam
/// JSON schema tweak never panics. Pure — unit-tested.
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

/// One GOG Catalog product, as far as art is concerned.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GogCover {
    pub title: String,
    /// Portrait cover (600x900-ish) — the tile and the capsule.
    pub vertical: Option<String>,
    /// Wide cover (16:9) — the hero background.
    pub horizontal: Option<String>,
}

/// Parse a GOG Catalog response into products, keeping only
/// `productType` "game" or "pack" (never "dlc" / "extras") and only
/// products that have at least one cover URL. All fields are
/// `Option` — a GOG JSON schema tweak never panics. Pure.
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

/// Validate a user-pasted image URL: https only, no whitespace
/// (same strictness as the Paste Music URL check). Pure.
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

/// Failure of one keyless lookup attempt.
pub enum LookupError {
    /// HTTP 429 or 403 — stop the session's remaining lookups.
    RateLimited,
    /// Anything else (network, timeout, oversize, bad JSON).
    Failed(String),
}

const STORESEARCH_API: &str = "https://store.steampowered.com/api/storesearch/";
const GOG_CATALOG_API: &str = "https://catalog.gog.com/v1/catalog";
/// 5 s timeout and 512 KB cap for the small JSON lookups.
const LOOKUP_TIMEOUT_SECS: u64 = 5;
const LOOKUP_MAX_BYTES: usize = 512 * 1024;

/// One capped GET with a single retry on a plain failure (never on a
/// rate limit). Returns the body bytes, or the error.
fn lookup_get(url: &str) -> Result<Vec<u8>, LookupError> {
    for attempt in 0..2 {
        match crate::http::get_capped(url, LOOKUP_TIMEOUT_SECS, LOOKUP_MAX_BYTES) {
            crate::http::CappedGet::Ok(bytes) => return Ok(bytes),
            crate::http::CappedGet::RateLimited => return Err(LookupError::RateLimited),
            crate::http::CappedGet::Failed(e) => {
                if attempt == 1 {
                    return Err(LookupError::Failed(e));
                }
                // One retry on a network error, then give up.
            }
        }
    }
    Err(LookupError::Failed("lookup failed".to_string()))
}

/// Resolve a folder name to a Steam appid via the free store search.
/// Only the numeric id is taken from the JSON — the art itself always
/// comes from the hardcoded Steam CDN URLs.
pub fn resolve_steam_appid(name: &str) -> Result<Option<u32>, LookupError> {
    let clean = sanitize_game_name(name);
    if clean.is_empty() {
        return Ok(None);
    }
    let url = format!(
        "{STORESEARCH_API}?term={}&l=english&cc=US",
        crate::http::url_encode(&clean)
    );
    let bytes = lookup_get(&url)?;
    let json: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| LookupError::Failed(format!("bad Steam JSON: {e}")))?;
    let items = parse_storesearch(&json);
    let names: Vec<String> = items.iter().map(|(_, n)| n.clone()).collect();
    Ok(pick_match(name, &names).map(|i| items[i].0))
}

/// Resolve a folder name to GOG cover art via the free GOG Catalog
/// (no key). The cover URLs come from GOG's own image host only —
/// anything else is rejected before download (see `is_gog_cover_url`).
pub fn resolve_gog_cover(name: &str) -> Result<Option<GogCover>, LookupError> {
    let clean = sanitize_game_name(name);
    if clean.is_empty() {
        return Ok(None);
    }
    let url = format!(
        "{GOG_CATALOG_API}?query={}&limit=5",
        crate::http::url_encode(&format!("like:{clean}"))
    );
    let bytes = lookup_get(&url)?;
    let json: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| LookupError::Failed(format!("bad GOG JSON: {e}")))?;
    let products = parse_gog_catalog(&json);
    let names: Vec<String> = products.iter().map(|p| p.title.clone()).collect();
    Ok(pick_match(name, &names).map(|i| products[i].clone()))
}

/// GOG serves its covers from this host only. A cover URL anywhere
/// else is never downloaded.
fn is_gog_cover_url(url: &str) -> bool {
    url.starts_with("https://images.gog-statics.com/")
}

/// "Auto-fetch cover art from Steam" (Settings toggle). ON by
/// default — a missing setting means enabled.
pub fn is_auto_fetch_art_enabled(app: &AppHandle) -> bool {
    db::get_setting(app, "auto_fetch_art")
        .ok()
        .flatten()
        .map(|v| v.trim() != "0")
        .unwrap_or(true)
}

/// What one keyless art resolution for a scanned game came to.
pub enum ArtResolve {
    /// Art is now on disk (downloaded, or already cached — the next
    /// render retries a download that failed on the network).
    Found,
    /// No store has this game (failure recorded for the cooldown).
    NotFound,
    /// A recent failure still blocks automatic retries.
    Blocked,
    /// The store rate-limited us — stop the session's lookups.
    RateLimited,
    /// Network/parse failure — nothing recorded, retry later.
    Failed,
    /// Auto-fetch is off.
    Disabled,
    /// A manual cover is set — automatic lookups never touch it.
    Manual,
    /// Not a scanned game (no DB row to cache in).
    NotScanned,
}

/// Marker file recording a failed logo download (v0.2.8): the
/// unix time of the failure, so a missing logo is not re-fetched
/// on every scan — same 14-day cooldown as a failed keyless art
/// lookup (`art_lookup_blocked`). A file marker (not a DB column)
/// because Steam-library games have no `scanned_games` row.
fn logo_miss_file(app: &AppHandle, game_key: &str) -> Option<PathBuf> {
    art_dir(app)
        .ok()
        .map(|dir| dir.join(format!("{}-logo.miss", safe_key(game_key))))
}

/// `true` when a recorded logo miss still blocks a fresh download.
fn logo_miss_blocked(app: &AppHandle, game_key: &str) -> bool {
    let Some(path) = logo_miss_file(app, game_key) else {
        return false;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return false;
    };
    art_lookup_blocked(text.trim().parse().ok(), db::now())
}

fn mark_logo_miss(app: &AppHandle, game_key: &str) {
    if let Some(path) = logo_miss_file(app, game_key) {
        let _ = std::fs::write(&path, db::now().to_string());
    }
}

fn clear_logo_miss(app: &AppHandle, game_key: &str) {
    if let Some(path) = logo_miss_file(app, game_key) {
        let _ = std::fs::remove_file(&path);
    }
}

/// Download a Steam game logo (v0.2.8): PNG only, 10 MB cap, PNG
/// magic check, cached in the art dir. A miss is recorded so the
/// logo is not re-fetched for 14 days. Returns `true` when the
/// logo file exists afterwards (downloaded or already cached).
/// A missing logo is normal — never an error.
fn download_logo_art(app: &AppHandle, game_key: &str, logo_url: &str) -> bool {
    let Ok(dir) = art_dir(app) else {
        return false;
    };
    let dest = dir.join(logo_file_name(game_key));
    if let Ok(meta) = std::fs::metadata(&dest) {
        if meta.len() > 0 {
            return true; // cache hit
        }
    }
    if logo_miss_blocked(app, game_key) {
        return false;
    }
    let Ok(bytes) = crate::http::download_bytes_with_header(logo_url, None) else {
        mark_logo_miss(app, game_key);
        return false;
    };
    if bytes.len() > MAX_ART_BYTES || !is_png_bytes(&bytes) {
        mark_logo_miss(app, game_key);
        return false;
    }
    if std::fs::write(&dest, bytes).is_ok() {
        clear_logo_miss(app, game_key);
        return true;
    }
    mark_logo_miss(app, game_key);
    false
}

/// Download the three Steam CDN images for an appid. Returns `true`
/// when the tile file exists afterwards.
fn download_steam_art(app: &AppHandle, game_key: &str, appid: u32) -> bool {
    let Ok(dir) = art_dir(app) else {
        return false;
    };
    let (header_url, hero_url, capsule_url, logo_url) = steam_cdn_urls(appid);
    let tile = dir.join(file_name(game_key, "header"));
    let hero = dir.join(file_name(game_key, "hero"));
    let capsule = dir.join(file_name(game_key, "capsule"));
    download_art(&header_url, &tile, None);
    download_art(&hero_url, &hero, None);
    download_art(&capsule_url, &capsule, None);
    // Logo last (v0.2.8): optional — a miss is recorded, never an error.
    let _ = download_logo_art(app, game_key, &logo_url);
    tile.is_file()
}

/// Download a GOG cover: portrait to tile + capsule, wide to hero
/// (portrait again when no wide cover exists; wide as the tile when
/// no portrait exists). Only URLs on GOG's own image host are
/// fetched. Returns `true` when the tile exists afterwards.
fn download_gog_art(app: &AppHandle, game_key: &str, cover: &GogCover) -> bool {
    let Ok(dir) = art_dir(app) else {
        return false;
    };
    let tile = dir.join(file_name(game_key, "header"));
    let hero = dir.join(file_name(game_key, "hero"));
    let capsule = dir.join(file_name(game_key, "capsule"));
    if let Some(url) = &cover.vertical {
        if is_gog_cover_url(url) {
            download_art(url, &tile, None);
            let _ = std::fs::copy(&tile, &capsule);
        }
    }
    match &cover.horizontal {
        Some(url) if is_gog_cover_url(url) => {
            download_art(url, &hero, None);
        }
        _ => {
            // No wide cover: reuse the portrait as the hero.
            let _ = std::fs::copy(&tile, &hero);
        }
    }
    if !tile.is_file() {
        // No portrait cover: fall back to the wide one as the tile.
        if let Some(url) = &cover.horizontal {
            if is_gog_cover_url(url) {
                download_art(url, &tile, None);
                let _ = std::fs::copy(&tile, &capsule);
            }
        }
    }
    tile.is_file()
}

/// `true` when a scanned game's art is already settled — a manual
/// cover, a cached GOG cover, or a resolved Steam appid — so no
/// store lookup is needed. An exe-icon fallback is NOT settled
/// (v0.2.12): the icon is the last resort, and a game showing it
/// still needs a store lookup — a hit replaces the icon with the
/// real cover, a genuine miss records the failure (which surfaces
/// "Retry Art" in the ••• menu). Pure — unit-tested.
pub fn art_already_resolved(art_source: Option<&str>, resolved_appid: Option<i64>) -> bool {
    resolved_appid.is_some() || matches!(art_source, Some("manual") | Some("gog"))
}

/// Curated Steam appids for games whose current store listing is a
/// renamed variant the strict matcher rejects (v0.2.13). Steam
/// replaced the original "Grand Theft Auto V" listing with the
/// Enhanced edition and "The Witcher 3" with "Wild Hunt —
/// Remastered" / "REDkit" — no bare name is returned, so the
/// matcher can never pick them. Keys are normalized folder names
/// (`normalize_name` of the sanitized name). Consulted in
/// `resolve_art_for_game` before the store search. Keep it small
/// and explicit — only games proven to need it.
const KNOWN_APPIDS: &[(&str, u32)] = &[("grand theft auto 5", 3240220), ("the witcher 3", 292030)];

/// The curated Steam appid for a folder name, if one is known.
/// Pure — unit-tested.
pub fn known_appid(name: &str) -> Option<u32> {
    let norm = normalize_name(&sanitize_game_name(name));
    KNOWN_APPIDS
        .iter()
        .find(|(key, _)| *key == norm)
        .map(|(_, appid)| *appid)
}

/// `true` when a Steam store hit resolves the game: only when the
/// CDN download actually produced the tile (v0.2.13 F1). A failed
/// download leaves the game unresolved — the failure is recorded
/// (F2) so the game stays retryable and the toast stays truthful.
/// Pure — unit-tested.
pub fn steam_hit_resolves(download_ok: bool) -> bool {
    download_ok
}

/// `true` when a lookup error should be recorded as a failed
/// lookup (v0.2.13 F2): network errors, timeouts, bad responses,
/// and failed CDN downloads are all recorded, so "Retry Art" stays
/// in the ••• menu. A rate limit is transient — the worker stops
/// the session's lookups anyway — and is never recorded. Pure —
/// unit-tested.
pub fn lookup_failure_records(rate_limited: bool) -> bool {
    !rate_limited
}

/// Resolve art for one scanned game: cached Steam appid first, then
/// the Steam store search, then the GOG Catalog. Hits are cached in
/// the DB; a double miss records the failure time (14-day cooldown).
/// Never touches a game with a manual cover.
pub fn resolve_art_for_game(app: &AppHandle, game_key: &str, name: &str) -> ArtResolve {
    let Some(id) = db::scanned_id_from_key(game_key) else {
        return ArtResolve::NotScanned;
    };
    let Ok(Some(game)) = db::get_scanned_game(app, id) else {
        return ArtResolve::NotScanned;
    };
    if game.art_source.as_deref() == Some("manual") {
        return ArtResolve::Manual;
    }
    // Settled art (a resolved Steam appid or a cached GOG cover)
    // needs no lookup. An exe-icon fallback is NOT settled
    // (v0.2.12): the icon is the last resort, so the lookup below
    // still runs for it — a hit replaces the icon with the real
    // cover, a genuine miss records the failure (which surfaces
    // "Retry Art"). The old early-return froze icon games on the
    // icon forever after any transient lookup failure.
    if art_already_resolved(game.art_source.as_deref(), game.resolved_appid) {
        // A cached Steam appid: download the CDN art, no lookup needed.
        if let Some(appid) = game.resolved_appid.and_then(|a| u32::try_from(a).ok()) {
            download_steam_art(app, game_key, appid);
        }
        // A cached GOG hit: the files are already on disk.
        return ArtResolve::Found;
    }
    if !is_auto_fetch_art_enabled(app) {
        return ArtResolve::Disabled;
    }
    // Curated override (v0.2.13): a known appid skips the store
    // search entirely. This sits BEFORE the cooldown check on
    // purpose — the override performs no store search, so the
    // cooldown (which exists to protect the search API) does not
    // apply to it. A failed download is still recorded (F2), so
    // the worker backs off and "Retry Art" stays available.
    // Without this, a game whose lookup failed yesterday would
    // stay blocked for 14 days even though its appid is known.
    if let Some(appid) = known_appid(name) {
        if steam_hit_resolves(download_steam_art(app, game_key, appid)) {
            let _ = db::set_art_resolved(app, id, appid, "steam");
            return ArtResolve::Found;
        }
        let _ = db::mark_art_lookup_failed(app, id);
        return ArtResolve::Failed;
    }
    if art_lookup_blocked(game.art_lookup_failed_at, db::now()) {
        return ArtResolve::Blocked;
    }
    // Steam store search first.
    match resolve_steam_appid(name) {
        Ok(Some(appid)) => {
            // v0.2.13 F1: only a download that actually produced
            // the tile resolves the game. Recording a failed
            // download as a success cleared the failure and
            // settled the game forever with a placeholder tile
            // and no "Retry Art".
            if steam_hit_resolves(download_steam_art(app, game_key, appid)) {
                let _ = db::set_art_resolved(app, id, appid, "steam");
                ArtResolve::Found
            } else {
                let _ = db::mark_art_lookup_failed(app, id);
                ArtResolve::Failed
            }
        }
        Ok(None) => {
            // Then the GOG Catalog.
            match resolve_gog_cover(name) {
                Ok(Some(cover)) => {
                    if download_gog_art(app, game_key, &cover) {
                        let _ = db::set_art_source(app, id, "gog");
                        ArtResolve::Found
                    } else {
                        let _ = db::mark_art_lookup_failed(app, id);
                        ArtResolve::NotFound
                    }
                }
                Ok(None) => {
                    let _ = db::mark_art_lookup_failed(app, id);
                    ArtResolve::NotFound
                }
                Err(e) => {
                    // v0.2.13 F2: record network failures too — a
                    // network error used to record nothing, so
                    // "Retry Art" vanished from the menu until
                    // some later render double-missed.
                    if lookup_failure_records(matches!(e, LookupError::RateLimited)) {
                        let _ = db::mark_art_lookup_failed(app, id);
                    }
                    match e {
                        LookupError::RateLimited => ArtResolve::RateLimited,
                        LookupError::Failed(_) => ArtResolve::Failed,
                    }
                }
            }
        }
        Err(e) => {
            // v0.2.13 F2: see above.
            if lookup_failure_records(matches!(e, LookupError::RateLimited)) {
                let _ = db::mark_art_lookup_failed(app, id);
            }
            match e {
                LookupError::RateLimited => ArtResolve::RateLimited,
                LookupError::Failed(_) => ArtResolve::Failed,
            }
        }
    }
}

/// Background worker: after a scan finishes, resolve art for every
/// scanned game that still needs it — sequential, ~400 ms between
/// calls, at most one retry per lookup (inside the resolvers).
/// Stops the session's remaining lookups on a rate limit, and skips
/// games with a manual cover, a cached hit, a recent failure, or
/// art already on disk. Runs on its own thread; never blocks the UI.
pub fn run_art_lookup_worker(app: &AppHandle) {
    if is_steamgriddb_key_set(app) {
        // A saved SGDB key takes precedence — get_game_art uses it.
        return;
    }
    if !is_auto_fetch_art_enabled(app) {
        return;
    }
    let Ok(games) = db::list_scanned_games(app) else {
        return;
    };
    for game in games {
        // Settled art needs no lookup. An exe-icon fallback is not
        // settled (v0.2.12) — the worker retries it like any other
        // art-less game.
        if art_already_resolved(game.art_source.as_deref(), game.resolved_appid) {
            continue;
        }
        if art_lookup_blocked(game.art_lookup_failed_at, db::now()) {
            continue;
        }
        let key = format!("scan-{}", game.id);
        // Art already on disk (from an earlier run)? Skip the lookup.
        if let Ok(dir) = art_dir(app) {
            let tile = dir.join(file_name(&key, "header"));
            if std::fs::metadata(&tile)
                .map(|m| m.len() > 0)
                .unwrap_or(false)
            {
                continue;
            }
        }
        if matches!(
            resolve_art_for_game(app, &key, &game.name),
            ArtResolve::RateLimited
        ) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
}

// ---------------------------------------------------------------------------
// Public API.
// ---------------------------------------------------------------------------

/// Save the user's own SteamGridDB API key. The key is stored in the
/// settings table only — never logged, never sent anywhere except to
/// `steamgriddb.com` as a Bearer token.
pub fn set_steamgriddb_key(app: &AppHandle, key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        // Empty key = clear it.
        return db::set_setting(app, "steamgriddb_key", "");
    }
    db::set_setting(app, "steamgriddb_key", key)
}

/// Whether a SteamGridDB key is saved. The frontend gets only this
/// boolean — never the key itself.
pub fn is_steamgriddb_key_set(app: &AppHandle) -> bool {
    db::get_setting(app, "steamgriddb_key")
        .ok()
        .flatten()
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false)
}

fn steamgriddb_key(app: &AppHandle) -> Option<String> {
    let k = db::get_setting(app, "steamgriddb_key").ok().flatten()?;
    let k = k.trim().to_string();
    if k.is_empty() {
        None
    } else {
        Some(k)
    }
}

/// 10 MB cap for a single artwork download.
const MAX_ART_BYTES: usize = 10 * 1024 * 1024;

/// JPEG (FF D8 FF), PNG (89 50 4E 47 0D 0A 1A 0A), WebP (RIFF xxxx WEBP).
fn is_image_bytes(b: &[u8]) -> bool {
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

/// Download an artwork image with validation: 10 MB max, and the bytes
/// must start with a JPEG / PNG / WebP magic signature before anything
/// is written to disk. Returns `true` when a file was written, `false`
/// on a cache hit or a failed / oversized / invalid download.
fn download_art(url: &str, dest: &PathBuf, auth: Option<&str>) -> bool {
    if let Ok(meta) = std::fs::metadata(dest) {
        if meta.len() > 0 {
            return false; // cache hit
        }
    }
    let Ok(bytes) = crate::http::download_bytes_with_header(url, auth) else {
        return false;
    };
    if bytes.len() > MAX_ART_BYTES || !is_image_bytes(&bytes) {
        return false;
    }
    if let Some(parent) = dest.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return false;
        }
    }
    std::fs::write(dest, bytes).is_ok()
}

/// Download (or reuse from cache) the art for one game.
///
/// - Steam games (`appid` set): Steam CDN first.
/// - Scanned games with a manual cover: the cached files only —
///   automatic lookups and SGDB never touch them.
/// - Scanned games with a cached Steam appid: the Steam CDN (only
///   when no SGDB key is saved — a saved key takes precedence).
/// - Other games: SteamGridDB by name when a key is saved; otherwise
///   the keyless auto-fetch (Steam store search, then GOG Catalog)
///   when it is enabled.
/// - Last resort (v0.2.6): the game's own .exe icon (PNG >= 64 px),
///   centered on the gradient card — only when no other art exists
///   at all.
///
/// Never errors on download failure — returns whatever art could be
/// obtained (possibly none), so the UI never breaks on a bad network.
pub fn get_game_art(
    app: &AppHandle,
    game_key: &str,
    appid: Option<u32>,
    name: &str,
    exe_path: Option<&str>,
) -> GameArt {
    let Ok(dir) = art_dir(app) else {
        return GameArt::default();
    };
    let tile = dir.join(file_name(game_key, "header"));
    let hero = dir.join(file_name(game_key, "hero"));
    let capsule = dir.join(file_name(game_key, "capsule"));
    let logo = dir.join(logo_file_name(game_key));

    if let Some(id) = appid {
        let (header_url, hero_url, capsule_url, logo_url) = steam_cdn_urls(id);
        download_art(&header_url, &tile, None);
        download_art(&hero_url, &hero, None);
        download_art(&capsule_url, &capsule, None);
        // Logo last (v0.2.8): optional — a miss is recorded, never an error.
        let _ = download_logo_art(app, game_key, &logo_url);
        return build_art(&tile, &hero, &capsule, &logo);
    }

    // Scanned games: a manual cover wins over everything, and a
    // cached Steam appid wins over the keyless lookup (but not over
    // a saved SGDB key — that behavior stays as-is).
    if let Some(scan_id) = db::scanned_id_from_key(game_key) {
        if let Ok(Some(game)) = db::get_scanned_game(app, scan_id) {
            if game.art_source.as_deref() == Some("manual") {
                return build_art(&tile, &hero, &capsule, &logo);
            }
            if let Some(resolved) = game.resolved_appid.and_then(|a| u32::try_from(a).ok()) {
                if steamgriddb_key(app).is_none() {
                    download_steam_art(app, game_key, resolved);
                    return build_art(&tile, &hero, &capsule, &logo);
                }
            }
        }
    }

    // Non-Steam: SteamGridDB by name, only when a key is saved.
    if let Some(key) = steamgriddb_key(app) {
        if let Ok(results) = sgdb_search(&key, name) {
            if let Some((game_id, _)) = results.into_iter().next() {
                if let Ok((grid, hero_url)) = sgdb_images(&key, game_id) {
                    if let Some(grid_url) = grid {
                        download_art(&grid_url, &tile, Some(&sgdb_auth(&key)));
                        // The portrait grid doubles as the capsule.
                        let _ = std::fs::copy(&tile, &capsule);
                    }
                    if let Some(url) = hero_url {
                        download_art(&url, &hero, Some(&sgdb_auth(&key)));
                    }
                }
            }
        }
    } else {
        // No key: keyless auto-fetch (Steam store search, then GOG).
        let _ = resolve_art_for_game(app, game_key, name);
    }

    // Last resort: the game's own .exe icon — only when no other
    // art exists at all (no tile file on disk).
    if !tile.is_file() {
        if let Some(icon) = try_exe_icon(app, game_key, exe_path) {
            return icon;
        }
    }

    build_art(&tile, &hero, &capsule, &logo)
}

/// Let the user pick a local image as this game's cover. The image
/// is copied into the art dir (tile + hero + capsule) and marked
/// manual, so rescans and automatic lookups never overwrite it.
/// Returns the copied path, or `None` when the user cancels.
pub fn pick_cover_image(app: &AppHandle, game_key: &str) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let picked = app
        .dialog()
        .file()
        .add_filter("Images", &["jpg", "jpeg", "png", "webp"])
        .set_title("Choose cover art")
        .blocking_pick_file();
    let Some(file) = picked else {
        return Ok(None);
    };
    let Some(src) = file.as_path() else {
        return Err("picked file has no local path".to_string());
    };
    let bytes = std::fs::read(src).map_err(|e| format!("cannot read image: {e}"))?;
    if bytes.len() > MAX_ART_BYTES || !is_image_bytes(&bytes) {
        return Err("not a supported image (JPG / PNG / WebP, up to 10 MB)".to_string());
    }
    let dir = art_dir(app)?;
    let tile = dir.join(file_name(game_key, "header"));
    let hero = dir.join(file_name(game_key, "hero"));
    let capsule = dir.join(file_name(game_key, "capsule"));
    std::fs::write(&tile, &bytes).map_err(|e| format!("cannot save cover: {e}"))?;
    let _ = std::fs::copy(&tile, &hero);
    let _ = std::fs::copy(&tile, &capsule);
    if let Some(id) = db::scanned_id_from_key(game_key) {
        let _ = db::set_art_source(app, id, "manual");
    }
    Ok(Some(tile.to_string_lossy().into_owned()))
}

/// Set a game's cover from a Steam AppID the user typed in: verify
/// the AppID has CDN art first, then cache it as a manual cover.
/// Returns an error (and persists nothing) when Steam has no art
/// for that AppID.
pub fn set_cover_art_appid(app: &AppHandle, game_key: &str, appid: u32) -> Result<(), String> {
    if appid == 0 {
        return Err("AppID must be a number greater than 0".to_string());
    }
    let dir = art_dir(app)?;
    let tile = dir.join(file_name(game_key, "header"));
    let hero = dir.join(file_name(game_key, "hero"));
    let capsule = dir.join(file_name(game_key, "capsule"));
    // Clear any previous cover so a cache hit cannot fake a success.
    // The logo goes too — it belongs to the old AppID — and its miss
    // marker, so the new AppID's logo is fetched fresh.
    let _ = std::fs::remove_file(&tile);
    let _ = std::fs::remove_file(&hero);
    let _ = std::fs::remove_file(&capsule);
    let _ = std::fs::remove_file(&dir.join(logo_file_name(game_key)));
    clear_logo_miss(app, game_key);
    if !download_steam_art(app, game_key, appid) {
        return Err(format!("Steam has no cover art for AppID {appid}"));
    }
    if let Some(id) = db::scanned_id_from_key(game_key) {
        let _ = db::set_art_resolved(app, id, appid, "manual");
    }
    Ok(())
}

/// Set a game's cover from a user-pasted image URL: https only, no
/// whitespace, 10 MB cap, magic-byte check — then saved into the art
/// dir and marked manual. Never hotlinked: the bytes are downloaded
/// once and served locally from then on.
pub fn set_cover_art_url(app: &AppHandle, game_key: &str, url: &str) -> Result<(), String> {
    let url = url.trim();
    validate_image_url(url)?;
    let bytes = match crate::http::get_capped(url, 8, MAX_ART_BYTES) {
        crate::http::CappedGet::Ok(b) => b,
        crate::http::CappedGet::RateLimited => {
            return Err("the image host refused the download (429/403)".to_string());
        }
        crate::http::CappedGet::Failed(e) => return Err(e),
    };
    if !is_image_bytes(&bytes) {
        return Err("that URL is not an image (JPG / PNG / WebP)".to_string());
    }
    let dir = art_dir(app)?;
    let tile = dir.join(file_name(game_key, "header"));
    let hero = dir.join(file_name(game_key, "hero"));
    let capsule = dir.join(file_name(game_key, "capsule"));
    std::fs::write(&tile, &bytes).map_err(|e| format!("cannot save cover: {e}"))?;
    let _ = std::fs::copy(&tile, &hero);
    let _ = std::fs::copy(&tile, &capsule);
    if let Some(id) = db::scanned_id_from_key(game_key) {
        let _ = db::set_art_source(app, id, "manual");
    }
    Ok(())
}

/// "Retry Art" from the ••• menu: forget the last failure and run a
/// fresh keyless lookup now. Returns a short status for the toast.
pub fn retry_art(app: &AppHandle, game_key: &str, name: &str) -> Result<String, String> {
    let Some(id) = db::scanned_id_from_key(game_key) else {
        return Err("Retry Art is only for scanned games".to_string());
    };
    let _ = db::clear_art_lookup_failed(app, id);
    // A manual retry re-attempts the logo too.
    clear_logo_miss(app, game_key);
    Ok(match resolve_art_for_game(app, game_key, name) {
        ArtResolve::Found => "Cover art found".to_string(),
        ArtResolve::NotFound => "No cover found for this game".to_string(),
        ArtResolve::Blocked => "Cover lookup was already retried recently".to_string(),
        ArtResolve::RateLimited => "Steam is rate-limiting lookups — try again later".to_string(),
        ArtResolve::Failed => "Cover lookup failed (network?) — try again later".to_string(),
        ArtResolve::Disabled => "Auto-fetch cover art is off in Settings".to_string(),
        ArtResolve::Manual => "This game has a manual cover".to_string(),
        ArtResolve::NotScanned => "Retry Art is only for scanned games".to_string(),
    })
}

/// Clear Art / Music Cache (v0.2.11, Settings → About): delete every
/// cached cover/logo file (including `*-logo.miss` markers, which
/// live in the art dir) and every cached theme file, forget all art
/// decisions and all music choices in the DB. The next lookup /
/// download starts fresh. Returns a short summary for the toast.
pub fn clear_art_music_cache(app: &AppHandle) -> Result<String, String> {
    let mut art_files = 0;
    if let Ok(dir) = art_dir(app) {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if std::fs::remove_file(entry.path()).is_ok() {
                    art_files += 1;
                }
            }
        }
    }
    let mut music_files = 0;
    if let Ok(dir) = crate::music::music_dir(app) {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if std::fs::remove_file(entry.path()).is_ok() {
                    music_files += 1;
                }
            }
        }
    }
    db::clear_all_art_cache(app)?;
    db::clear_all_music(app)?;
    Ok(format!(
        "Cleared {art_files} art files and {music_files} music files"
    ))
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
        assert!(is_png_bytes(&[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A
        ]));
        assert!(!is_png_bytes(&[0x89, 0x50, 0x4E, 0x47])); // too short
        assert!(!is_png_bytes(&[
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x00, 0x00
        ])); // JPEG
        assert!(!is_png_bytes(b"<!DOCTYPE html>"));
        assert!(!is_png_bytes(b""));
    }

    #[test]
    fn logo_file_name_is_png_and_sanitized() {
        assert_eq!(logo_file_name("steam-440"), "steam-440-logo.png");
        assert_eq!(
            logo_file_name("weird key/with:chars"),
            "weird_key_with_chars-logo.png"
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
        // A year that is part of the title is kept (only bracketed
        // years are dropped).
        assert_eq!(sanitize_game_name("Battlefield 1942"), "battlefield 1942");
    }

    #[test]
    fn normalize_converts_roman_numerals_and_punctuation() {
        assert_eq!(normalize_name("The Witcher III"), "the witcher 3");
        assert_eq!(normalize_name("Grand Theft Auto V"), "grand theft auto 5");
        assert_eq!(normalize_name("Hades II"), "hades 2");
        assert_eq!(normalize_name("Civilization VI"), "civilization 6");
        // A numeral inside a word is not converted.
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
        // "ghost" must not trip the "ost" marker (whole tokens only).
        assert!(!is_junk_name(
            &normalize_name("Ghost of Tsushima"),
            &normalize_name("Ghost of Tsushima")
        ));
        // When the folder itself is the soundtrack, it is not junk.
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
        // Two candidates, equally close (>= 0.85) -> too close to call.
        let tied = vec![
            "Legend of Zelda Breath of the Wild Master".to_string(),
            "Legend of Zelda Breath of the Wild Deluxe".to_string(),
        ];
        assert_eq!(
            pick_match("Legend of Zelda Breath of the Wild", &tied),
            None
        );
        // One clear winner, #2 far behind -> pick it.
        let clear = vec![
            "Legend of Zelda Breath of the Wild Master".to_string(),
            "Legend of Zelda Tears of the Kingdom".to_string(),
        ];
        assert_eq!(
            pick_match("Legend of Zelda Breath of the Wild", &clear),
            Some(0)
        );
        // Best below 0.85 -> None.
        assert_eq!(pick_match("Doom", &["Doom Eternal".to_string()]), None);
        assert_eq!(pick_match("Doom", &[] as &[String]), None);
    }

    #[test]
    fn pick_match_grand_theft_auto_v_is_a_tier1_exact_match() {
        // v0.2.13: the live Steam storesearch (2026-11-24) no
        // longer returns the bare "Grand Theft Auto V" (271590) —
        // Rockstar replaced the listing with "Grand Theft Auto V
        // Enhanced" (3240220). The old test used a hypothetical
        // list containing the bare title, so its premise was stale
        // (and the v0.2.12 diagnosis that matching was NOT the
        // problem was wrong for the same reason). With bare
        // trailing "enhanced" stripped, the Enhanced listing
        // normalizes to "grand theft auto 5" and Tier-1 matches.
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
    fn pick_match_witcher_3_live_list_has_no_bare_name() {
        // v0.2.13: the live storesearch returns only renamed
        // variants — "Wild Hunt — Remastered" (0.600) and "REDkit"
        // (0.750) — so the strict matcher rightly returns None
        // and the curated override is what finds the cover.
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
