//! Game art auto-download (Phase 2).
//!
//! Primary source: the free Steam CDN (no key needed) — per appid:
//! - tile:    `.../steam/apps/{appid}/header.jpg`
//! - hero:    `.../steam/apps/{appid}/library_hero.jpg`
//! - capsule: `.../steam/apps/{appid}/library_600x900.jpg`
//!
//! Optional source: SteamGridDB, when the user saved their own API key
//! in Settings (stored in the settings table, never hardcoded, never
//! logged). On any SteamGridDB failure we fall back to the Steam CDN (or
//! to no art for non-Steam games).
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
    /// Local path of the tile image (Steam header or SGDB grid).
    pub tile_path: Option<String>,
    /// Local path of the hero background image.
    pub hero_path: Option<String>,
    /// Local path of the portrait capsule image.
    pub capsule_path: Option<String>,
    /// Base64 data URI of the tile (fallback when convertFileSrc fails).
    pub tile_data_uri: Option<String>,
    /// Base64 data URI of the hero (fallback when convertFileSrc fails).
    pub hero_data_uri: Option<String>,
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

/// The three Steam CDN URLs for one appid. Pure — unit-tested.
pub fn steam_cdn_urls(appid: u32) -> (String, String, String) {
    (
        format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/header.jpg"),
        format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/library_hero.jpg"),
        format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{appid}/library_600x900.jpg"),
    )
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

fn build_art(tile: &PathBuf, hero: &PathBuf, capsule: &PathBuf) -> GameArt {
    GameArt {
        tile_path: path_str(tile),
        hero_path: path_str(hero),
        capsule_path: path_str(capsule),
        tile_data_uri: data_uri(tile),
        hero_data_uri: data_uri(hero),
    }
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
/// - Other games: SteamGridDB by name when a key is saved; otherwise no
///   art (the frontend keeps the Sonic-gradient placeholder).
///
/// Never errors on download failure — returns whatever art could be
/// obtained (possibly none), so the UI never breaks on a bad network.
pub fn get_game_art(app: &AppHandle, game_key: &str, appid: Option<u32>, name: &str) -> GameArt {
    let Ok(dir) = art_dir(app) else {
        return GameArt::default();
    };
    let tile = dir.join(file_name(game_key, "header"));
    let hero = dir.join(file_name(game_key, "hero"));
    let capsule = dir.join(file_name(game_key, "capsule"));

    if let Some(id) = appid {
        let (header_url, hero_url, capsule_url) = steam_cdn_urls(id);
        download_art(&header_url, &tile, None);
        download_art(&hero_url, &hero, None);
        download_art(&capsule_url, &capsule, None);
        return build_art(&tile, &hero, &capsule);
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
    }

    build_art(&tile, &hero, &capsule)
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
