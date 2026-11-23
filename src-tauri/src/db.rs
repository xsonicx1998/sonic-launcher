//! Manually added games, stored in SQLite (`sonic-launcher.db` in the app
//! data dir). One connection is opened per call — the tables are tiny and
//! calls are rare, so no shared state is needed.
//!
//! Tables (Phase 2–5 additions):
//! - `settings` — key/value app settings (SteamGridDB key, music toggle,
//!   volume, scan folders as JSON, minimize-on-launch).
//! - `music` — per-game theme music choice (cached file path + title).
//! - `playtime` — one row per launch session (started/ended, minutes).
//! - `scanned_games` — games found by the generic folder scan.

use rusqlite::{params, Connection};
use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// One manually added game.
#[derive(Debug, Clone, Serialize)]
pub struct ManualGame {
    pub id: i64,
    pub name: String,
    pub exe_path: String,
    pub added_at: i64,
}

fn db_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("cannot resolve app data dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create app data dir: {e}"))?;
    Ok(dir.join("sonic-launcher.db"))
}

fn open(app: &AppHandle) -> Result<Connection, String> {
    let path = db_path(app)?;
    let conn = Connection::open(&path).map_err(|e| format!("cannot open database: {e}"))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS manual_games (
            id        INTEGER PRIMARY KEY AUTOINCREMENT,
            name      TEXT NOT NULL,
            exe_path  TEXT NOT NULL,
            added_at  INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS settings (
            key       TEXT PRIMARY KEY,
            value     TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS music (
            game_key      TEXT PRIMARY KEY,
            source        TEXT NOT NULL,
            path          TEXT NOT NULL,
            title         TEXT NOT NULL,
            downloaded_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS playtime (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            game_key   TEXT NOT NULL,
            game_name  TEXT NOT NULL,
            started_at INTEGER NOT NULL,
            ended_at   INTEGER,
            minutes    INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS scanned_games (
            id       INTEGER PRIMARY KEY AUTOINCREMENT,
            name     TEXT NOT NULL,
            exe_path TEXT NOT NULL UNIQUE,
            source   TEXT NOT NULL DEFAULT 'scan',
            added_at INTEGER NOT NULL
        );",
    )
    .map_err(|e| format!("cannot create table: {e}"))?;
    Ok(conn)
}

/// Add a manually picked .exe. Returns the new row id.
pub fn add_manual_game(app: &AppHandle, name: &str, exe_path: &str) -> Result<i64, String> {
    let name = name.trim();
    let exe_path = exe_path.trim();
    if name.is_empty() {
        return Err("game name is empty".to_string());
    }
    if exe_path.is_empty() {
        return Err("exe path is empty".to_string());
    }
    let conn = open(app)?;
    conn.execute(
        "INSERT INTO manual_games (name, exe_path, added_at) VALUES (?1, ?2, ?3)",
        params![name, exe_path, now_unix()],
    )
    .map_err(|e| format!("cannot add game: {e}"))?;
    Ok(conn.last_insert_rowid())
}

/// List all manually added games, newest first.
pub fn list_manual_games(app: &AppHandle) -> Result<Vec<ManualGame>, String> {
    let conn = open(app)?;
    let mut stmt = conn
        .prepare("SELECT id, name, exe_path, added_at FROM manual_games ORDER BY id DESC")
        .map_err(|e| format!("cannot list games: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ManualGame {
                id: row.get(0)?,
                name: row.get(1)?,
                exe_path: row.get(2)?,
                added_at: row.get(3)?,
            })
        })
        .map_err(|e| format!("cannot list games: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("cannot read game row: {e}"))?);
    }
    Ok(out)
}

/// Remove a manually added game by id.
pub fn remove_manual_game(app: &AppHandle, id: i64) -> Result<(), String> {
    let conn = open(app)?;
    let changed = conn
        .execute("DELETE FROM manual_games WHERE id = ?1", params![id])
        .map_err(|e| format!("cannot remove game: {e}"))?;
    if changed == 0 {
        return Err(format!("no manual game with id {id}"));
    }
    Ok(())
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Current Unix time (public — other modules need it too).
pub fn now() -> i64 {
    now_unix()
}

// ---------------------------------------------------------------------------
// Settings (Phase 5) — plain key/value store.
// ---------------------------------------------------------------------------

/// Read one setting. `None` when the key was never set.
pub fn get_setting(app: &AppHandle, key: &str) -> Result<Option<String>, String> {
    let conn = open(app)?;
    let mut stmt = conn
        .prepare("SELECT value FROM settings WHERE key = ?1")
        .map_err(|e| format!("cannot read setting: {e}"))?;
    let mut rows = stmt
        .query(params![key])
        .map_err(|e| format!("cannot read setting: {e}"))?;
    match rows.next() {
        Ok(Some(row)) => Ok(Some(
            row.get::<_, String>(0)
                .map_err(|e| format!("cannot read setting: {e}"))?,
        )),
        Ok(None) => Ok(None),
        Err(e) => Err(format!("cannot read setting: {e}")),
    }
}

/// Write one setting (insert or replace).
pub fn set_setting(app: &AppHandle, key: &str, value: &str) -> Result<(), String> {
    let conn = open(app)?;
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .map_err(|e| format!("cannot save setting: {e}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Theme music (Phase 3).
// ---------------------------------------------------------------------------

/// One cached theme-music choice.
#[derive(Debug, Clone, Serialize)]
pub struct MusicRow {
    pub game_key: String,
    pub source: String,
    pub path: String,
    pub title: String,
    pub downloaded_at: i64,
}

/// The cached music for a game, if any.
pub fn get_music(app: &AppHandle, game_key: &str) -> Result<Option<MusicRow>, String> {
    let conn = open(app)?;
    let mut stmt = conn
        .prepare(
            "SELECT game_key, source, path, title, downloaded_at FROM music WHERE game_key = ?1",
        )
        .map_err(|e| format!("cannot read music: {e}"))?;
    let mut rows = stmt
        .query(params![game_key])
        .map_err(|e| format!("cannot read music: {e}"))?;
    match rows.next() {
        Ok(Some(row)) => Ok(Some(MusicRow {
            game_key: row.get(0).map_err(|e| format!("cannot read music: {e}"))?,
            source: row.get(1).map_err(|e| format!("cannot read music: {e}"))?,
            path: row.get(2).map_err(|e| format!("cannot read music: {e}"))?,
            title: row.get(3).map_err(|e| format!("cannot read music: {e}"))?,
            downloaded_at: row.get(4).map_err(|e| format!("cannot read music: {e}"))?,
        })),
        Ok(None) => Ok(None),
        Err(e) => Err(format!("cannot read music: {e}")),
    }
}

/// Save (insert or replace) a game's theme-music choice.
pub fn save_music(
    app: &AppHandle,
    game_key: &str,
    source: &str,
    path: &str,
    title: &str,
) -> Result<(), String> {
    let conn = open(app)?;
    conn.execute(
        "INSERT INTO music (game_key, source, path, title, downloaded_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(game_key) DO UPDATE SET source = excluded.source, path = excluded.path,
         title = excluded.title, downloaded_at = excluded.downloaded_at",
        params![game_key, source, path, title, now_unix()],
    )
    .map_err(|e| format!("cannot save music: {e}"))?;
    Ok(())
}

/// Forget a game's theme-music choice.
pub fn delete_music(app: &AppHandle, game_key: &str) -> Result<(), String> {
    let conn = open(app)?;
    conn.execute("DELETE FROM music WHERE game_key = ?1", params![game_key])
        .map_err(|e| format!("cannot delete music: {e}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Playtime (Phase 5) — one row per launch session.
// ---------------------------------------------------------------------------

/// Start a play session. Returns the session row id.
pub fn start_playtime_session(
    app: &AppHandle,
    game_key: &str,
    game_name: &str,
) -> Result<i64, String> {
    let conn = open(app)?;
    conn.execute(
        "INSERT INTO playtime (game_key, game_name, started_at) VALUES (?1, ?2, ?3)",
        params![game_key, game_name, now_unix()],
    )
    .map_err(|e| format!("cannot start playtime session: {e}"))?;
    Ok(conn.last_insert_rowid())
}

/// End a play session, filling in `ended_at` and `minutes`.
/// Ending an already-ended (or unknown) session is a no-op.
pub fn end_playtime_session(app: &AppHandle, session_id: i64) -> Result<(), String> {
    let conn = open(app)?;
    let ended = now_unix();
    conn.execute(
        "UPDATE playtime SET ended_at = ?2,
         minutes = MAX(0, (?2 - started_at) / 60)
         WHERE id = ?1 AND ended_at IS NULL",
        params![session_id, ended],
    )
    .map_err(|e| format!("cannot end playtime session: {e}"))?;
    Ok(())
}

/// Total tracked minutes for one game, including a still-open session's
/// elapsed time so far.
pub fn playtime_total_minutes(app: &AppHandle, game_key: &str) -> Result<i64, String> {
    let conn = open(app)?;
    let total: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(minutes), 0) + COALESCE(SUM(
                CASE WHEN ended_at IS NULL THEN MAX(0, (?2 - started_at) / 60) ELSE 0 END), 0)
             FROM playtime WHERE game_key = ?1",
            params![game_key, now_unix()],
            |row| row.get(0),
        )
        .map_err(|e| format!("cannot read playtime: {e}"))?;
    Ok(total)
}

/// Per-game playtime totals, most-played first.
#[derive(Debug, Clone, Serialize)]
pub struct PlaytimeTotal {
    pub game_key: String,
    pub game_name: String,
    pub total_minutes: i64,
    pub sessions: i64,
}

pub fn list_playtime_totals(app: &AppHandle) -> Result<Vec<PlaytimeTotal>, String> {
    let conn = open(app)?;
    let mut stmt = conn
        .prepare(
            "SELECT game_key, game_name, COALESCE(SUM(minutes), 0), COUNT(*)
             FROM playtime GROUP BY game_key ORDER BY 3 DESC",
        )
        .map_err(|e| format!("cannot list playtime: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(PlaytimeTotal {
                game_key: row.get(0)?,
                game_name: row.get(1)?,
                total_minutes: row.get(2)?,
                sessions: row.get(3)?,
            })
        })
        .map_err(|e| format!("cannot list playtime: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("cannot read playtime row: {e}"))?);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Scanned games (Phase 4).
// ---------------------------------------------------------------------------

/// One game found by the generic folder scan.
#[derive(Debug, Clone, Serialize)]
pub struct ScannedGame {
    pub id: i64,
    pub name: String,
    pub exe_path: String,
    pub source: String,
    pub added_at: i64,
}

/// Add a scanned game. Returns `None` when the exe path is already stored
/// (deduped by `exe_path UNIQUE`).
pub fn add_scanned_game(
    app: &AppHandle,
    name: &str,
    exe_path: &str,
) -> Result<Option<i64>, String> {
    let conn = open(app)?;
    let changed = conn
        .execute(
            "INSERT OR IGNORE INTO scanned_games (name, exe_path, source, added_at)
             VALUES (?1, ?2, 'scan', ?3)",
            params![name, exe_path, now_unix()],
        )
        .map_err(|e| format!("cannot add scanned game: {e}"))?;
    if changed == 0 {
        Ok(None)
    } else {
        Ok(Some(conn.last_insert_rowid()))
    }
}

/// List all scanned games, newest first.
pub fn list_scanned_games(app: &AppHandle) -> Result<Vec<ScannedGame>, String> {
    let conn = open(app)?;
    let mut stmt = conn
        .prepare("SELECT id, name, exe_path, source, added_at FROM scanned_games ORDER BY id DESC")
        .map_err(|e| format!("cannot list scanned games: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ScannedGame {
                id: row.get(0)?,
                name: row.get(1)?,
                exe_path: row.get(2)?,
                source: row.get(3)?,
                added_at: row.get(4)?,
            })
        })
        .map_err(|e| format!("cannot list scanned games: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("cannot read scanned game row: {e}"))?);
    }
    Ok(out)
}

/// Remove one scanned game by id.
pub fn remove_scanned_game(app: &AppHandle, id: i64) -> Result<(), String> {
    let conn = open(app)?;
    conn.execute("DELETE FROM scanned_games WHERE id = ?1", params![id])
        .map_err(|e| format!("cannot remove scanned game: {e}"))?;
    Ok(())
}
