//! Sonic Launcher — Phase 1.
//!
//! Backend commands:
//! - `detect_steam_games` — Steam library scan (steam.rs)
//! - `launch_game` — Steam URL or manual .exe (launch.rs)
//! - `add_manual_game` / `list_manual_games` / `remove_manual_game`
//!   — SQLite-backed manual games (db.rs)
//! - `get_running_appid` — Steam's currently running game (steam.rs)
//!
//! The UI is the v33 PS5-style single-file preview (dist/index.html).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod db;
mod launch;
mod steam;

use tauri::AppHandle;

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

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                window.open_devtools();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            detect_steam_games,
            get_running_appid,
            launch_game,
            add_manual_game,
            list_manual_games,
            remove_manual_game
        ])
        .run(tauri::generate_context!())
        .expect("error while running Sonic Launcher");
}
