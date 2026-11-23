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
    let words: Vec<&str> = game_name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3)
        .collect();
    for cand in &candidates {
        let stem = cand
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        if words.iter().any(|w| stem.contains(&w.to_lowercase())) {
            return Some((*cand).clone());
        }
    }
    // Fallback: the largest exe (usually the game binary).
    candidates
        .into_iter()
        .max_by_key(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
        .cloned()
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
    if stem_lower == "setup" {
        // Installer only when the folder has another exe to be the game.
        return siblings.iter().any(|s| s != "setup");
    }
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

/// Scan folders for game exes (max depth 4), filter installers, dedupe
/// by path. Never errors — missing folders are skipped silently.
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
        // setup.exe: installer only when a sibling exe exists.
        assert!(is_installer_exe(
            "setup",
            &["setup".to_string(), "game.exe".to_string()]
        ));
        assert!(!is_installer_exe("setup", &["setup".to_string()]));
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
