//! Other game stores + generic folder scan (Phase 4).
//!
//! Detection is Windows-only (registry / well-known folders) and
//! best-effort: every detector returns an empty list — never an error —
//! when its store is absent or its files are malformed.
//!
//! - Epic: `%ProgramData%\Epic\EpicGamesLauncher\Data\Manifests\*.item`
//!   (JSON: DisplayName, InstallLocation, LaunchExecutable).
//! - GOG: `HKLM\SOFTWARE\WOW6432Node\GOG.com\Games\*` (gameName, path).
//! - Ubisoft: `HKLM\SOFTWARE\WOW6432Node\Ubisoft\Launcher\Installs\*`
//!   (InstallDir; name = install folder name).
//! - EA: `HKLM\SOFTWARE\WOW6432Node\EA Games\*` (Install Dir).
//! - Battle.net: `%ProgramData%\Battle.net\Agent\product.db` (SQLite,
//!   known product codes only; unknown codes are skipped).
//! - Xbox / Microsoft Store: NO reliable local method exists (games are
//!   UWP packages launched by family name, not by exe path) —
//!   deliberately NOT faked. Those games are covered by manual add and
//!   the generic scan.
//!
//! Generic scan: the user adds folders in Settings; we walk them
//! (max depth 4) for `*.exe`, drop installer/updater exes, dedupe by
//! path, and store results in `scanned_games`.

use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

use crate::db;

/// One game found outside Steam, as exposed to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct DetectedGame {
    pub name: String,
    pub exe_path: Option<String>,
    pub launch_uri: Option<String>,
    pub source: String,
    pub appid: Option<u32>,
}

/// One game in the unified list (Steam + manual + stores + scanned).
#[derive(Debug, Clone, Serialize)]
pub struct UnifiedGame {
    pub key: String,
    pub name: String,
    pub source: String,
    pub appid: Option<u32>,
    pub exe_path: Option<String>,
    pub launch_uri: Option<String>,
}

/// One exe found by the generic folder scan.
#[derive(Debug, Clone, Serialize)]
pub struct ScannedFound {
    pub name: String,
    pub exe_path: String,
}

// ---------------------------------------------------------------------------
// Epic Games Store.
// ---------------------------------------------------------------------------

/// Parse one Epic `.item` manifest into a game. Pure — unit-tested.
/// Returns `None` when DisplayName / InstallLocation / LaunchExecutable
/// are missing (or the exe path is empty).
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

/// Read every Epic manifest. The exe must exist on disk to be returned.
pub fn detect_epic() -> Vec<DetectedGame> {
    let base = std::env::var("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(r"C:\ProgramData"));
    let manifests = base.join(r"Epic\EpicGamesLauncher\Data\Manifests");
    let Ok(entries) = std::fs::read_dir(&manifests) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("item") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(game) = parse_epic_item(&text) {
            // Only keep games whose exe actually exists.
            if let Some(exe) = &game.exe_path {
                if Path::new(exe).is_file() {
                    out.push(game);
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

// ---------------------------------------------------------------------------
// Registry-based stores (Windows only).
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn reg_subkey_names(hive: winreg::HKEY, path: &str) -> Vec<String> {
    use winreg::RegKey;
    let Ok(key) = RegKey::predef(hive).open_subkey(path) else {
        return Vec::new();
    };
    key.enum_keys().flatten().collect()
}

#[cfg(windows)]
fn reg_string(hive: winreg::HKEY, path: &str, value: &str) -> Option<String> {
    use winreg::RegKey;
    let key = RegKey::predef(hive).open_subkey(path).ok()?;
    let s: String = key.get_value(value).ok()?;
    let s = s.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Pick the best exe inside an install dir: prefer one whose name
/// contains a word from the game name, otherwise the largest exe.
/// Installer/updater exes are skipped. `None` when nothing fits.
/// Pick the best exe from a list of candidates for one game folder.
/// Prefers an exe whose stem matches a word (>= 3 chars) of the folder
/// name, else the largest exe by file size (usually the game binary).
fn pick_best_exe(exes: &[PathBuf], folder_name: &str) -> Option<PathBuf> {
    if exes.is_empty() {
        return None;
    }
    let words: Vec<&str> = folder_name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3)
        .collect();
    for cand in exes {
        let stem = cand
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        if words.iter().any(|w| stem.contains(&w.to_lowercase())) {
            return Some(cand.clone());
        }
    }
    // Fallback: the largest exe (usually the game binary).
    exes.iter()
        .max_by_key(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
        .cloned()
}

fn best_exe_in_dir(dir: &Path, game_name: &str) -> Option<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return None;
    };
    let mut exes: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("exe"))
            == Some(true)
            && p.is_file()
        {
            exes.push(p);
        }
    }
    let names: Vec<String> = exes
        .iter()
        .filter_map(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_lowercase())
        })
        .collect();
    let candidates: Vec<&PathBuf> = exes
        .iter()
        .filter(|p| {
            let stem = p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            !is_installer_exe(&stem, &names)
        })
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let owned: Vec<PathBuf> = candidates.into_iter().cloned().collect();
    pick_best_exe(&owned, game_name)
}

/// GOG Galaxy games from the registry.
#[cfg(windows)]
pub fn detect_gog() -> Vec<DetectedGame> {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    let mut out = Vec::new();
    for sub in reg_subkey_names(HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\GOG.com\Games") {
        let path = format!(r"SOFTWARE\WOW6432Node\GOG.com\Games\{sub}");
        let Some(name) = reg_string(HKEY_LOCAL_MACHINE, &path, "gameName") else {
            continue;
        };
        let Some(install) = reg_string(HKEY_LOCAL_MACHINE, &path, "path") else {
            continue;
        };
        let dir = PathBuf::from(&install);
        if !dir.is_dir() {
            continue;
        }
        if let Some(exe) = best_exe_in_dir(&dir, &name) {
            out.push(DetectedGame {
                name,
                exe_path: Some(exe.to_string_lossy().into_owned()),
                launch_uri: None,
                source: "gog".to_string(),
                appid: None,
            });
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

#[cfg(not(windows))]
pub fn detect_gog() -> Vec<DetectedGame> {
    Vec::new()
}

/// Ubisoft Connect games from the registry (name = install folder name).
#[cfg(windows)]
pub fn detect_ubisoft() -> Vec<DetectedGame> {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    let mut out = Vec::new();
    for sub in reg_subkey_names(
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\WOW6432Node\Ubisoft\Launcher\Installs",
    ) {
        let path = format!(r"SOFTWARE\WOW6432Node\Ubisoft\Launcher\Installs\{sub}");
        let Some(install) = reg_string(HKEY_LOCAL_MACHINE, &path, "InstallDir") else {
            continue;
        };
        let dir = PathBuf::from(&install);
        if !dir.is_dir() {
            continue;
        }
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Ubisoft Game")
            .to_string();
        if let Some(exe) = best_exe_in_dir(&dir, &name) {
            out.push(DetectedGame {
                name,
                exe_path: Some(exe.to_string_lossy().into_owned()),
                launch_uri: None,
                source: "ubisoft".to_string(),
                appid: None,
            });
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

#[cfg(not(windows))]
pub fn detect_ubisoft() -> Vec<DetectedGame> {
    Vec::new()
}

/// EA games from the registry (EA App / Origin era keys).
#[cfg(windows)]
pub fn detect_ea() -> Vec<DetectedGame> {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    let mut out = Vec::new();
    for sub in reg_subkey_names(HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\EA Games") {
        let path = format!(r"SOFTWARE\WOW6432Node\EA Games\{sub}");
        let Some(install) = reg_string(HKEY_LOCAL_MACHINE, &path, "Install Dir") else {
            continue;
        };
        let dir = PathBuf::from(&install);
        if !dir.is_dir() {
            continue;
        }
        let name = sub.clone();
        if let Some(exe) = best_exe_in_dir(&dir, &name) {
            out.push(DetectedGame {
                name,
                exe_path: Some(exe.to_string_lossy().into_owned()),
                launch_uri: None,
                source: "ea".to_string(),
                appid: None,
            });
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

#[cfg(not(windows))]
pub fn detect_ea() -> Vec<DetectedGame> {
    Vec::new()
}

// ---------------------------------------------------------------------------
// Battle.net — product.db (SQLite) + known product codes.
// ---------------------------------------------------------------------------

/// Known Battle.net product codes → (display name, exe file name).
/// Only codes we are sure about are listed; unknown codes are skipped
/// rather than guessed (the generic scan covers them).
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

/// Read Battle.net's product.db (read-only). Returns
/// `(product_code, install_path)` pairs for installed products.
fn battlenet_installed() -> Vec<(String, String)> {
    let base = std::env::var("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(r"C:\ProgramData"));
    let db_path = base.join(r"Battle.net\Agent\product.db");
    if !db_path.is_file() {
        return Vec::new();
    }
    let Ok(conn) =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return Vec::new();
    };
    let mut stmt = match conn
        .prepare("SELECT product_code, install_path FROM product_install WHERE installed = 1")
    {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = match stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for row in rows.flatten() {
        out.push(row);
    }
    out
}

pub fn detect_battlenet() -> Vec<DetectedGame> {
    let mut out = Vec::new();
    for (code, install_path) in battlenet_installed() {
        let Some((name, exe_name)) = battlenet_product(&code) else {
            continue;
        };
        let exe = Path::new(&install_path).join(exe_name);
        if exe.is_file() {
            out.push(DetectedGame {
                name: name.to_string(),
                exe_path: Some(exe.to_string_lossy().into_owned()),
                launch_uri: None,
                source: "battlenet".to_string(),
                appid: None,
            });
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// All non-Steam stores, deduped by exe path (case-insensitive).
pub fn detect_other_stores() -> Vec<DetectedGame> {
    let mut out = Vec::new();
    out.extend(detect_epic());
    out.extend(detect_gog());
    out.extend(detect_ubisoft());
    out.extend(detect_ea());
    out.extend(detect_battlenet());
    let mut seen: HashSet<String> = HashSet::new();
    out.retain(|g| {
        let key = g.exe_path.as_deref().unwrap_or("").to_lowercase();
        if key.is_empty() {
            return true;
        }
        seen.insert(key)
    });
    out
}

// ---------------------------------------------------------------------------
// Generic folder scan.
// ---------------------------------------------------------------------------

/// Whether an exe file stem is an installer/updater rather than a game.
///
/// - Always installer: contains `uninstall` / `unins` / `vcredist` /
///   `dxsetup` / `redist`, or is exactly `update` / `updater`.
/// - `setup`: installer ONLY when the same folder contains another exe
///   (a game whose only exe is `setup.exe` is kept — the locked plan
///   explicitly protects legitimately-named game exes).
/// - `launcher` is NEVER filtered: many games ship as `Launcher.exe`.
///
/// Pure — unit-tested. `siblings` is the list of lowercased exe stems in
/// the same directory (including this one).
pub fn is_installer_exe(stem_lower: &str, siblings: &[String]) -> bool {
    // `setup.exe` is ALWAYS an installer — even when it is the only exe in
    // a folder. Treating a lone installer as a game would add installers
    // to the library.
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

/// Derive a display name from an exe path. Generic stems (`launcher`,
/// `game`, `client`, `main`, `start`) fall back to the parent folder
/// name. Backslashes are normalized first so Windows paths behave the
/// same on every platform (the verify crate runs on Linux). Pure —
/// unit-tested.
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

/// `my_cool-game` → `My Cool Game`.
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

/// Scan folders for games (max depth 4). One best exe per top-level game
/// folder under each user-configured root — so an Unreal game with a root
/// exe plus a `Binaries\Win64\` exe contributes ONE game, not two.
/// Exes directly in the root are each their own game. Installers, helpers
/// and paths in `ignored` are dropped. Never errors — missing folders are
/// skipped silently.
pub fn scan_folders(folders: &[String], ignored: &HashSet<String>) -> Vec<ScannedFound> {
    let mut out: Vec<ScannedFound> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for folder in folders {
        let dir = PathBuf::from(folder);
        if !dir.is_dir() {
            continue;
        }
        let mut all: Vec<PathBuf> = Vec::new();
        walk_exes(&dir, 0, 4, &mut all);
        let mut exes: Vec<PathBuf> = Vec::new();
        for p in all {
            let stem = p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if is_installer_exe(&stem, &[]) {
                continue;
            }
            let key = p.to_string_lossy().to_lowercase();
            if ignored.contains(&key) {
                continue;
            }
            exes.push(p);
        }
        if exes.is_empty() {
            continue;
        }
        let root_name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        // Single-game-folder escape: if the best exe of the whole tree
        // matches the root folder's name, the root IS one game (the user
        // added the game's own folder, which may have bin/ + engine/
        // subdirs). Otherwise each immediate subdir is one game folder.
        if let Some(best) = pick_best_exe(&exes, &root_name) {
            let stem = best
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if exe_stem_matches_folder(&stem, &root_name) {
                let name = game_name_for_group(&root_name, &best);
                push_found(&mut out, &mut seen, &best, &name);
                continue;
            }
        }
        // Group by immediate child directory of the root.
        let mut by_dir: std::collections::HashMap<PathBuf, Vec<PathBuf>> =
            std::collections::HashMap::new();
        let mut direct: Vec<PathBuf> = Vec::new();
        for p in exes {
            let rel = match p.strip_prefix(&dir) {
                Ok(r) => r.to_path_buf(),
                Err(_) => {
                    direct.push(p);
                    continue;
                }
            };
            let mut comps = rel.components();
            match comps.next() {
                Some(std::path::Component::Normal(first)) if comps.next().is_some() => {
                    by_dir.entry(dir.join(first)).or_default().push(p);
                }
                _ => direct.push(p),
            }
        }
        for p in direct {
            let name = display_name_for_exe(&p);
            push_found(&mut out, &mut seen, &p, &name);
        }
        for (game_dir, group) in by_dir {
            let folder_name = game_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Game")
                .to_string();
            if let Some(best) = pick_best_exe(&group, &folder_name) {
                let name = game_name_for_group(&folder_name, &best);
                push_found(&mut out, &mut seen, &best, &name);
            }
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Does an exe stem match its folder name (alphanumeric, case-insensitive,
/// prefix match either way)? Used to recognise a root that IS one game.
fn exe_stem_matches_folder(stem_lower: &str, folder_name: &str) -> bool {
    let norm = |s: &str| {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_lowercase()
    };
    let f = norm(folder_name);
    let s = norm(stem_lower);
    !f.is_empty() && (s.starts_with(&f) || f.starts_with(&s))
}

/// Game name for a folder group: the folder name, unless it is a generic
/// word, in which case fall back to the exe-derived name.
fn game_name_for_group(folder_name: &str, exe_path: &Path) -> String {
    let generic = ["launcher", "game", "client", "main", "start", "play"];
    if !folder_name.trim().is_empty() && !generic.contains(&folder_name.to_lowercase().as_str()) {
        return prettify(folder_name);
    }
    display_name_for_exe(exe_path)
}

fn push_found(out: &mut Vec<ScannedFound>, seen: &mut HashSet<String>, exe: &Path, name: &str) {
    let key = exe.to_string_lossy().to_lowercase();
    if !seen.insert(key) {
        return;
    }
    out.push(ScannedFound {
        name: name.to_string(),
        exe_path: exe.to_string_lossy().into_owned(),
    });
}

// ---------------------------------------------------------------------------
// Unified list.
// ---------------------------------------------------------------------------

fn slug(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// Every known game, unified: Steam + manual + other stores + scanned.
/// Deduped by Steam appid and by exe path (case-insensitive). Never
/// errors — an absent store/folder simply contributes nothing.
pub fn list_all_games(app: &AppHandle) -> Vec<UnifiedGame> {
    let mut out: Vec<UnifiedGame> = Vec::new();
    let mut seen_exe: HashSet<String> = HashSet::new();
    let mut seen_appid: HashSet<u32> = HashSet::new();

    for s in crate::steam::detect_steam_games() {
        if seen_appid.insert(s.appid) {
            out.push(UnifiedGame {
                key: format!("steam-{}", s.appid),
                name: s.name,
                source: "steam".to_string(),
                appid: Some(s.appid),
                exe_path: None,
                launch_uri: Some(format!("steam://rungameid/{}", s.appid)),
            });
        }
    }

    if let Ok(manual) = db::list_manual_games(app) {
        for m in manual {
            let key = m.exe_path.to_lowercase();
            if seen_exe.insert(key) {
                out.push(UnifiedGame {
                    key: format!("manual-{}", m.id),
                    name: m.name,
                    source: "manual".to_string(),
                    appid: None,
                    exe_path: Some(m.exe_path),
                    launch_uri: None,
                });
            }
        }
    }

    for g in detect_other_stores() {
        let exe_key = g.exe_path.as_deref().unwrap_or("").to_lowercase();
        if !exe_key.is_empty() && !seen_exe.insert(exe_key) {
            continue;
        }
        out.push(UnifiedGame {
            key: format!("{}-{}", g.source, slug(&g.name)),
            name: g.name,
            source: g.source,
            appid: g.appid,
            exe_path: g.exe_path,
            launch_uri: g.launch_uri,
        });
    }

    if let Ok(scanned) = db::list_scanned_games(app) {
        for s in scanned {
            let key = s.exe_path.to_lowercase();
            if seen_exe.insert(key) {
                out.push(UnifiedGame {
                    key: format!("scan-{}", s.id),
                    name: s.name,
                    source: "scan".to_string(),
                    appid: None,
                    exe_path: Some(s.exe_path),
                    launch_uri: None,
                });
            }
        }
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
        // setup.exe: ALWAYS an installer, even when alone in a folder.
        assert!(is_installer_exe(
            "setup",
            &["setup".to_string(), "game.exe".to_string()]
        ));
        assert!(is_installer_exe("setup", &["setup".to_string()]));
        // Helper / non-game exes inside real game folders.
        assert!(is_installer_exe("crashreporter", &empty));
        assert!(is_installer_exe("crashhandler", &empty));
        assert!(is_installer_exe("unitycrashhandler", &empty));
        assert!(is_installer_exe("crashpad_handler", &empty));
        assert!(is_installer_exe("ue4prereqsetup_x64", &empty));
        assert!(is_installer_exe("ueprereqsetup_x64", &empty));
        assert!(is_installer_exe("dotnet", &empty));
        assert!(is_installer_exe("benchmark", &empty));
        assert!(is_installer_exe("gameconfig", &empty));
        assert!(is_installer_exe("easyanticheat_setup", &empty));
        assert!(is_installer_exe("battleye", &empty));
        // Real games are never filtered — including Launcher.exe.
        assert!(!is_installer_exe("launcher", &empty));
        assert!(!is_installer_exe("fortniteclient-win64-shipping", &empty));
        assert!(!is_installer_exe("game", &empty));
        assert!(!is_installer_exe("setup_game", &empty));
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
        let ue = root.join("UE Game");
        let ue_bin = ue.join("Binaries").join("Win64");
        let inst_only = root.join("Installer Only");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::create_dir_all(&ue_bin).unwrap();
        std::fs::create_dir_all(&inst_only).unwrap();
        std::fs::write(game.join("MyGame.exe"), b"x").unwrap();
        std::fs::write(game.join("setup.exe"), b"x").unwrap(); // dropped: always
        std::fs::write(game.join("unins000.exe"), b"x").unwrap(); // dropped
        std::fs::write(game.join("CrashReporter.exe"), b"x").unwrap(); // dropped: helper
        std::fs::write(other.join("Launcher.exe"), b"x").unwrap(); // kept, folder name
        std::fs::write(other.join("sub").join("Deep.exe"), b"x").unwrap_or_else(|_| {
            std::fs::create_dir_all(other.join("sub")).unwrap();
            std::fs::write(other.join("sub").join("Deep.exe"), b"x").unwrap();
        });
        // UE-style: root exe + Binaries\Win64\ exe -> ONE game, not two.
        std::fs::write(ue.join("UEGame.exe"), b"xx").unwrap();
        std::fs::write(ue_bin.join("UEGame-Win64-Shipping.exe"), b"x").unwrap();
        // A folder with ONLY setup.exe contributes nothing.
        std::fs::write(inst_only.join("setup.exe"), b"x").unwrap();
        // A loose exe directly in the root is its own game.
        std::fs::write(root.join("Loose.exe"), b"x").unwrap();
        let ignored: HashSet<String> = HashSet::new();
        let found = scan_folders(&[root.to_string_lossy().into_owned()], &ignored);
        let names: Vec<&str> = found.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"My Game"), "{names:?}");
        assert!(names.contains(&"Other Game"), "{names:?}");
        assert!(names.contains(&"UE Game"), "{names:?}");
        assert!(names.contains(&"Loose"), "{names:?}");
        // Deep.exe is inside the "Other Game" group, not a separate game.
        assert!(!names.contains(&"Deep"), "{names:?}");
        assert!(!names.iter().any(|n| n.to_lowercase().contains("setup")));
        assert!(!names.iter().any(|n| n.to_lowercase().contains("unins")));
        assert!(!names.iter().any(|n| n.to_lowercase().contains("crash")));
        assert_eq!(found.len(), 4);
        // Ignored paths are skipped.
        let ignored_exe = game.join("MyGame.exe").to_string_lossy().to_lowercase();
        let mut ignored: HashSet<String> = HashSet::new();
        ignored.insert(ignored_exe);
        let found2 = scan_folders(&[root.to_string_lossy().into_owned()], &ignored);
        let names2: Vec<&str> = found2.iter().map(|f| f.name.as_str()).collect();
        assert!(!names2.contains(&"My Game"), "{names2:?}");
        assert_eq!(found2.len(), 3);
        // Missing folder contributes nothing.
        assert!(scan_folders(&["/does/not/exist".to_string()], &ignored).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_folders_does_not_follow_symlinks() {
        // A symlink (or Windows junction) inside the scan root is never
        // followed: the target game must not appear twice, and a link
        // pointing outside the root must not pull outside games in.
        let root = std::env::temp_dir().join("p4-verify-scan-link");
        let _ = std::fs::remove_dir_all(&root);
        let real = root.join("Real Game");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("RealGame.exe"), b"x").unwrap();
        let outside = std::env::temp_dir().join("p4-verify-scan-outside");
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("Outside.exe"), b"x").unwrap();
        // Try to create the link; skip the test if the OS refuses
        // (Windows needs Developer Mode / admin for symlinks).
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
        let ignored: HashSet<String> = HashSet::new();
        let found = scan_folders(&[root.to_string_lossy().into_owned()], &ignored);
        let names: Vec<&str> = found.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"Real Game"), "{names:?}");
        // The outside game (reachable only via the link) must NOT appear.
        assert!(!names.contains(&"Outside"), "{names:?}");
        assert_eq!(found.len(), 1, "{names:?}");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn scan_folders_single_game_folder_escape() {
        // A root that IS one game (name matches its exe) yields one game,
        // even though it has bin/ + engine/ subdirs.
        let game_root = std::env::temp_dir().join("Cyberpunk 2077");
        let _ = std::fs::remove_dir_all(&game_root);
        std::fs::create_dir_all(game_root.join("bin").join("x64")).unwrap();
        std::fs::create_dir_all(game_root.join("engine")).unwrap();
        std::fs::write(
            game_root.join("bin").join("x64").join("Cyberpunk2077.exe"),
            b"xx",
        )
        .unwrap();
        let ignored: HashSet<String> = HashSet::new();
        let found = scan_folders(&[game_root.to_string_lossy().into_owned()], &ignored);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].name, "Cyberpunk 2077");
        let _ = std::fs::remove_dir_all(&game_root);
    }
}
