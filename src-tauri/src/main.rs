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
//! - v0.2.11: `quit_launcher` (exit from tray / Settings / Power /
//!   Alt+F4 when no game runs), `cancel_theme_music` + `is_youtube_blocked`
//!   (yt-dlp kill-on-change + session circuit-breaker, music.rs),
//!   `clear_art_music_cache` (art.rs), `get_last_played` (playtime.rs).
//!   Theme music is OFF by default (opt-in); the Settings panel was
//!   restructured into General / Library & Artwork / Audio / About
//!   with zero dead clicks.
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

#[tauri::command]
fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Quit the launcher entirely (v0.2.11 — the discoverable exit:
/// tray menu "Quit", Settings → About → "Exit Launcher", and the
/// Power menu "Exit Sonic Launcher" all call this; Alt+F4 / window
/// close also ends here when no game is running, via the run
/// callback letting the exit through).
#[tauri::command]
fn quit_launcher(app: AppHandle) {
    app.exit(0);
}

/// Kill the yt-dlp child currently running, if any — the frontend
/// calls this when the user selects another game (kill-on-change).
#[tauri::command]
fn cancel_theme_music() {
    music::cancel_ytdlp();
}

/// Whether YouTube lookups are circuit-broken for this session
/// (bot-block detected) — the frontend skips the YouTube steps.
#[tauri::command]
fn is_youtube_blocked() -> bool {
    music::is_youtube_blocked()
}

/// Clear the cached art + theme music (Settings → About).
#[tauri::command]
fn clear_art_music_cache(app: AppHandle) -> Result<String, String> {
    art::clear_art_music_cache(&app)
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

/// Unix time of the last finished play session for a game, or 0 when
/// never played (v0.2.11 — the hero "Last played ..." line only shows
/// when this is non-zero).
#[tauri::command]
fn get_last_played(app: AppHandle, game_key: String) -> i64 {
    playtime::get_last_played(&app, &game_key)
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
    // Window-stays-open launch path (setting OFF): a game is now
    // running, so Alt+F4 / window close must NOT quit the app — the
    // v0.2.10 tray-host behaviour holds until the session ends.
    session::set_game_running(true);
    playtime::start_session(&app, &game_key, &game_name)
}

#[tauri::command]
fn end_playtime_session(app: AppHandle, session_id: i64) -> Result<(), String> {
    session::set_game_running(false);
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
            // Close any play sessions left open by a previous run
            // (tray quit or crash mid-game) before anything else
            // reads playtime — stale sessions credit zero minutes.
            match playtime::close_stale_sessions(app.handle()) {
                Ok(closed) if closed > 0 => {
                    eprintln!("closed {closed} stale playtime session(s) from a previous run");
                }
                Ok(_) => {}
                Err(e) => eprintln!("could not close stale playtime sessions: {e}"),
            }
            // System tray: the ~4 MB host stays here while a game runs
            // and the window is destroyed. Clicking the icon brings
            // the launcher window back (recreating it if needed).
            //
            // v0.2.10: two things keep the host alive, and BOTH are
            // needed. (1) The tray icon is the only way back to the
            // window once it is destroyed — it must actually exist:
            // `default_window_icon()` is Option, so a missing icon
            // used to skip tray creation entirely. (2) A tray icon
            // alone does NOT keep the process alive: the Tauri
            // runtime asks to exit when the last window is
            // destroyed, and only the run callback in main() can
            // prevent that. Without (2) the v0.2.9 build exited the
            // whole process on Play — no host, no tray icon, no
            // window recreation, no fail-soft notice.
            //
            // The menu is also the only way to quit: the window is
            // borderless (no close button), and with exit prevented
            // the app never ends on its own. "Quit" calls app.exit,
            // which the run callback lets through (code Some).
            if let Some(icon) = app.default_window_icon() {
                let menu = tauri::menu::MenuBuilder::new(app)
                    .text("show", "Show Launcher")
                    .separator()
                    .text("quit", "Quit")
                    .build()?;
                let _tray = tauri::tray::TrayIconBuilder::with_id("main")
                    .icon(icon.clone())
                    .menu(&menu)
                    .tooltip("Sonic Launcher")
                    // Left-click shows the window, right-click shows the
                    // menu. The builder default is true (menu on left
                    // click too), which used to open the menu AND show
                    // the window on every click.
                    .show_menu_on_left_click(false)
                    .on_menu_event(|app, event| {
                        if event.id() == "quit" {
                            app.exit(0);
                        } else if event.id() == "show" {
                            let handle = app.clone();
                            let _ = session::show_or_recreate_window(&handle);
                        }
                    })
                    .on_tray_icon_event(|tray, event| {
                        // Left-button release ONLY: any click (incl.
                        // right-click, which also opens the menu) used
                        // to show the window over the running game.
                        // Field names verified against the vendored
                        // tauri 2.12.7 crate sources on this VM.
                        if let tauri::tray::TrayIconEvent::Click {
                            button: tauri::tray::MouseButton::Left,
                            button_state: tauri::tray::MouseButtonState::Up,
                            ..
                        } = event
                        {
                            let handle = tray.app_handle().clone();
                            // Never build a window on the main thread:
                            // the tray event arrives there, and Tauri
                            // warns that creating windows there can
                            // deadlock on Windows.
                            std::thread::spawn(move || {
                                let _ = session::show_or_recreate_window(&handle);
                            });
                        }
                    })
                    .build(app)?;
            } else {
                // No icon, no tray: destroying the window would
                // strand a live process with no way back to the UI.
                // Loud on stderr (visible when run from a terminal);
                // this must never happen in a real build — the icon
                // is embedded by tauri-build from src-tauri/icons/.
                eprintln!(
                    "WARNING: Sonic Launcher has no default window icon; no tray icon was created"
                );
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
            quit_launcher,
            cancel_theme_music,
            is_youtube_blocked,
            clear_art_music_cache,
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
            get_last_played,
            list_playtime,
            start_playtime_session,
            end_playtime_session,
            is_process_running,
            begin_managed_launch,
            show_launcher_window,
            get_pending_fast_exit
        ])
        .build(tauri::generate_context!())
        .expect("error while building Sonic Launcher")
        .run(|_app, event| match event {
            // v0.2.10: the runtime asks to exit (code None) when the
            // last window is destroyed. Destroying the window on game
            // launch must NOT end the process — the tray host stays
            // alive and recreates the window on game exit, and the
            // persisted fail-soft notice surfaces after the
            // recreate. Only an explicit exit request (code Some,
            // e.g. the tray "Quit" item calling app.exit) may end it.
            //
            // v0.2.11: the same exit is a real quit when NO game is
            // running (Alt+F4 / window close — the window is
            // borderless, so this is the only visible way out besides
            // tray / Settings / Power). The game-running flag is set
            // on launch (managed path + start_playtime_session) and
            // cleared on game exit.
            tauri::RunEvent::ExitRequested { api, code, .. } => {
                if session::should_prevent_exit(code, session::is_game_running()) {
                    api.prevent_exit();
                }
            }
            _ => {}
        });
}
