//! Sonic Launcher — Phases 1–5.
//!
//! Backend commands:
//! - Phase 1: `detect_steam_games`, `launch_game`, `add_manual_game` /
//!   `list_manual_games` / `remove_manual_game`, `get_running_appid`
//! - Phase 2: `get_game_art`, `set_steamgriddb_key`,
//!   `is_steamgriddb_key_set` (art.rs)
//! - v0.2.5: keyless cover lookup (Steam store search, then GOG
//!   Catalog) with a background worker after a scan; manual cover
//!   commands `pick_cover_art`, `set_cover_art_appid`,
//!   `set_cover_art_url`, `retry_art` (art.rs). The "Auto-fetch
//!   cover art" toggle uses the generic `get_setting` / `set_setting`
//!   with key `auto_fetch_art` (ON by default).
//! - v0.2.6: exe-icon fallback (art.rs + exe_icon.rs) — the game's
//!   own .exe icon, centered on the gradient card, when no other
//!   art source has anything.
//! - Phase 3: `ensure_ytdlp`, `search_theme_music`,
//!   `download_theme_music`, `get_game_music`, `pick_music_file`,
//!   `get_music_data_uri` (music.rs)
//! - Phase 4: `detect_other_stores`, `scan_folders`, `list_all_games`,
//!   `add_scan_folder` / `list_scan_folders` / `remove_scan_folder`
//!   (stores.rs + db.rs settings)
//! - Exe guarantee (v0.2.3): `list_exe_candidates`, `set_exe_override`
//!   (stores.rs scoring + db.rs override columns), fail-soft launch
//!   watch in `launch.rs`
//! - Phase 5: `get_setting` / `set_setting`, `get_playtime`,
//!   `list_playtime`, `start_playtime_session` / `end_playtime_session`,
//!   `is_process_running` (playtime.rs)
//!
//! The UI is the v33 PS5-style single-file preview (dist/index.html).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod art;
mod db;
mod exe_icon;
mod http;
mod khinsider;
mod launch;
mod music;
mod playtime;
mod session;
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
fn launch_game(
    app: AppHandle,
    appid: Option<u32>,
    exe_path: Option<String>,
    game_name: Option<String>,
    game_key: Option<String>,
) -> Result<(), String> {
    launch::launch_game(&app, appid, exe_path, game_name, game_key)
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
    exe_path: Option<String>,
) -> art::GameArt {
    art::get_game_art(&app, &game_key, appid, &name, exe_path.as_deref())
}

#[tauri::command]
fn set_steamgriddb_key(app: AppHandle, key: String) -> Result<(), String> {
    art::set_steamgriddb_key(&app, &key)
}

#[tauri::command]
fn is_steamgriddb_key_set(app: AppHandle) -> bool {
    art::is_steamgriddb_key_set(&app)
}

#[tauri::command]
fn pick_cover_art(app: AppHandle, game_key: String) -> Result<Option<String>, String> {
    art::pick_cover_image(&app, &game_key)
}

#[tauri::command]
fn set_cover_art_appid(app: AppHandle, game_key: String, appid: u32) -> Result<(), String> {
    art::set_cover_art_appid(&app, &game_key, appid)
}

#[tauri::command]
fn set_cover_art_url(app: AppHandle, game_key: String, url: String) -> Result<(), String> {
    art::set_cover_art_url(&app, &game_key, &url)
}

#[tauri::command]
fn retry_art(app: AppHandle, game_key: String, name: String) -> Result<String, String> {
    art::retry_art(&app, &game_key, &name)
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
    fallback: Option<bool>,
    source: Option<String>,
) -> Result<Vec<music::MusicCandidate>, String> {
    music::search_theme_music(
        &app,
        &game_name,
        fallback.unwrap_or(false),
        source.as_deref().unwrap_or("youtube"),
    )
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
fn download_theme_music_url(
    app: AppHandle,
    game_key: String,
    url: String,
    source: Option<String>,
) -> Result<String, String> {
    music::download_theme_music_from_url(&app, &game_key, &url, source.as_deref().unwrap_or("url"))
}

#[tauri::command]
fn pick_ambient_file(app: AppHandle) -> Result<Option<String>, String> {
    music::pick_ambient_file(&app)
}

/// KHInsider fallback: search + download the game's theme track in one
/// call. Returns the cached path, or `None` when KHInsider has nothing
/// for the game (or is blocked) — the caller falls through to the next
/// source.
#[tauri::command]
fn download_khinsider_theme(
    app: AppHandle,
    game_key: String,
    game_name: String,
) -> Result<Option<String>, String> {
    let track = khinsider::search_khinsider_track(&game_name)?;
    let Some(track) = track else {
        return Ok(None);
    };
    let path = khinsider::download_khinsider_track(&app, &game_key, &track)?;
    Ok(Some(path))
}

/// The user-overridable SFX folder: `%APPDATA%\com.sonic.launcher\sfx\`.
/// Created on startup so users can drop their own sound pack here.
/// The SFX engine looks here first, then falls back to `dist/sfx/`.
#[tauri::command]
fn get_sfx_dir(app: AppHandle) -> Result<String, String> {
    use tauri::Manager;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("cannot resolve app data dir: {e}"))?
        .join("sfx");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create sfx dir: {e}"))?;
    Ok(dir.to_string_lossy().into_owned())
}

/// Open the SFX folder in Windows Explorer.
#[tauri::command]
fn open_sfx_folder(app: AppHandle) -> Result<(), String> {
    let dir = get_sfx_dir(app)?;
    open::that(dir).map_err(|e| format!("cannot open sfx folder: {e}"))?;
    Ok(())
}

#[tauri::command]
fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
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
    let ignored: std::collections::HashSet<String> = db::list_ignored_paths(&app)
        .unwrap_or_default()
        .into_iter()
        .collect();
    let found = stores::scan_folders(&folders, &ignored);
    for f in &found {
        let _ = db::add_scanned_game(
            &app,
            &f.name,
            &f.exe_path,
            &f.folder,
            &f.confidence,
            &f.pick_source,
        );
    }
    // Keyless cover lookup runs in the background after the scan:
    // sequential, throttled, and it never blocks the scan result.
    let worker_app = app.clone();
    let _ = std::thread::spawn(move || art::run_art_lookup_worker(&worker_app));
    found
}

#[tauri::command]
fn remove_scanned_game(app: AppHandle, id: i64) -> Result<(), String> {
    db::remove_scanned_game(&app, id)
}

/// The folder to rank candidates in for a scanned game. Legacy rows
/// (pre-v0.2.3) have no folder: use the parent dir of the current exe.
fn scanned_game_folder(game: &db::ScannedGame) -> String {
    if !game.folder.is_empty() {
        return game.folder.clone();
    }
    std::path::Path::new(&game.effective_exe())
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[tauri::command]
fn list_exe_candidates(app: AppHandle, id: i64) -> Result<Vec<stores::RankedExe>, String> {
    let game =
        db::get_scanned_game(&app, id)?.ok_or_else(|| format!("no scanned game with id {id}"))?;
    Ok(stores::list_exe_candidates(&scanned_game_folder(&game)))
}

#[tauri::command]
fn set_exe_override(app: AppHandle, id: i64, exe_path: Option<String>) -> Result<(), String> {
    match exe_path {
        Some(path) => db::set_scanned_exe_override(&app, id, &path),
        None => {
            // Clear the override, then re-rank the folder so the
            // auto-pick (not the stale exe) takes over again.
            let game = db::get_scanned_game(&app, id)?
                .ok_or_else(|| format!("no scanned game with id {id}"))?;
            db::clear_scanned_exe_override(&app, id)?;
            let candidates = stores::list_exe_candidates(&scanned_game_folder(&game));
            if let Some(top) = candidates.first() {
                let _ = db::update_scanned_pick(
                    &app,
                    id,
                    &top.exe_path,
                    stores::confidence_of(top.score),
                    &top.source,
                );
            }
            Ok(())
        }
    }
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

// --- v0.2.9: destroy the window on launch -------------------------------

/// Start a managed game session (see session.rs): start the playtime
/// session, spawn the exit watcher, then destroy the launcher window.
/// The watcher recreates the window when the game exits.
#[tauri::command]
fn begin_managed_launch(
    app: AppHandle,
    source: String,
    appid: Option<u32>,
    exe_path: Option<String>,
    game_key: String,
    game_name: String,
) -> Result<(), String> {
    session::begin_managed_launch(&app, &source, appid, exe_path, &game_key, &game_name)
}

/// Show the launcher window, recreating it first if it was destroyed
/// (e.g. while a game was running). The tray click uses the same path.
#[tauri::command]
fn show_launcher_window(app: AppHandle) -> Result<(), String> {
    session::show_or_recreate_window(&app)
}

/// Take (read + delete) the pending fast-exit notice the backend
/// recorded, if there is a fresh one. The frontend calls this on
/// startup and after every window restore — the live event alone can
/// be lost while the window is hidden or destroyed.
#[tauri::command]
fn get_pending_fast_exit(app: AppHandle) -> Option<launch::FastExit> {
    launch::take_pending_fast_exit(&app)
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // System tray: the ~4 MB host stays here while a game runs
            // and the window is destroyed. Clicking the icon brings
            // the launcher window back (recreating it if needed).
            if let Some(icon) = app.default_window_icon() {
                let _tray = tauri::tray::TrayIconBuilder::with_id("main")
                    .icon(icon.clone())
                    .tooltip("Sonic Launcher")
                    .on_tray_icon_event(|tray, event| {
                        if let tauri::tray::TrayIconEvent::Click { .. } = event {
                            let handle = tray.app_handle().clone();
                            let _ = session::show_or_recreate_window(&handle);
                        }
                    })
                    .build(app)?;
            }
            Ok(())
        })
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
            pick_cover_art,
            set_cover_art_appid,
            set_cover_art_url,
            retry_art,
            ensure_ytdlp,
            search_theme_music,
            download_theme_music,
            download_theme_music_url,
            download_khinsider_theme,
            get_sfx_dir,
            open_sfx_folder,
            pick_ambient_file,
            get_app_version,
            get_game_music,
            pick_music_file,
            get_music_data_uri,
            detect_other_stores,
            scan_folders,
            remove_scanned_game,
            list_exe_candidates,
            set_exe_override,
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
            is_process_running,
            begin_managed_launch,
            show_launcher_window,
            get_pending_fast_exit
        ])
        .run(tauri::generate_context!())
        .expect("error while running Sonic Launcher");
}
