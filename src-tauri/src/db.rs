//! Manually added games, stored in SQLite (`sonic-launcher.db` in the app
//! data dir). One connection is opened per call — the table is tiny and
//! calls are rare, so no shared state is needed.

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
