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
        );
        CREATE TABLE IF NOT EXISTS ignored_paths (
            path       TEXT PRIMARY KEY,
            removed_at INTEGER NOT NULL
        );",
    )
    .map_err(|e| format!("cannot create table: {e}"))?;
    // Migrations for `scanned_games` (exe-guarantee round, v0.2.3):
    // the table predates the folder/override/confidence columns, and
    // CREATE TABLE IF NOT EXISTS never adds them. `ensure_column` adds
    // each one only when it is missing, so old databases upgrade in
    // place with their games intact. The art-lookup columns (v0.2.5)
    // follow the same pattern.
    ensure_column(&conn, "scanned_games", "folder", "TEXT NOT NULL DEFAULT ''")?;
    ensure_column(&conn, "scanned_games", "exe_override", "TEXT")?;
    ensure_column(
        &conn,
        "scanned_games",
        "confidence",
        "TEXT NOT NULL DEFAULT 'low'",
    )?;
    ensure_column(
        &conn,
        "scanned_games",
        "pick_source",
        "TEXT NOT NULL DEFAULT 'largest'",
    )?;
    ensure_column(&conn, "scanned_games", "resolved_appid", "INTEGER")?;
    ensure_column(&conn, "scanned_games", "art_lookup_failed_at", "INTEGER")?;
    ensure_column(&conn, "scanned_games", "art_source", "TEXT")?;
    Ok(conn)
}

/// Add `column` to `table` when it is missing (see `open`).
fn ensure_column(conn: &Connection, table: &str, column: &str, ddl: &str) -> Result<(), String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| format!("cannot inspect table {table}: {e}"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| format!("cannot inspect table {table}: {e}"))?;
    for name in rows.flatten() {
        if name == column {
            return Ok(());
        }
    }
    conn.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {ddl}"),
        [],
    )
    .map_err(|e| format!("cannot add column {column} to {table}: {e}"))?;
    Ok(())
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
    /// The game folder the exe was picked from.
    pub folder: String,
    /// The exe the user chose instead of the auto-pick, if any. This
    /// always wins: rescans never touch a row that has one.
    pub exe_override: Option<String>,
    /// "high" / "medium" / "low" — how much the auto-pick is trusted.
    pub confidence: String,
    /// Which signal picked the exe (shortcut, known game, registry, ...).
    pub pick_source: String,
    /// Steam appid found by the keyless store search (v0.2.5): the
    /// game has no Steam appid of its own, so this is the cached
    /// lookup result. `None` = never resolved (or resolved via GOG).
    pub resolved_appid: Option<i64>,
    /// Unix time of the last failed keyless art lookup (Steam and
    /// GOG both missed). No auto-retry for 14 days unless the user
    /// clicks Retry Art.
    pub art_lookup_failed_at: Option<i64>,
    /// Where the game's art came from: "steam" (store search hit),
    /// "gog" (GOG Catalog hit), "manual" (user-picked cover, AppID,
    /// or image URL). `None` = not resolved yet. A manual cover is
    /// never overwritten by an automatic lookup.
    pub art_source: Option<String>,
}

impl ScannedGame {
    /// The exe that actually launches: the user's override when set,
    /// otherwise the auto-picked exe.
    pub fn effective_exe(&self) -> String {
        self.exe_override
            .clone()
            .unwrap_or_else(|| self.exe_path.clone())
    }
}

/// Add a scanned game, keyed by FOLDER (not by exe path): a rescan that
/// picks a different exe for the same folder updates the row in place
/// instead of adding a duplicate. A row with a user override is never
/// touched — the override always wins. Returns the row id, or `None`
/// when nothing was added/changed.
pub fn add_scanned_game(
    app: &AppHandle,
    name: &str,
    exe_path: &str,
    folder: &str,
    confidence: &str,
    pick_source: &str,
) -> Result<Option<i64>, String> {
    // Never re-add a path the user explicitly removed.
    let ignored = list_ignored_paths(app).unwrap_or_default();
    if ignored.contains(&exe_path.trim().to_lowercase()) {
        return Ok(None);
    }
    let conn = open(app)?;
    // Legacy rows (pre-v0.2.3) have no folder: match them by exe path
    // and adopt them into the new scheme.
    let legacy: Option<i64> = conn
        .query_row(
            "SELECT id FROM scanned_games WHERE exe_path = ?1 AND folder = ''",
            params![exe_path],
            |row| row.get(0),
        )
        .ok();
    if let Some(id) = legacy {
        conn.execute(
            "UPDATE scanned_games SET name = ?1, folder = ?2, confidence = ?3, pick_source = ?4 WHERE id = ?5",
            params![name, folder, confidence, pick_source, id],
        )
        .map_err(|e| format!("cannot update scanned game: {e}"))?;
        return Ok(Some(id));
    }
    // A row for this folder already exists?
    let existing: Option<(i64, Option<String>)> = conn
        .query_row(
            "SELECT id, exe_override FROM scanned_games WHERE folder = ?1",
            params![folder],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();
    if let Some((id, override_path)) = existing {
        // The user overrode this game's exe: the rescan must not
        // clobber it (and must not add a second row for the folder).
        if override_path.is_some() {
            return Ok(None);
        }
        // No override: update the auto-pick in place (keeps the row id,
        // so music/playtime keyed by `scan-<id>` survive a rescan).
        // If another row already owns the new exe path, drop that row
        // first — one exe belongs to one folder.
        let _ = conn.execute(
            "DELETE FROM scanned_games WHERE exe_path = ?1 AND id != ?2",
            params![exe_path, id],
        );
        conn.execute(
            "UPDATE scanned_games SET name = ?1, exe_path = ?2, confidence = ?3, pick_source = ?4 WHERE id = ?5",
            params![name, exe_path, confidence, pick_source, id],
        )
        .map_err(|e| format!("cannot update scanned game: {e}"))?;
        return Ok(Some(id));
    }
    let changed = conn
        .execute(
            "INSERT OR IGNORE INTO scanned_games (name, exe_path, source, added_at, folder, confidence, pick_source)
             VALUES (?1, ?2, 'scan', ?3, ?4, ?5, ?6)",
            params![name, exe_path, now_unix(), folder, confidence, pick_source],
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
        .prepare("SELECT id, name, exe_path, source, added_at, folder, exe_override, confidence, pick_source, resolved_appid, art_lookup_failed_at, art_source FROM scanned_games ORDER BY id DESC")
        .map_err(|e| format!("cannot list scanned games: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ScannedGame {
                id: row.get(0)?,
                name: row.get(1)?,
                exe_path: row.get(2)?,
                source: row.get(3)?,
                added_at: row.get(4)?,
                folder: row.get(5)?,
                exe_override: row.get(6)?,
                confidence: row.get(7)?,
                pick_source: row.get(8)?,
                resolved_appid: row.get(9)?,
                art_lookup_failed_at: row.get(10)?,
                art_source: row.get(11)?,
            })
        })
        .map_err(|e| format!("cannot list scanned games: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("cannot read scanned game row: {e}"))?);
    }
    Ok(out)
}

/// One scanned game by id, or `None`.
pub fn get_scanned_game(app: &AppHandle, id: i64) -> Result<Option<ScannedGame>, String> {
    let conn = open(app)?;
    let row = conn
        .query_row(
            "SELECT id, name, exe_path, source, added_at, folder, exe_override, confidence, pick_source, resolved_appid, art_lookup_failed_at, art_source FROM scanned_games WHERE id = ?1",
            params![id],
            |row| {
                Ok(ScannedGame {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    exe_path: row.get(2)?,
                    source: row.get(3)?,
                    added_at: row.get(4)?,
                    folder: row.get(5)?,
                    exe_override: row.get(6)?,
                    confidence: row.get(7)?,
                    pick_source: row.get(8)?,
                    resolved_appid: row.get(9)?,
                    art_lookup_failed_at: row.get(10)?,
                    art_source: row.get(11)?,
                })
            },
        )
        .ok();
    Ok(row)
}

/// Set the user's exe override for a scanned game. The exe must exist,
/// end in `.exe`, and live inside the game's folder. The override is
/// never clobbered by rescans (see `add_scanned_game`).
pub fn set_scanned_exe_override(app: &AppHandle, id: i64, exe_path: &str) -> Result<(), String> {
    let game = get_scanned_game(app, id)?.ok_or_else(|| format!("no scanned game with id {id}"))?;
    let exe_path = exe_path.trim();
    if !exe_path.to_lowercase().ends_with(".exe") {
        return Err("not an .exe file".to_string());
    }
    if !std::path::Path::new(exe_path).is_file() {
        return Err("exe file does not exist".to_string());
    }
    if !game.folder.is_empty() {
        let folder_norm = game.folder.replace('\\', "/").to_lowercase();
        let exe_norm = exe_path.replace('\\', "/").to_lowercase();
        let folder_norm = folder_norm.trim_end_matches('/');
        if !exe_norm.starts_with(&format!("{folder_norm}/")) {
            return Err("exe is not inside the game folder".to_string());
        }
    }
    let conn = open(app)?;
    let changed = conn
        .execute(
            "UPDATE scanned_games SET exe_override = ?1 WHERE id = ?2",
            params![exe_path, id],
        )
        .map_err(|e| format!("cannot set exe override: {e}"))?;
    if changed == 0 {
        return Err(format!("no scanned game with id {id}"));
    }
    Ok(())
}

/// Clear the user's exe override (the auto-pick takes over again).
pub fn clear_scanned_exe_override(app: &AppHandle, id: i64) -> Result<(), String> {
    let conn = open(app)?;
    let changed = conn
        .execute(
            "UPDATE scanned_games SET exe_override = NULL WHERE id = ?1",
            params![id],
        )
        .map_err(|e| format!("cannot clear exe override: {e}"))?;
    if changed == 0 {
        return Err(format!("no scanned game with id {id}"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Keyless art lookup state (v0.2.5) — cached on `scanned_games`.
// Rescans never touch these columns: `add_scanned_game` updates only
// name / exe_path / confidence / pick_source, so a resolved appid,
// a manual marker, and a failure timestamp all survive every rescan.
// ---------------------------------------------------------------------------

/// The scan id for a `scan-<id>` game key, or `None` for other keys
/// (steam-*, manual-*, epic-*, ...).
pub fn scanned_id_from_key(game_key: &str) -> Option<i64> {
    game_key.strip_prefix("scan-")?.parse().ok()
}

/// Record a successful keyless Steam lookup: cache the appid and the
/// source ("steam", or "manual" when the user typed the AppID
/// themselves), and clear any previous failure timestamp.
pub fn set_art_resolved(app: &AppHandle, id: i64, appid: u32, source: &str) -> Result<(), String> {
    let conn = open(app)?;
    let changed = conn
        .execute(
            "UPDATE scanned_games SET resolved_appid = ?1, art_source = ?2, art_lookup_failed_at = NULL WHERE id = ?3",
            params![appid as i64, source, id],
        )
        .map_err(|e| format!("cannot save resolved appid: {e}"))?;
    if changed == 0 {
        return Err(format!("no scanned game with id {id}"));
    }
    Ok(())
}

/// Record where a game's art came from without a Steam appid: "gog"
/// for a GOG Catalog hit, "manual" for a user-picked cover or image
/// URL. A manual marker is never overwritten by an automatic lookup
/// (see art.rs).
pub fn set_art_source(app: &AppHandle, id: i64, source: &str) -> Result<(), String> {
    let conn = open(app)?;
    let changed = conn
        .execute(
            "UPDATE scanned_games SET art_source = ?1 WHERE id = ?2",
            params![source, id],
        )
        .map_err(|e| format!("cannot save art source: {e}"))?;
    if changed == 0 {
        return Err(format!("no scanned game with id {id}"));
    }
    Ok(())
}

/// Record a failed keyless art lookup (Steam and GOG both missed).
/// Automatic retries stay blocked for 14 days (see art.rs); the
/// user can always force a retry from the ••• menu.
pub fn mark_art_lookup_failed(app: &AppHandle, id: i64) -> Result<(), String> {
    let conn = open(app)?;
    let changed = conn
        .execute(
            "UPDATE scanned_games SET art_lookup_failed_at = ?1 WHERE id = ?2",
            params![now_unix(), id],
        )
        .map_err(|e| format!("cannot mark art lookup failed: {e}"))?;
    if changed == 0 {
        return Err(format!("no scanned game with id {id}"));
    }
    Ok(())
}

/// Clear the failure timestamp (Retry Art, or a later success).
pub fn clear_art_lookup_failed(app: &AppHandle, id: i64) -> Result<(), String> {
    let conn = open(app)?;
    let changed = conn
        .execute(
            "UPDATE scanned_games SET art_lookup_failed_at = NULL WHERE id = ?1",
            params![id],
        )
        .map_err(|e| format!("cannot clear art lookup failure: {e}"))?;
    if changed == 0 {
        return Err(format!("no scanned game with id {id}"));
    }
    Ok(())
}

/// Update the auto-picked exe for a scanned game (used when the
/// override is cleared and the folder is re-ranked).
pub fn update_scanned_pick(
    app: &AppHandle,
    id: i64,
    exe_path: &str,
    confidence: &str,
    pick_source: &str,
) -> Result<(), String> {
    let conn = open(app)?;
    // One exe belongs to one folder: drop any other row owning this path.
    let _ = conn.execute(
        "DELETE FROM scanned_games WHERE exe_path = ?1 AND id != ?2",
        params![exe_path, id],
    );
    let changed = conn
        .execute(
            "UPDATE scanned_games SET exe_path = ?1, confidence = ?2, pick_source = ?3 WHERE id = ?4",
            params![exe_path, confidence, pick_source, id],
        )
        .map_err(|e| format!("cannot update scanned pick: {e}"))?;
    if changed == 0 {
        return Err(format!("no scanned game with id {id}"));
    }
    Ok(())
}

/// Remove one scanned game by id AND remember its exe path in
/// `ignored_paths`, so the next scan never re-adds a game the user
/// explicitly removed.
pub fn remove_scanned_game(app: &AppHandle, id: i64) -> Result<(), String> {
    let conn = open(app)?;
    // Remember the EFFECTIVE exe (override when set) so the next scan
    // never re-adds the game the user just removed.
    let exe_path: Option<String> = conn
        .query_row(
            "SELECT COALESCE(exe_override, exe_path) FROM scanned_games WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .ok();
    conn.execute("DELETE FROM scanned_games WHERE id = ?1", params![id])
        .map_err(|e| format!("cannot remove scanned game: {e}"))?;
    if let Some(path) = exe_path {
        let _ = ignore_scanned_path(app, &path);
    }
    Ok(())
}

/// Remember an exe path as ignored (never re-add it on a rescan).
pub fn ignore_scanned_path(app: &AppHandle, exe_path: &str) -> Result<(), String> {
    let conn = open(app)?;
    conn.execute(
        "INSERT OR REPLACE INTO ignored_paths (path, removed_at) VALUES (?1, ?2)",
        params![exe_path.trim().to_lowercase(), now_unix()],
    )
    .map_err(|e| format!("cannot ignore path: {e}"))?;
    Ok(())
}

/// All ignored exe paths (lowercased), for the scan filter.
pub fn list_ignored_paths(app: &AppHandle) -> Result<Vec<String>, String> {
    let conn = open(app)?;
    let mut stmt = conn
        .prepare("SELECT path FROM ignored_paths")
        .map_err(|e| format!("cannot list ignored paths: {e}"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| format!("cannot list ignored paths: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("cannot read ignored path: {e}"))?);
    }
    Ok(out)
}
