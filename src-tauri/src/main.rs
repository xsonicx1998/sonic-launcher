//! Sonic Launcher — Phases 1–5.
//!
//! Backend commands:
//! - Phase 1: `detect_steam_games`, `launch_game`, `add_manual_game` /
//!   `list_manual_games` / `remove_manual_game`, `get_running_appid`
//! - Phase 2: `get_game_art`, `set_steamgriddb_key`,
//!   `is_steamgriddb_key_set` (art.rs)
//! - Phase 3: `ensure_ytdlp`, `search_theme_music`,
//!   `download_theme_music`, `get_game_music`, `pick_music_file`,
//!   `get_music_data_uri` (music.rs)
//! - Phase 4: `detect_other_stores`, `scan_folders`, `list_all_games`,
//!   `add_scan_folder` / `list_scan_folders` / `remove_scan_folder`
//!   (stores.rs + db.rs settings)
//! - Phase 5: `get_setting` / `set_setting`, `get_playtime`,
//!   `list_playtime`, `start_playtime_session` / `end_playtime_session`,
//!   `is_process_running` (playtime.rs)
//!
//! The UI is the v33 PS5-style single-file preview (dist/index.html).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod art;
mod db;
mod http;
mod launch;
mod music;
mod playtime;
mod steam;
mod stores;

use tauri::AppHandle;

// --- Phase 1 ---------------------------------------------------------------

#[tauri::command]
fn detect_steam_games() -> Vec<steam::SteamGame> {
    steam::detect_steam_games()
}

#[tauri::command]
fn get_running_appid() -> Option<u32> {
    steam::running_appid()
}

#[tauri::command]
fn launch_game(appid: Option<u32>, exe_path: Option<String>) -> Result<(), String> {
    launch::launch_game(appid, exe_path)
}

#[tauri::command]
fn add_manual_game(app: AppHandle, name: String, exe_path: String) -> Result<i64, String> {
    db::add_manual_game(&app, &name, &exe_path)
}

#[tauri::command]
fn list_manual_games(app: AppHandle) -> Result<Vec<db::ManualGame>, String> {
    db::list_manual_games(&app)
}

#[tauri::command]
fn remove_manual_game(app: AppHandle, id: i64) -> Result<(), String> {
    db::remove_manual_game(&app, id)
}

// --- Phase 2: art ----------------------------------------------------------

#[tauri::command]
fn get_game_art(
    app: AppHandle,
    game_key: String,
    appid: Option<u32>,
    name: String,
) -> art::GameArt {
    art::get_game_art(&app, &game_key, appid, &name)
}

#[tauri::command]
fn set_steamgriddb_key(app: AppHandle, key: String) -> Result<(), String> {
    art::set_steamgriddb_key(&app, &key)
}

#[tauri::command]
fn is_steamgriddb_key_set(app: AppHandle) -> bool {
    art::is_steamgriddb_key_set(&app)
}

// --- Phase 3: theme music --------------------------------------------------

#[tauri::command]
fn ensure_ytdlp(app: AppHandle) -> Result<String, String> {
    music::ensure_ytdlp(&app)
}

#[tauri::command]
fn search_theme_music(
    app: AppHandle,
    game_name: String,
) -> Result<Vec<music::MusicCandidate>, String> {
    music::search_theme_music(&app, &game_name)
}

#[tauri::command]
fn download_theme_music(
    app: AppHandle,
    game_key: String,
    video_id: String,
) -> Result<String, String> {
    music::download_theme_music(&app, &game_key, &video_id)
}

#[tauri::command]
fn get_game_music(app: AppHandle, game_key: String) -> Option<music::MusicInfo> {
    music::get_game_music(&app, &game_key)
}

#[tauri::command]
fn pick_music_file(app: AppHandle, game_key: String) -> Result<Option<String>, String> {
    music::pick_music_file(&app, &game_key)
}

#[tauri::command]
fn get_music_data_uri(app: AppHandle, game_key: String) -> Result<Option<String>, String> {
    music::get_music_data_uri(&app, &game_key)
}

// --- Phase 4: other stores + generic scan ---------------------------------

#[tauri::command]
fn detect_other_stores() -> Vec<stores::DetectedGame> {
    stores::detect_other_stores()
}

#[tauri::command]
fn scan_folders(app: AppHandle, folders: Vec<String>) -> Vec<stores::ScannedFound> {
    let found = stores::scan_folders(&folders);
    for f in &found {
        let _ = db::add_scanned_game(&app, &f.name, &f.exe_path);
    }
    found
}

#[tauri::command]
fn list_all_games(app: AppHandle) -> Vec<stores::UnifiedGame> {
    stores::list_all_games(&app)
}

fn scan_folders_setting(app: &AppHandle) -> Vec<String> {
    match db::get_setting(app, "scan_folders") {
        Ok(Some(v)) => serde_json::from_str(&v).unwrap_or_default(),
        _ => Vec::new(),
    }
}

#[tauri::command]
fn add_scan_folder(app: AppHandle, path: String) -> Result<Vec<String>, String> {
    let path = path.trim().to_string();
    if path.is_empty() {
        return Err("folder path is empty".to_string());
    }
    let mut folders = scan_folders_setting(&app);
    if !folders.iter().any(|f| f.eq_ignore_ascii_case(&path)) {
        folders.push(path);
    }
    db::set_setting(
        &app,
        "scan_folders",
        &serde_json::to_string(&folders).unwrap_or_else(|_| "[]".to_string()),
    )?;
    Ok(folders)
}

#[tauri::command]
fn list_scan_folders(app: AppHandle) -> Vec<String> {
    scan_folders_setting(&app)
}

#[tauri::command]
fn remove_scan_folder(app: AppHandle, path: String) -> Result<Vec<String>, String> {
    let folders: Vec<String> = scan_folders_setting(&app)
        .into_iter()
        .filter(|f| !f.eq_ignore_ascii_case(path.trim()))
        .collect();
    db::set_setting(
        &app,
        "scan_folders",
        &serde_json::to_string(&folders).unwrap_or_else(|_| "[]".to_string()),
    )?;
    Ok(folders)
}

// --- Phase 5: settings + playtime ------------------------------------------

#[tauri::command]
fn get_setting(app: AppHandle, key: String) -> Option<String> {
    db::get_setting(&app, &key).ok().flatten()
}

#[tauri::command]
fn set_setting(app: AppHandle, key: String, value: String) -> Result<(), String> {
    db::set_setting(&app, &key, &value)
}

#[tauri::command]
fn get_playtime(app: AppHandle, game_key: String) -> i64 {
    playtime::get_playtime(&app, &game_key).unwrap_or(0)
}

#[tauri::command]
fn list_playtime(app: AppHandle) -> Vec<db::PlaytimeTotal> {
    playtime::list_playtime(&app).unwrap_or_default()
}

#[tauri::command]
fn start_playtime_session(
    app: AppHandle,
    game_key: String,
    game_name: String,
) -> Result<i64, String> {
    playtime::start_session(&app, &game_key, &game_name)
}

#[tauri::command]
fn end_playtime_session(app: AppHandle, session_id: i64) -> Result<(), String> {
    playtime::end_session(&app, session_id)
}

#[tauri::command]
fn is_process_running(exe_name: String) -> bool {
    playtime::is_process_running(&exe_name)
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            detect_steam_games,
            get_running_appid,
            launch_game,
            add_manual_game,
            list_manual_games,
            remove_manual_game,
            get_game_art,
            set_steamgriddb_key,
            is_steamgriddb_key_set,
            ensure_ytdlp,
            search_theme_music,
            download_theme_music,
            get_game_music,
            pick_music_file,
            get_music_data_uri,
            detect_other_stores,
            scan_folders,
            list_all_games,
            add_scan_folder,
            list_scan_folders,
            remove_scan_folder,
            get_setting,
            set_setting,
            get_playtime,
            list_playtime,
            start_playtime_session,
            end_playtime_session,
            is_process_running
        ])
        .run(tauri::generate_context!())
        .expect("error while running Sonic Launcher");
}
