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
            added_at INTEGER NOT NULL
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
    fn scanned_games_dedupe_and_list() {
        let conn = open();
        let add = |name: &str, exe: &str| {
            conn.execute(
                "INSERT OR IGNORE INTO scanned_games (name, exe_path, source, added_at)
                 VALUES (?1, ?2, 'scan', ?3)",
                params![name, exe, 1000],
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
}
