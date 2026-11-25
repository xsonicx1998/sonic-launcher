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
use std::collections::{HashMap, HashSet};
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
    /// Scanned games only: the game folder the exe was picked from.
    pub folder: Option<String>,
    /// Scanned games only: "high" / "medium" / "low" pick confidence.
    pub confidence: Option<String>,
    /// Scanned games only: which signal picked the exe (shortcut, known
    /// game, registry, acronym, ...).
    pub pick_source: Option<String>,
    /// Scanned games only: the user overrode the picked exe.
    pub has_override: bool,
    /// Scanned games only: a keyless art lookup failed for this game
    /// (Steam and GOG both missed) — the ••• menu offers Retry Art.
    pub art_failed: bool,
    /// Scanned games only: the tile is the game's own .exe icon
    /// (v0.2.6) — the frontend renders it centered on the Sonic
    /// gradient card instead of stretched full-bleed.
    pub art_is_icon: bool,
}

/// One exe found by the generic folder scan.
#[derive(Debug, Clone, Serialize)]
pub struct ScannedFound {
    pub name: String,
    pub exe_path: String,
    /// The game folder this exe was picked from.
    pub folder: String,
    /// "high" / "medium" / "low" — how much the pick is trusted.
    pub confidence: String,
    /// Which signal picked the exe (see `score_exe`).
    pub pick_source: String,
}

/// One ranked exe candidate, for the "Change Executable" picker.
#[derive(Debug, Clone, Serialize)]
pub struct RankedExe {
    pub exe_path: String,
    pub score: i32,
    pub source: String,
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

/// Pick the best exe from a list of candidates for one game folder.
/// Wrapper over `rank_exes` with no shortcut/registry signals (used
/// where the store already knows the exe, or no signals were collected).
/// `None` when the list is empty.
fn pick_best_exe(exes: &[PathBuf], folder_name: &str, known: &KnownGames) -> Option<PathBuf> {
    rank_exes(exes, folder_name, &HashSet::new(), &HashSet::new(), known)
        .into_iter()
        .next()
        .map(|r| PathBuf::from(r.exe_path))
}

fn best_exe_in_dir(dir: &Path, game_name: &str, known: &KnownGames) -> Option<PathBuf> {
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
    pick_best_exe(&owned, game_name, known)
}

/// GOG Galaxy games from the registry.
#[cfg(windows)]
pub fn detect_gog(known: &KnownGames) -> Vec<DetectedGame> {
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
        if let Some(exe) = best_exe_in_dir(&dir, &name, known) {
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
pub fn detect_gog(known: &KnownGames) -> Vec<DetectedGame> {
    let _ = known;
    Vec::new()
}

/// Ubisoft Connect games from the registry (name = install folder name).
#[cfg(windows)]
pub fn detect_ubisoft(known: &KnownGames) -> Vec<DetectedGame> {
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
        if let Some(exe) = best_exe_in_dir(&dir, &name, known) {
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
pub fn detect_ubisoft(known: &KnownGames) -> Vec<DetectedGame> {
    let _ = known;
    Vec::new()
}

/// EA games from the registry (EA App / Origin era keys).
#[cfg(windows)]
pub fn detect_ea(known: &KnownGames) -> Vec<DetectedGame> {
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
        if let Some(exe) = best_exe_in_dir(&dir, &name, known) {
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
pub fn detect_ea(known: &KnownGames) -> Vec<DetectedGame> {
    let _ = known;
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
    // The known-games table is loaded once per detection pass and
    // dropped after — see `load_known_games`.
    let known = load_known_games();
    let mut out = Vec::new();
    out.extend(detect_epic());
    out.extend(detect_gog(&known));
    out.extend(detect_ubisoft(&known));
    out.extend(detect_ea(&known));
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

/// Whether an exe file stem is an installer/updater/helper rather than a
/// game. This is the HARD denylist: a match is dropped unconditionally,
/// even when it is the only `.exe` in the folder. It is the one real
/// guarantee in the picker — we can never promise the right exe, but we
/// can promise never to pick (or launch) an uninstaller, cleaner, or
/// setup file.
///
/// - `setup` is ALWAYS an installer, even alone.
/// - Uninstallers / deleters / cleaners: `uninstall`, `unins`, `uninst`,
///   `remove`, `remover`, `delete`, `cleanup`, `cleaner`, `wipe`,
///   `purge`, `reset`, `repair`, `modify`.
/// - Installers / prerequisites: `install`, `installer`, `autorun`,
///   `bootstrap`, `redist`, `vcredist`, `vc_redist`, `dxsetup`,
///   `directx`, `oalinst`, `physx`, `dotnet`, `ue4prereq`, `ueprereq`.
/// - Crash / updaters / helpers / tools: `crashreporter`,
///   `crashhandler`, `unitycrashhandler`, `crashpad`, `bugsplat`,
///   `update`, `updater`, `benchmark`, `config`,
///   `easyanticheat_setup`, `battleye`, `beservice`, `dedicated`,
///   `server`.
/// - `launcher` is NEVER filtered: many games ship as `Launcher.exe`.
///
/// Pure — unit-tested. `siblings` is the list of lowercased exe stems in
/// the same directory (including this one); kept for signature
/// compatibility, the denylist no longer needs it.
pub fn is_installer_exe(stem_lower: &str, siblings: &[String]) -> bool {
    // `setup.exe` is ALWAYS an installer — even when it is the only exe in
    // a folder. Treating a lone installer as a game would add installers
    // to the library.
    if stem_lower == "setup" {
        return true;
    }
    const DENY: &[&str] = &[
        // Uninstallers / deleters / cleaners.
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
        // Installers / prerequisites.
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
        // Crash / updaters / helpers / tools.
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

// ---------------------------------------------------------------------------
// Exe picking: scoring (exe-guarantee round, v0.2.3).
//
// No heuristic can guarantee the right exe for every PC game, so the
// design is: a HARD denylist (never pick an uninstaller/cleaner/setup,
// even alone) + a scored pick over several signals + a 1-click
// "Change Executable" fallback in the UI. PE `FileVersionInfo`
// matching is deliberately deferred: the acronym + known-games table
// below already rescue the famous cryptic names (`bg3.exe`,
// `cod.exe`), and PE reading needs a new Windows API dependency.
// ---------------------------------------------------------------------------

/// Lowercase alphanumeric only — the canonical form for comparing
/// folder and exe names (`Cyberpunk 2077` -> `cyberpunk2077`).
fn normalize_name(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

/// A whole word that is a Roman numeral, as an Arabic digit (`V` -> `5`,
/// `III` -> `3`). Only whole words count — `vi` inside `civil` is not
/// a numeral.
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

/// Acronym (initialism) of a folder name, two variants: raw initials
/// (`Grand Theft Auto V` -> `gtav`, `The Elder Scrolls V` -> `tesv`)
/// and Roman-numeral-normalized initials (`Grand Theft Auto V` ->
/// `gta5`). Both are needed — `GTA5.exe` uses the digit, `TESV.exe`
/// uses the letter. Digits in the name contribute themselves
/// (`Red Dead Redemption 2` -> `rdr2`). A lone `s` word (the
/// possessive in `Baldur's Gate 3`) is skipped.
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
            // Raw keeps the Roman letter as the initial; normalized
            // swaps in the Arabic digit.
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

/// Whether an exe stem is the acronym of its folder (`GTA5.exe` for
/// `Grand Theft Auto V`, `RDR2.exe` for `Red Dead Redemption 2`,
/// `bg3_dx11.exe` for `Baldur's Gate 3`). Pure — unit-tested.
pub fn acronym_matches(stem_lower: &str, folder_name: &str) -> bool {
    let stem = normalize_name(stem_lower);
    if stem.is_empty() {
        return false;
    }
    let (raw, normalized) = acronym_variants(folder_name);
    // Single-letter acronyms match everything — require at least 2.
    (!raw.is_empty() && raw.len() >= 2 && (stem == raw || stem.starts_with(&raw)))
        || (!normalized.is_empty()
            && normalized.len() >= 2
            && (stem == normalized || stem.starts_with(&normalized)))
}

/// Bundled known-games table: normalized game title -> relative Windows
/// exe paths (lowercase, `/` separators). Generated at build time by
/// `scripts/generate_known_games.py` from the Discord detectable-apps
/// API plus a small curated overrides file, and embedded with
/// `include_str!`. This is the only true guarantee for these titles:
/// when the folder matches, a listed exe wins outright (unless a
/// shortcut or the registry says otherwise — those rank higher).
/// Paths are matched by suffix, so Discord's intermediate folders
/// (e.g. `tom clancy's rainbow six siege/rainbowsix.exe`) don't matter.
/// Loaded once per scan / store detection and dropped right after, so
/// it costs 0 MB of idle RAM.
pub type KnownGames = HashMap<String, Vec<String>>;

/// Parse the bundled known-games JSON. Malformed input yields an empty
/// table — the scan continues on the other signals. Pure — unit-tested.
pub fn parse_known_games(json: &str) -> KnownGames {
    serde_json::from_str(json).unwrap_or_default()
}

/// Load the bundled known-games table (embedded at compile time).
pub fn load_known_games() -> KnownGames {
    parse_known_games(include_str!("../known_games.json"))
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

/// Bonus points for where an exe lives, minus a penalty for utility
/// folders. Unreal `Binaries\Win64` and `bin\x64` are where the real
/// game binary lives; `Engine\`, `Tools\`, `Redist`, `Support`,
/// `Plugins`, `Mods`, `Editor` hold helpers, not the game. Capped at
/// +90. Pure except for the path string — unit-tested.
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

/// Whether a Unity game exe has its `<stem>_Data` folder next to it —
/// for Unity, that identifies the game exe exactly.
fn has_unity_data_dir(exe: &Path) -> bool {
    let Some(stem) = exe.file_stem().and_then(|s| s.to_str()) else {
        return false;
    };
    let Some(parent) = exe.parent() else {
        return false;
    };
    parent.join(format!("{stem}_Data")).is_dir()
}

/// Score one exe candidate for a game folder. Higher wins. Weights
/// (Jumani + Claude, merged):
/// - Desktop/Start shortcut target: +200 ("shortcut")
/// - Known-games table: +180 ("known game")
/// - Uninstall-registry DisplayIcon: +150 ("registry")
/// - Unity `<stem>_Data` next to the exe: +100 ("unity data")
/// - Acronym match: +90 ("acronym")
/// - Exe stem exactly the folder name (spaceless): +90 ("exact name")
/// - Path bonus (Binaries\Win64, bin\x64, Shipping): up to +90 ("path")
/// - Folder-word match (a word of 3+ chars from the folder name in the
///   stem): +50 ("name")
/// - Tiny exe (< 1 MB): -20 (stubs are small; real binaries rarely are)
///
/// "high" (>= 150) stays reserved for external evidence (shortcut,
/// known game, registry): the exact-name signal ties the acronym
/// tier at +90, so a bare `Minecraft.exe` in a "Minecraft" folder
/// (90 + 50 - 20 = 120) or a real one (90 + 50 = 140) scores
/// "medium" and no longer shows the false "Not sure this is the
/// right file" warning (v0.2.14, Round 2). Generic folder names
/// ("launcher", "game", "client", "main", "start", "play") are
/// excluded — a `launcher.exe` in a "Launcher" folder proves
/// nothing. Genuinely uncertain picks still warn: `fail.exe` in
/// "Fail Game" and `cod.exe` in "Call of Duty Modern Warfare II"
/// match nothing and stay "low".
///
/// Returns the score and the label of the biggest single contributor
/// (or "largest" when nothing contributed — the size tiebreak in
/// `rank_exes` decides then). Pure except for the Unity dir check and
/// file size — unit-tested.
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
    // v0.2.14: an exe whose stem IS the folder name (spaceless) is
    // the game's own binary — "HorizonZeroDawn.exe" in
    // "Horizon Zero Dawn", "Minecraft.exe" in "Minecraft". Generic
    // folder names are excluded (same list as
    // `game_name_for_group`): a `launcher.exe` in a "Launcher"
    // folder is not evidence of anything.
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

/// "high" (>= 150: a shortcut, known game, or registry hit),
/// "medium" (>= 60: a strong name/path/Unity signal), else "low".
pub fn confidence_of(score: i32) -> &'static str {
    if score >= 150 {
        "high"
    } else if score >= 60 {
        "medium"
    } else {
        "low"
    }
}

/// Rank exe candidates for one game folder, best first. Ties break by
/// file size (larger wins), then by path (deterministic). `shortcuts`
/// and `registry` are lowercased exe paths that a shortcut / the
/// uninstall registry point at. Pure except for the Unity dir check
/// and file sizes — unit-tested.
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

/// Target path of a Windows shortcut (`.lnk`) file, if it points at a
/// local file. Parses the documented MS-SHLLINK layout: 76-byte header
/// (signature `0x0000004C`), optional LinkTargetIDList, then LinkInfo
/// whose LocalBasePath is the target. Both ANSI and Unicode
/// LocalBasePath are handled. Pure — unit-tested with synthetic files.
pub fn parse_lnk_target(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 76 {
        return None;
    }
    // Header signature 0x0000004C ("L") at offset 0.
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
        // 2-byte size + the IDList itself + the 2-byte TerminalID (0x0000).
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
    // LinkInfoHeader: size +0, flags +4, VolumeIDOffset +8,
    // LocalBasePathOffset +12, CommonNetworkRelativeLinkOffset +16,
    // CommonPathSuffixOffset +20, LocalBasePathUnicodeOffset +24
    // (present in the 28-byte header; a 36-byte header adds the
    // CommonPathSuffixUnicodeOffset at +28).
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
    // Prefer the Unicode path when one is present (try both plausible
    // offsets — the header layout differs between 28- and 36-byte
    // variants — then fall back to the ANSI path, which is present in
    // virtually every shortcut).
    if let Some(s) = read_unicode(unicode_off) {
        return Some(s);
    }
    if let Some(s) = read_unicode(unicode_off2) {
        return Some(s);
    }
    read_ansi(local_off)
}

/// `.exe` targets of Desktop / Start Menu shortcuts, lowercased.
/// Best-effort: missing folders contribute nothing. On non-Windows the
/// env vars are absent, so this returns empty.
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

/// Strip the `,N` icon index from an uninstall-registry DisplayIcon
/// value (`C:\Game\Game.exe,0` -> `C:\Game\Game.exe`). Pure — tested.
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

/// Whether an icon/registry path points at an .exe inside the game
/// folder. Pure — tested.
pub fn icon_points_inside(icon: &str, folder: &str) -> bool {
    let icon_norm = icon.replace('\\', "/").to_lowercase();
    let folder_norm = folder.replace('\\', "/").to_lowercase();
    let folder_norm = folder_norm.trim_end_matches('/');
    icon_norm.ends_with(".exe") && icon_norm.starts_with(&format!("{folder_norm}/"))
}

/// `.exe` paths from the uninstall registry (DisplayIcon), lowercased:
/// the exe must exist, sit inside its InstallLocation, and survive the
/// hard denylist. Windows only — on other platforms this returns empty.
#[cfg(windows)]
pub fn collect_registry_exes() -> HashSet<String> {
    use winreg::enums::*;
    use winreg::RegKey;
    let mut out = HashSet::new();
    for (hive, base) in [
        (
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
        (
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
        (
            HKEY_CURRENT_USER,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
    ] {
        let Ok(key) = RegKey::predef(hive).open_subkey(base) else {
            continue;
        };
        for sub in key.enum_keys().flatten() {
            let Ok(subkey) = key.open_subkey(&sub) else {
                continue;
            };
            let location: String = match subkey.get_value("InstallLocation") {
                Ok(v) => v,
                Err(_) => continue,
            };
            let icon: String = match subkey.get_value("DisplayIcon") {
                Ok(v) => v,
                Err(_) => continue,
            };
            let icon = strip_icon_index(&icon).trim().to_string();
            if !icon.to_lowercase().ends_with(".exe") || !Path::new(&icon).is_file() {
                continue;
            }
            if !icon_points_inside(&icon, &location) {
                continue;
            }
            let stem = Path::new(&icon)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if is_installer_exe(&stem, &[]) {
                continue;
            }
            out.insert(icon.to_lowercase());
        }
    }
    out
}

#[cfg(not(windows))]
pub fn collect_registry_exes() -> HashSet<String> {
    HashSet::new()
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
    // Shortcut + registry signals are collected once per scan, not once
    // per folder: they are the two strongest signals and the lookups
    // (walking Start Menu, reading the registry) are not free.
    let shortcuts = collect_shortcut_targets();
    let registry = collect_registry_exes();
    // Same for the known-games table: parsed once per scan, dropped after.
    let known = load_known_games();
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
        let ranked = rank_exes(&exes, &root_name, &shortcuts, &registry, &known);
        if let Some(best) = ranked.first() {
            let best_path = PathBuf::from(&best.exe_path);
            let stem = best_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if exe_stem_matches_folder(&stem, &root_name) {
                let name = game_name_for_group(&root_name, &best_path);
                push_found(&mut out, &mut seen, &best_path, &name, &dir, best);
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
            let key = p.to_string_lossy().to_lowercase();
            let (score, source) = score_exe(
                &p,
                &root_name,
                shortcuts.contains(&key),
                registry.contains(&key),
                &known,
            );
            let rank = RankedExe {
                exe_path: p.to_string_lossy().into_owned(),
                score,
                source: source.to_string(),
            };
            push_found(&mut out, &mut seen, &p, &name, &dir, &rank);
        }
        for (game_dir, group) in by_dir {
            let folder_name = game_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Game")
                .to_string();
            let ranked = rank_exes(&group, &folder_name, &shortcuts, &registry, &known);
            if let Some(best) = ranked.first() {
                let best_path = PathBuf::from(&best.exe_path);
                let name = game_name_for_group(&folder_name, &best_path);
                push_found(&mut out, &mut seen, &best_path, &name, &game_dir, best);
            }
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Ranked exe candidates inside one game folder, for the "Change
/// Executable" picker. Re-walks the folder (max depth 4) and scores
/// every non-denylisted exe with the shortcut/registry signals.
/// Returns empty when the folder is missing or has no candidates.
pub fn list_exe_candidates(folder: &str) -> Vec<RankedExe> {
    let dir = PathBuf::from(folder);
    if !dir.is_dir() {
        return Vec::new();
    }
    let folder_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
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
        exes.push(p);
    }
    let shortcuts = collect_shortcut_targets();
    let registry = collect_registry_exes();
    let known = load_known_games();
    rank_exes(&exes, &folder_name, &shortcuts, &registry, &known)
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

fn push_found(
    out: &mut Vec<ScannedFound>,
    seen: &mut HashSet<String>,
    exe: &Path,
    name: &str,
    folder: &Path,
    rank: &RankedExe,
) {
    let key = exe.to_string_lossy().to_lowercase();
    if !seen.insert(key) {
        return;
    }
    out.push(ScannedFound {
        name: name.to_string(),
        exe_path: exe.to_string_lossy().into_owned(),
        folder: folder.to_string_lossy().into_owned(),
        confidence: confidence_of(rank.score).to_string(),
        pick_source: rank.source.clone(),
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
                folder: None,
                confidence: None,
                pick_source: None,
                has_override: false,
                art_failed: false,
                art_is_icon: false,
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
                    folder: None,
                    confidence: None,
                    pick_source: None,
                    has_override: false,
                    art_failed: false,
                    art_is_icon: false,
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
            folder: None,
            confidence: None,
            pick_source: None,
            has_override: false,
            art_failed: false,
            art_is_icon: false,
        });
    }

    if let Ok(scanned) = db::list_scanned_games(app) {
        for s in scanned {
            let effective = s.effective_exe();
            let key = effective.to_lowercase();
            if seen_exe.insert(key) {
                out.push(UnifiedGame {
                    key: format!("scan-{}", s.id),
                    name: s.name,
                    source: "scan".to_string(),
                    appid: None,
                    exe_path: Some(effective),
                    launch_uri: None,
                    folder: Some(s.folder),
                    confidence: Some(s.confidence),
                    pick_source: Some(s.pick_source),
                    has_override: s.exe_override.is_some(),
                    art_failed: s.art_lookup_failed_at.is_some(),
                    art_is_icon: s.art_source.as_deref() == Some("icon"),
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

    #[test]
    fn is_installer_exe_hard_denylist_never_picks_dangerous_exes() {
        let empty: Vec<String> = vec![];
        // New hard-denylist words (v0.2.3): uninstallers, deleters,
        // cleaners, installers, prerequisites, crash/update helpers,
        // dedicated servers.
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
            "vc_redist.x64",
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
        // The denylist must not over-fire on real game names.
        assert!(!is_installer_exe("launcher", &empty));
        assert!(!is_installer_exe("remnant2", &empty));
        assert!(!is_installer_exe("fortniteclient-win64-shipping", &empty));
        assert!(!is_installer_exe("game", &empty));
        assert!(!is_installer_exe("setup_game", &empty));
        assert!(!is_installer_exe("code", &empty));
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
        // No false match on unrelated stems, and single-letter
        // acronyms never match.
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
    fn score_exe_ranks_known_game_and_unity_first() {
        let empty: HashSet<String> = HashSet::new();
        let known = parse_known_games(
            r#"{"cyberpunk2077":["bin/x64/cyberpunk2077.exe"],"grandtheftautov":["gta5.exe"],"baldursgate3":["bin/bg3.exe","bin/bg3_dx11.exe"]}"#,
        );
        // Cyberpunk: the known-table exe beats the root stub.
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
        // Call of Duty: `cod.exe` is NOT in the known table (too
        // ambiguous across CoD titles) — the acronym alone must win.
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
        assert_eq!(ranked[0].source, "acronym");
        // Unity: the exe with a `<stem>_Data` folder beats a larger decoy.
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
        info[4..8].copy_from_slice(&1u32.to_le_bytes()); // VolumeIDAndLocalBasePath
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
        // With an IDList (the common real-world case) the LinkInfo
        // offset must still land correctly.
        assert_eq!(
            parse_lnk_target(&make_lnk(target, true, false)).as_deref(),
            Some(target)
        );
        // Unicode path preferred when present.
        assert_eq!(
            parse_lnk_target(&make_lnk(target, true, true)).as_deref(),
            Some(target)
        );
        // Garbage is rejected, not mis-parsed.
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
        // A comma inside the name is not an icon index.
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

    #[test]
    fn scan_folders_reports_confidence_and_source() {
        let game_root = std::env::temp_dir().join("Cyberpunk 2077");
        let _ = std::fs::remove_dir_all(&game_root);
        std::fs::create_dir_all(game_root.join("bin").join("x64")).unwrap();
        std::fs::write(
            game_root.join("bin").join("x64").join("Cyberpunk2077.exe"),
            b"xx",
        )
        .unwrap();
        let ignored: HashSet<String> = HashSet::new();
        let found = scan_folders(&[game_root.to_string_lossy().into_owned()], &ignored);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].confidence, "high");
        assert_eq!(found[0].pick_source, "known game");
        assert!(found[0].folder.ends_with("Cyberpunk 2077"), "{found:?}");
        let _ = std::fs::remove_dir_all(&game_root);
    }
}
