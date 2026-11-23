//! Steam detection: install dir, library folders, installed games.
//!
//! Windows-only registry access lives behind `cfg(windows)`; on any other
//! platform (or when Steam is absent / files are malformed) every function
//! degrades to an empty result instead of erroring the UI.

use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

/// One installed Steam game, as exposed to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct SteamGame {
    pub appid: u32,
    pub name: String,
    pub installdir: String,
    pub library_path: String,
}

/// Find the Steam install directory.
///
/// Registry `HKCU\Software\Valve\Steam\SteamPath` first, then the two
/// standard install locations.
pub fn steam_install_dir() -> Option<PathBuf> {
    if let Some(p) = steam_path_from_registry() {
        if p.is_dir() {
            return Some(p);
        }
    }
    for candidate in [r"C:\Program Files (x86)\Steam", r"C:\Program Files\Steam"] {
        let p = PathBuf::from(candidate);
        if p.is_dir() {
            return Some(p);
        }
    }
    None
}

#[cfg(windows)]
fn steam_path_from_registry() -> Option<PathBuf> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu.open_subkey(r"Software\Valve\Steam").ok()?;
    let path: String = key.get_value("SteamPath").ok()?;
    let path = path.trim();
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

#[cfg(not(windows))]
fn steam_path_from_registry() -> Option<PathBuf> {
    None
}

/// Read Steam's `RunningAppID` registry value (the appid of the game Steam
/// is currently running, if any). `None` when Steam is not running a game.
#[cfg(windows)]
pub fn running_appid() -> Option<u32> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu.open_subkey(r"Software\Valve\Steam").ok()?;
    let v: u32 = key.get_value("RunningAppID").ok()?;
    if v == 0 {
        None
    } else {
        Some(v)
    }
}

#[cfg(not(windows))]
pub fn running_appid() -> Option<u32> {
    None
}

/// Detect every installed Steam game. Never errors: returns an empty list
/// when Steam is not installed or its files are missing/malformed.
pub fn detect_steam_games() -> Vec<SteamGame> {
    let Some(steam_dir) = steam_install_dir() else {
        return Vec::new();
    };
    let mut libraries = library_paths(&steam_dir);
    // The main install dir is always a library, even if libraryfolders.vdf
    // is missing or unparsable.
    let main_lib = steam_dir.clone();
    if !libraries.iter().any(|p| p == &main_lib) {
        libraries.insert(0, main_lib);
    }
    let mut games = Vec::new();
    for lib in libraries {
        games.extend(games_in_library(&lib));
    }
    games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    games.dedup_by(|a, b| a.appid == b.appid);
    games
}

/// Parse `<steam>/steamapps/libraryfolders.vdf` and return every library
/// `"path"` value. Valve KeyValue text; paths use escaped backslashes
/// (`C:\\Program Files (x86)\\Steam`).
fn library_paths(steam_dir: &Path) -> Vec<PathBuf> {
    let vdf = steam_dir.join("steamapps").join("libraryfolders.vdf");
    let Ok(text) = fs::read_to_string(&vdf) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for raw in find_path_values(&text) {
        let unescaped = unescape_kv(&raw);
        let p = PathBuf::from(unescaped);
        if p.is_dir() && !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// Read every `appmanifest_*.acf` in `<library>/steamapps`.
fn games_in_library(library: &Path) -> Vec<SteamGame> {
    let steamapps = library.join("steamapps");
    let Ok(entries) = fs::read_dir(&steamapps) else {
        return Vec::new();
    };
    let mut games = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("appmanifest_") || !name.ends_with(".acf") {
            continue;
        }
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue;
        };
        if let Some(game) = parse_appmanifest(&text, library) {
            games.push(game);
        }
    }
    games
}

/// Extract `appid`, `name` and `installdir` from one appmanifest.
fn parse_appmanifest(text: &str, library: &Path) -> Option<SteamGame> {
    let appid: u32 = kv_string(text, "appid")?.parse().ok()?;
    let name = kv_string(text, "name")?;
    let installdir = kv_string(text, "installdir")?;
    if name.is_empty() || installdir.is_empty() {
        return None;
    }
    Some(SteamGame {
        appid,
        name,
        installdir,
        library_path: library.to_string_lossy().into_owned(),
    })
}

/// Extract the value of `"key"  "value"` from Valve KeyValue text.
fn kv_string<'a>(text: &'a str, key: &str) -> Option<String> {
    // Keys and values are both double-quoted; whitespace between them is
    // tabs/spaces. Values may contain escaped backslashes and quotes.
    let mut rest = text;
    loop {
        let q = rest.find('"')?;
        rest = &rest[q + 1..];
        let end = rest.find('"')?;
        let found_key = &rest[..end];
        rest = &rest[end + 1..];
        if found_key != key {
            continue;
        }
        let rest = rest.trim_start();
        if !rest.starts_with('"') {
            continue;
        }
        let rest = &rest[1..];
        let mut value = String::new();
        let mut chars = rest.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    // Unescape the next character (\" or \\).
                    if let Some(n) = chars.next() {
                        value.push(n);
                    }
                }
                '"' => return Some(value),
                _ => value.push(c),
            }
        }
        return None;
    }
}

/// Scan KeyValue text for every `"path"  "value"` pair. Returns the raw
/// (still escaped) values; callers unescape with [`unescape_kv`].
fn find_path_values(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(q) = rest.find("\"path\"") {
        rest = &rest[q + 6..];
        let rest_trim = rest.trim_start();
        if !rest_trim.starts_with('"') {
            continue;
        }
        let inner = &rest_trim[1..];
        let mut value = String::new();
        let mut chars = inner.chars();
        let mut closed = false;
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    if let Some(n) = chars.next() {
                        // Keep the escape pair; unescaped later.
                        value.push('\\');
                        value.push(n);
                    }
                }
                '"' => {
                    closed = true;
                    break;
                }
                _ => value.push(c),
            }
        }
        if closed {
            out.push(value);
        }
        rest = rest_trim;
    }
    out
}

/// Unescape a Valve KeyValue string (`\\` -> `\`, `\"` -> `"`).
fn unescape_kv(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmpdir(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn unescape_kv_handles_backslashes_and_quotes() {
        assert_eq!(
            unescape_kv(r"C:\\Program Files (x86)\\Steam"),
            r"C:\Program Files (x86)\Steam"
        );
        assert_eq!(unescape_kv(r#"say \"hi\""#), r#"say "hi""#);
        assert_eq!(unescape_kv("plain"), "plain");
    }

    #[test]
    fn find_path_values_extracts_escaped_windows_paths() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t\t\"label\"\t\t\"\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\Games\\\\SteamLib\"\n\t}\n}\n";
        let found = find_path_values(vdf);
        assert_eq!(found.len(), 2);
        assert_eq!(unescape_kv(&found[0]), r"C:\Program Files (x86)\Steam");
        assert_eq!(unescape_kv(&found[1]), r"D:\Games\SteamLib");
    }

    #[test]
    fn kv_string_reads_quoted_pairs() {
        let acf = "\"AppState\"\n{\n\t\"appid\"\t\t\"440\"\n\t\"name\"\t\t\"Team Fortress 2\"\n\t\"installdir\"\t\t\"Team Fortress 2\"\n}\n";
        assert_eq!(kv_string(acf, "appid").as_deref(), Some("440"));
        assert_eq!(kv_string(acf, "name").as_deref(), Some("Team Fortress 2"));
        assert_eq!(
            kv_string(acf, "installdir").as_deref(),
            Some("Team Fortress 2")
        );
        assert_eq!(kv_string(acf, "missing"), None);
    }

    #[test]
    fn parse_appmanifest_valid_and_malformed() {
        let lib = PathBuf::from(r"C:\Steam");
        let ok = parse_appmanifest(
            "\"AppState\"\n{\n\t\"appid\"\t\t\"570\"\n\t\"name\"\t\t\"Dota 2\"\n\t\"installdir\"\t\t\"dota 2 beta\"\n}\n",
            &lib,
        );
        let g = ok.expect("valid manifest parses");
        assert_eq!(g.appid, 570);
        assert_eq!(g.name, "Dota 2");
        assert_eq!(g.installdir, "dota 2 beta");
        assert_eq!(g.library_path, lib.to_string_lossy());
        assert!(
            parse_appmanifest("\"AppState\"\n{\n\t\"name\"\t\t\"Broken\"\n}\n", &lib).is_none()
        );
        assert!(parse_appmanifest("\"AppState\"\n{\n\t\"appid\"\t\t\"not-a-number\"\n\t\"name\"\t\t\"X\"\n\t\"installdir\"\t\t\"X\"\n}\n", &lib).is_none());
    }

    #[test]
    fn games_in_library_reads_manifests_and_skips_bad_ones() {
        let lib = tmpdir("p1-verify-lib");
        let steamapps = lib.join("steamapps");
        fs::create_dir_all(&steamapps).unwrap();
        fs::write(
            steamapps.join("appmanifest_440.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"440\"\n\t\"name\"\t\t\"Team Fortress 2\"\n\t\"installdir\"\t\t\"Team Fortress 2\"\n}\n",
        )
        .unwrap();
        fs::write(
            steamapps.join("appmanifest_999.acf"),
            "\"AppState\"\n{\n\t\"name\"\t\t\"Broken\"\n}\n",
        )
        .unwrap();
        fs::write(steamapps.join("not-a-manifest.txt"), "junk").unwrap();
        let games = games_in_library(&lib);
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].appid, 440);
        assert_eq!(games[0].name, "Team Fortress 2");
    }

    #[test]
    fn library_paths_reads_vdf_and_ignores_missing_dirs() {
        let steam = tmpdir("p1-verify-steam2");
        let lib2 = tmpdir("p1-verify-lib2");
        let steamapps = steam.join("steamapps");
        fs::create_dir_all(&steamapps).unwrap();
        let vdf = format!(
            "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"2\"\n\t{{\n\t\t\"path\"\t\t\"/does/not/exist\"\n\t}}\n}}\n",
            steam.to_string_lossy(),
            lib2.to_string_lossy(),
        );
        fs::write(steamapps.join("libraryfolders.vdf"), vdf).unwrap();
        let paths = library_paths(&steam);
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&steam));
        assert!(paths.contains(&lib2));
        assert!(library_paths(&tmpdir("p1-verify-nosteam")).is_empty());
    }
}
