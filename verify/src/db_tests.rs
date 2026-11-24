//! SQLite integration tests for the db.rs schema and queries
//! (Phases 2-5 tables). Uses the same DDL and the same SQL statements
//! as db.rs, against an in-memory database.

use rusqlite::{params, Connection};

fn open() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
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
            added_at INTEGER NOT NULL,
            folder   TEXT NOT NULL DEFAULT '',
            exe_override TEXT,
            confidence TEXT NOT NULL DEFAULT 'low',
            pick_source TEXT NOT NULL DEFAULT 'largest',
            resolved_appid INTEGER,
            art_lookup_failed_at INTEGER,
            art_source TEXT
        );",
    )
    .unwrap();
    conn
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_overwrite() {
        let conn = open();
        // get on missing key -> no rows
        let missing: Result<String, _> = conn.query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params!["nope"],
            |r| r.get(0),
        );
        assert!(missing.is_err());
        // set + get
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params!["music_volume", "30"],
        )
        .unwrap();
        let v: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params!["music_volume"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "30");
        // overwrite
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params!["music_volume", "80"],
        )
        .unwrap();
        let v: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params!["music_volume"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "80");
        // JSON scan-folders list round-trips
        let folders = serde_json::to_string(&vec!["C:\\Games", "D:\\Steam"]).unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params!["scan_folders", folders],
        )
        .unwrap();
        let back: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params!["scan_folders"],
                |r| r.get(0),
            )
            .unwrap();
        let parsed: Vec<String> = serde_json::from_str(&back).unwrap();
        assert_eq!(parsed, vec!["C:\\Games", "D:\\Steam"]);
    }

    #[test]
    fn music_save_get_replace_and_delete() {
        let conn = open();
        conn.execute(
            "INSERT INTO music (game_key, source, path, title, downloaded_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(game_key) DO UPDATE SET source = excluded.source, path = excluded.path,
             title = excluded.title, downloaded_at = excluded.downloaded_at",
            params!["steam-440", "youtube", "C:\\music\\steam-440.m4a", "TF2 Theme", 1000],
        )
        .unwrap();
        let (source, title): (String, String) = conn
            .query_row(
                "SELECT source, title FROM music WHERE game_key = ?1",
                params!["steam-440"],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(source, "youtube");
        assert_eq!(title, "TF2 Theme");
        // replace with a manual pick
        conn.execute(
            "INSERT INTO music (game_key, source, path, title, downloaded_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(game_key) DO UPDATE SET source = excluded.source, path = excluded.path,
             title = excluded.title, downloaded_at = excluded.downloaded_at",
            params!["steam-440", "manual", "C:\\music\\steam-440.mp3", "my-song.mp3", 2000],
        )
        .unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM music WHERE game_key = ?1",
                params!["steam-440"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        conn.execute(
            "DELETE FROM music WHERE game_key = ?1",
            params!["steam-440"],
        )
        .unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM music", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn playtime_session_minutes_and_totals() {
        let conn = open();
        // Session 1: started 2h05m ago, ended now -> 125 minutes.
        let start1 = 1_000_000i64;
        let end1 = start1 + 125 * 60;
        conn.execute(
            "INSERT INTO playtime (game_key, game_name, started_at) VALUES (?1, ?2, ?3)",
            params!["steam-440", "Team Fortress 2", start1],
        )
        .unwrap();
        let id1 = conn.last_insert_rowid();
        conn.execute(
            "UPDATE playtime SET ended_at = ?2,
             minutes = MAX(0, (?2 - started_at) / 60)
             WHERE id = ?1 AND ended_at IS NULL",
            params![id1, end1],
        )
        .unwrap();
        // Ending again is a no-op (ended_at IS NULL no longer matches).
        let changed = conn
            .execute(
                "UPDATE playtime SET ended_at = ?2,
                 minutes = MAX(0, (?2 - started_at) / 60)
                 WHERE id = ?1 AND ended_at IS NULL",
                params![id1, end1 + 3600],
            )
            .unwrap();
        assert_eq!(changed, 0);
        // Session 2: still open, started 30 min ago.
        let start2 = end1 - 30 * 60;
        conn.execute(
            "INSERT INTO playtime (game_key, game_name, started_at) VALUES (?1, ?2, ?3)",
            params!["steam-440", "Team Fortress 2", start2],
        )
        .unwrap();
        // Total = finished minutes + open session elapsed (now = end1).
        let total: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(minutes), 0) + COALESCE(SUM(
                    CASE WHEN ended_at IS NULL THEN MAX(0, (?2 - started_at) / 60) ELSE 0 END), 0)
                 FROM playtime WHERE game_key = ?1",
                params!["steam-440", end1],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(total, 155);
        // Totals list groups per game, most-played first.
        conn.execute(
            "INSERT INTO playtime (game_key, game_name, started_at, ended_at, minutes) VALUES (?1, ?2, ?3, ?4, ?5)",
            params!["manual-1", "My Game", 900_000, 900_000 + 10 * 60, 10],
        )
        .unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT game_key, game_name, COALESCE(SUM(minutes), 0), COUNT(*)
                 FROM playtime GROUP BY game_key ORDER BY 3 DESC",
            )
            .unwrap();
        let rows: Vec<(String, i64, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        // steam-440: finished 125 + open session counts 0 in SUM(minutes)
        // (its minutes column is still 0) -> 125; manual-1 -> 10.
        assert_eq!(rows[0].0, "steam-440");
        assert_eq!(rows[0].1, 125);
        assert_eq!(rows[1].0, "manual-1");
        assert_eq!(rows[1].1, 10);
    }

    #[test]
    fn stale_playtime_sessions_close_with_zero_minutes() {
        let conn = open();
        // Session 1: finished normally (125 minutes) — must be untouched.
        let start1 = 1_000_000i64;
        conn.execute(
            "INSERT INTO playtime (game_key, game_name, started_at, ended_at, minutes) VALUES (?1, ?2, ?3, ?4, ?5)",
            params!["steam-440", "Team Fortress 2", start1, start1 + 125 * 60, 125],
        )
        .unwrap();
        // Session 2: still open — started 30 min ago, launcher crashed.
        let start2 = start1 + 200 * 60;
        conn.execute(
            "INSERT INTO playtime (game_key, game_name, started_at) VALUES (?1, ?2, ?3)",
            params!["steam-440", "Team Fortress 2", start2],
        )
        .unwrap();
        // The startup close: exact SQL from db::close_stale_playtime_sessions.
        let closed = conn
            .execute(
                "UPDATE playtime SET ended_at = started_at, minutes = 0 WHERE ended_at IS NULL",
                [],
            )
            .unwrap();
        assert_eq!(closed, 1);
        // The stale session credits no time: ended_at = started_at, 0 min.
        let (ended, minutes): (i64, i64) = conn
            .query_row(
                "SELECT ended_at, minutes FROM playtime WHERE started_at = ?1",
                params![start2],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(ended, start2);
        assert_eq!(minutes, 0);
        // The finished session is untouched.
        let (ended1, minutes1): (i64, i64) = conn
            .query_row(
                "SELECT ended_at, minutes FROM playtime WHERE started_at = ?1",
                params![start1],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(ended1, start1 + 125 * 60);
        assert_eq!(minutes1, 125);
        // Total is now just the finished 125 — no phantom time from the
        // stale session, even when "now" is far in the future.
        let total: i64 = conn
            .query_row(
                "SELECT COALESCE(SUM(minutes), 0) + COALESCE(SUM(
                    CASE WHEN ended_at IS NULL THEN MAX(0, (?2 - started_at) / 60) ELSE 0 END), 0)
                 FROM playtime WHERE game_key = ?1",
                params!["steam-440", start2 + 10 * 3600],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(total, 125);
        // A second close is a no-op.
        let closed2 = conn
            .execute(
                "UPDATE playtime SET ended_at = started_at, minutes = 0 WHERE ended_at IS NULL",
                [],
            )
            .unwrap();
        assert_eq!(closed2, 0);
    }

    #[test]
    fn scanned_games_dedupe_and_list() {
        let conn = open();
        let add = |name: &str, exe: &str| {
            conn.execute(
                "INSERT OR IGNORE INTO scanned_games (name, exe_path, source, added_at, folder, confidence, pick_source)
                 VALUES (?1, ?2, 'scan', ?3, ?4, ?5, ?6)",
                params![name, exe, 1000, "C:\\Games", "low", "largest"],
            )
            .unwrap()
        };
        assert_eq!(add("My Game", "C:\\Games\\MyGame.exe"), 1);
        assert_eq!(add("My Game", "C:\\Games\\MyGame.exe"), 0); // duplicate ignored
        assert_eq!(add("Other", "C:\\Games\\Other.exe"), 1);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM scanned_games", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
        let id: i64 = conn
            .query_row(
                "SELECT id FROM scanned_games WHERE exe_path = ?1",
                params!["C:\\Games\\Other.exe"],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute("DELETE FROM scanned_games WHERE id = ?1", params![id])
            .unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM scanned_games", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn scanned_games_folder_upsert_and_override_survive_rescan() {
        // Mirrors the SQL in db.rs add_scanned_game / set_scanned_exe_override.
        let conn = open();
        let folder = "C:\\Games\\Cyberpunk 2077";
        // First scan: known-game pick.
        conn.execute(
            "INSERT OR IGNORE INTO scanned_games (name, exe_path, source, added_at, folder, confidence, pick_source)
             VALUES (?1, ?2, 'scan', ?3, ?4, ?5, ?6)",
            params![
                "Cyberpunk 2077",
                "C:\\Games\\Cyberpunk 2077\\bin\\x64\\Cyberpunk2077.exe",
                1000,
                folder,
                "high",
                "known game"
            ],
        )
        .unwrap();
        let id: i64 = conn.last_insert_rowid();
        // Rescan picks a different exe for the same folder: the row is
        // updated in place (same id, no duplicate).
        let existing: Option<(i64, Option<String>)> = conn
            .query_row(
                "SELECT id, exe_override FROM scanned_games WHERE folder = ?1",
                params![folder],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        assert_eq!(existing, Some((id, None)));
        conn.execute(
            "UPDATE scanned_games SET name = ?1, exe_path = ?2, confidence = ?3, pick_source = ?4 WHERE id = ?5",
            params![
                "Cyberpunk 2077",
                "C:\\Games\\Cyberpunk 2077\\REDprelauncher.exe",
                "low",
                "largest",
                id
            ],
        )
        .unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM scanned_games", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        // The user overrides the exe: a later rescan must not clobber it.
        conn.execute(
            "UPDATE scanned_games SET exe_override = ?1 WHERE id = ?2",
            params!["C:\\Games\\Cyberpunk 2077\\bin\\x64\\Cyberpunk2077.exe", id],
        )
        .unwrap();
        let existing: Option<(i64, Option<String>)> = conn
            .query_row(
                "SELECT id, exe_override FROM scanned_games WHERE folder = ?1",
                params![folder],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        assert!(existing.unwrap().1.is_some());
        // Effective exe = override, not the stored pick.
        let effective: String = conn
            .query_row(
                "SELECT COALESCE(exe_override, exe_path) FROM scanned_games WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            effective,
            "C:\\Games\\Cyberpunk 2077\\bin\\x64\\Cyberpunk2077.exe"
        );
        // Legacy row adoption: a row with no folder is adopted by exe path.
        conn.execute(
            "INSERT INTO scanned_games (name, exe_path, source, added_at)
             VALUES (?1, ?2, 'scan', ?3)",
            params!["Legacy Game", "C:\\Games\\Legacy\\Legacy.exe", 1000],
        )
        .unwrap();
        let legacy_id = conn.last_insert_rowid();
        let legacy: Option<i64> = conn
            .query_row(
                "SELECT id FROM scanned_games WHERE exe_path = ?1 AND folder = ''",
                params!["C:\\Games\\Legacy\\Legacy.exe"],
                |r| r.get(0),
            )
            .ok();
        assert_eq!(legacy, Some(legacy_id));
        conn.execute(
            "UPDATE scanned_games SET name = ?1, folder = ?2, confidence = ?3, pick_source = ?4 WHERE id = ?5",
            params![
                "Legacy Game",
                "C:\\Games\\Legacy",
                "medium",
                "name",
                legacy_id
            ],
        )
        .unwrap();
        let folder_back: String = conn
            .query_row(
                "SELECT folder FROM scanned_games WHERE id = ?1",
                params![legacy_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(folder_back, "C:\\Games\\Legacy");
    }

    #[test]
    fn scanned_games_art_columns_round_trip_and_survive_rescan() {
        // Mirrors the SQL in db.rs set_art_resolved / set_art_source /
        // mark_art_lookup_failed / clear_art_lookup_failed.
        let conn = open();
        conn.execute(
            "INSERT INTO scanned_games (name, exe_path, source, added_at, folder, confidence, pick_source)
             VALUES (?1, ?2, 'scan', ?3, ?4, ?5, ?6)",
            params![
                "Cyberpunk 2077",
                "C:\\Games\\Cyberpunk 2077\\bin\\x64\\Cyberpunk2077.exe",
                1000,
                "C:\\Games\\Cyberpunk 2077",
                "high",
                "known game"
            ],
        )
        .unwrap();
        let id: i64 = conn.last_insert_rowid();
        // Fresh row: all art columns NULL.
        let fresh: (Option<i64>, Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT resolved_appid, art_lookup_failed_at, art_source FROM scanned_games WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(fresh, (None, None, None));
        // Steam store-search hit: cache the appid + source.
        conn.execute(
            "UPDATE scanned_games SET resolved_appid = ?1, art_source = ?2, art_lookup_failed_at = NULL WHERE id = ?3",
            params![1091500i64, "steam", id],
        )
        .unwrap();
        let hit: (Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT resolved_appid, art_source FROM scanned_games WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(hit, (Some(1091500), Some("steam".to_string())));
        // A rescan (the UPDATE add_scanned_game runs) must not clobber
        // the cached appid or the source marker.
        conn.execute(
            "UPDATE scanned_games SET name = ?1, exe_path = ?2, confidence = ?3, pick_source = ?4 WHERE id = ?5",
            params![
                "Cyberpunk 2077",
                "C:\\Games\\Cyberpunk 2077\\bin\\x64\\Cyberpunk2077.exe",
                "high",
                "known game",
                id
            ],
        )
        .unwrap();
        let kept: (Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT resolved_appid, art_source FROM scanned_games WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(kept, (Some(1091500), Some("steam".to_string())));
        // GOG hit: source marker only, no Steam appid.
        conn.execute(
            "UPDATE scanned_games SET art_source = ?1 WHERE id = ?2",
            params!["gog", id],
        )
        .unwrap();
        // Double miss: record the failure time; a retry clears it.
        conn.execute(
            "UPDATE scanned_games SET art_lookup_failed_at = ?1 WHERE id = ?2",
            params![1_800_000_000i64, id],
        )
        .unwrap();
        let failed: Option<i64> = conn
            .query_row(
                "SELECT art_lookup_failed_at FROM scanned_games WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(failed, Some(1_800_000_000));
        conn.execute(
            "UPDATE scanned_games SET art_lookup_failed_at = NULL WHERE id = ?1",
            params![id],
        )
        .unwrap();
        let cleared: Option<i64> = conn
            .query_row(
                "SELECT art_lookup_failed_at FROM scanned_games WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cleared, None);
        // Manual cover: the marker survives a rescan too.
        conn.execute(
            "UPDATE scanned_games SET art_source = ?1 WHERE id = ?2",
            params!["manual", id],
        )
        .unwrap();
        conn.execute(
            "UPDATE scanned_games SET name = ?1, exe_path = ?2, confidence = ?3, pick_source = ?4 WHERE id = ?5",
            params![
                "Cyberpunk 2077",
                "C:\\Games\\Cyberpunk 2077\\bin\\x64\\Cyberpunk2077.exe",
                "high",
                "known game",
                id
            ],
        )
        .unwrap();
        let manual: Option<String> = conn
            .query_row(
                "SELECT art_source FROM scanned_games WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(manual, Some("manual".to_string()));
    }
}
