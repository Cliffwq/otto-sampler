//! Best-effort disk reclamation, called only by the serialized index worker.
//!
//! No media or index records are deleted here. New databases use incremental
//! auto-vacuum, whose work is capped per pass. Converting an older database needs
//! one full VACUUM; only modest databases with substantial free space qualify.

use rusqlite::{Connection, Error, ErrorCode, Result};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MIB: u64 = 1024 * 1024;

#[derive(Clone, Copy)]
struct Policy {
    minimum_free_bytes: u64,
    minimum_free_percent: u64,
    maximum_legacy_bytes: u64,
    maximum_incremental_bytes: u64,
    retry_seconds: i64,
    success_interval_seconds: i64,
}

const POLICY: Policy = Policy {
    minimum_free_bytes: 16 * MIB,
    minimum_free_percent: 25,
    maximum_legacy_bytes: 256 * MIB,
    maximum_incremental_bytes: 64 * MIB,
    retry_seconds: 5 * 60,
    success_interval_seconds: 60 * 60,
};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReclaimOutcome {
    NotNeeded,
    InTransaction,
    CoolingDown,
    Busy,
    DeferredLargeLegacyDatabase,
    Reclaimed { before_bytes: u64, after_bytes: u64 },
}

/// Call immediately after opening a database, before creating its first table.
/// Existing databases are left alone: changing their mode requires a VACUUM.
pub(crate) fn configure_new_database(conn: &Connection) -> Result<()> {
    let pages: u64 = conn.pragma_query_value(None, "page_count", |row| {
        row.get::<_, u32>(0).map(u64::from)
    })?;
    if pages == 0 {
        conn.pragma_update(None, "auto_vacuum", 2)?;
    }
    Ok(())
}

/// Run after a successful scan/reset, after all of that operation's statements
/// have been dropped. The caller must hold the index-write guard and run on a
/// blocking worker. Failure must not turn a successful scan into a failed scan.
pub(crate) fn reclaim_unused_space(conn: &Connection) -> Result<ReclaimOutcome> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64;
    reclaim_at(conn, now, POLICY)
}

fn reclaim_at(conn: &Connection, now: i64, policy: Policy) -> Result<ReclaimOutcome> {
    if !conn.is_autocommit() {
        return Ok(ReclaimOutcome::InTransaction);
    }
    // Maintenance never waits for a competing reader/writer. Restore the
    // connection's normal busy timeout even when a maintenance statement fails.
    let old_timeout: u64 = conn.pragma_query_value(None, "busy_timeout", |row| {
        row.get::<_, u32>(0).map(u64::from)
    })?;
    conn.busy_timeout(Duration::ZERO)?;
    let result = reclaim_unlocked(conn, now, policy);
    let restore = conn.busy_timeout(Duration::from_millis(old_timeout));
    match result {
        Err(Error::SqliteFailure(code, _))
            if matches!(
                code.code,
                ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked
            ) =>
        {
            restore?;
            Ok(ReclaimOutcome::Busy)
        }
        result => {
            restore?;
            result
        }
    }
}

fn enough_free_space(pages: u64, free_pages: u64, page_size: u64, policy: Policy) -> bool {
    pages > 0
        && free_pages.saturating_mul(page_size) >= policy.minimum_free_bytes
        && free_pages.saturating_mul(100) >= pages.saturating_mul(policy.minimum_free_percent)
}

fn reclaim_unlocked(conn: &Connection, now: i64, policy: Policy) -> Result<ReclaimOutcome> {
    let pages: u64 = conn.pragma_query_value(None, "page_count", |row| {
        row.get::<_, u32>(0).map(u64::from)
    })?;
    let free_pages: u64 = conn.pragma_query_value(None, "freelist_count", |row| {
        row.get::<_, u32>(0).map(u64::from)
    })?;
    let page_size: u64 =
        conn.pragma_query_value(None, "page_size", |row| row.get::<_, u32>(0).map(u64::from))?;
    if !enough_free_space(pages, free_pages, page_size, policy) {
        return Ok(ReclaimOutcome::NotNeeded);
    }
    let before_bytes = pages.saturating_mul(page_size);
    let auto_vacuum: u32 = conn.pragma_query_value(None, "auto_vacuum", |row| row.get(0))?;
    if auto_vacuum == 0 && before_bytes > policy.maximum_legacy_bytes {
        // Avoid an unbounded automatic full rebuild of a large legacy library.
        return Ok(ReclaimOutcome::DeferredLargeLegacyDatabase);
    }

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS database_maintenance (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            last_attempt INTEGER NOT NULL DEFAULT 0,
            last_success INTEGER NOT NULL DEFAULT 0
        );
        INSERT OR IGNORE INTO database_maintenance (id) VALUES (1);",
    )?;
    let (last_attempt, last_success): (i64, i64) = conn.query_row(
        "SELECT last_attempt, last_success FROM database_maintenance WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if (last_attempt != 0 && now.saturating_sub(last_attempt) < policy.retry_seconds)
        || (last_success != 0 && now.saturating_sub(last_success) < policy.success_interval_seconds)
    {
        return Ok(ReclaimOutcome::CoolingDown);
    }
    conn.execute(
        "UPDATE database_maintenance SET last_attempt = ?1 WHERE id = 1",
        [now],
    )?;

    if auto_vacuum == 0 {
        // SQLite's VACUUM is transactional, including disk-full/lock failures.
        // Check before rebuilding and never replace/delete the live DB file.
        let check: String = conn.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
        if check != "ok" {
            return Err(Error::InvalidQuery);
        }
        conn.pragma_update(None, "auto_vacuum", 2)?;
        conn.execute_batch("VACUUM")?;
    } else if auto_vacuum == 2 {
        let limit = (policy.maximum_incremental_bytes / page_size).max(1);
        // execute_batch steps every result row produced by incremental_vacuum.
        conn.execute_batch(&format!("PRAGMA incremental_vacuum({limit})"))?;
    }

    conn.execute(
        "UPDATE database_maintenance SET last_success = ?1 WHERE id = 1",
        [now],
    )?;
    let after_pages: u64 = conn.pragma_query_value(None, "page_count", |row| {
        row.get::<_, u32>(0).map(u64::from)
    })?;
    Ok(ReclaimOutcome::Reclaimed {
        before_bytes,
        after_bytes: after_pages.saturating_mul(page_size),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const TEST_POLICY: Policy = Policy {
        minimum_free_bytes: 32 * 1024,
        maximum_legacy_bytes: 8 * MIB,
        maximum_incremental_bytes: 64 * 1024,
        ..POLICY
    };

    struct DatabaseFile(PathBuf);

    impl DatabaseFile {
        fn new() -> Self {
            static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self(std::env::temp_dir().join(format!(
                "otto-db-maintenance-{}-{stamp}-{}.db",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            )))
        }
    }

    impl Drop for DatabaseFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn populate(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE files (id INTEGER PRIMARY KEY, payload BLOB);
             CREATE TABLE favorites (name TEXT);
             INSERT INTO favorites VALUES ('keep my library');
             WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<300)
             INSERT INTO files SELECT i, zeroblob(2048) FROM n;
             DELETE FROM files WHERE id > 20;",
        )
        .unwrap();
    }

    fn assert_records_intact(conn: &Connection) {
        let (count, length): (i64, i64) = conn
            .query_row(
                "SELECT count(*), sum(length(payload)) FROM files",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((count, length), (20, 20 * 2048));
        assert_eq!(
            conn.query_row("SELECT name FROM favorites", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "keep my library"
        );
        assert_eq!(
            conn.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
    }

    #[test]
    fn thresholds_require_both_significant_bytes_and_significant_fraction() {
        assert!(!enough_free_space(10000, 4095, 4096, POLICY));
        assert!(!enough_free_space(20000, 4096, 4096, POLICY));
        assert!(enough_free_space(16384, 4096, 4096, POLICY));
        assert!(!enough_free_space(0, 0, 4096, POLICY));
    }

    #[test]
    fn legacy_compaction_preserves_records_and_enables_incremental_mode() {
        let file = DatabaseFile::new();
        let conn = Connection::open(&file.0).unwrap();
        populate(&conn);
        let result = reclaim_at(&conn, 100000, TEST_POLICY).unwrap();
        let ReclaimOutcome::Reclaimed {
            before_bytes,
            after_bytes,
        } = result
        else {
            panic!("{result:?}")
        };
        assert!(after_bytes < before_bytes / 2);
        assert_eq!(
            conn.pragma_query_value(None, "auto_vacuum", |row| row.get::<_, u32>(0))
                .unwrap(),
            2
        );
        assert_records_intact(&conn);
        assert_eq!(std::fs::metadata(&file.0).unwrap().len(), after_bytes);
    }

    #[test]
    fn incremental_work_is_bounded_and_cooldown_survives_reopening() {
        let file = DatabaseFile::new();
        {
            let conn = Connection::open(&file.0).unwrap();
            configure_new_database(&conn).unwrap();
            populate(&conn);
            let result = reclaim_at(&conn, 100000, TEST_POLICY).unwrap();
            let ReclaimOutcome::Reclaimed {
                before_bytes,
                after_bytes,
            } = result
            else {
                panic!("{result:?}")
            };
            assert!(after_bytes < before_bytes);
            assert!(before_bytes - after_bytes <= TEST_POLICY.maximum_incremental_bytes);
            assert_records_intact(&conn);
        }
        let conn = Connection::open(&file.0).unwrap();
        assert_eq!(
            reclaim_at(&conn, 100001, TEST_POLICY).unwrap(),
            ReclaimOutcome::CoolingDown
        );
        assert!(matches!(
            reclaim_at(&conn, 200000, TEST_POLICY).unwrap(),
            ReclaimOutcome::Reclaimed { .. }
        ));
        assert_records_intact(&conn);
    }

    #[test]
    fn busy_database_is_skipped_and_timeout_is_restored() {
        let file = DatabaseFile::new();
        let conn = Connection::open(&file.0).unwrap();
        populate(&conn);
        conn.busy_timeout(Duration::from_millis(127)).unwrap();
        let reader = Connection::open(&file.0).unwrap();
        reader
            .execute_batch("BEGIN; SELECT * FROM favorites;")
            .unwrap();
        assert_eq!(
            reclaim_at(&conn, 100000, TEST_POLICY).unwrap(),
            ReclaimOutcome::Busy
        );
        assert_eq!(
            conn.pragma_query_value(None, "busy_timeout", |row| row.get::<_, u32>(0))
                .unwrap(),
            127
        );
        reader.execute_batch("ROLLBACK").unwrap();
        assert_records_intact(&conn);
    }

    #[test]
    fn active_transaction_and_oversized_legacy_rebuild_are_skipped() {
        let file = DatabaseFile::new();
        let conn = Connection::open(&file.0).unwrap();
        populate(&conn);
        conn.execute_batch("BEGIN").unwrap();
        assert_eq!(
            reclaim_at(&conn, 100000, TEST_POLICY).unwrap(),
            ReclaimOutcome::InTransaction
        );
        conn.execute_batch("ROLLBACK").unwrap();
        let policy = Policy {
            maximum_legacy_bytes: 1,
            ..TEST_POLICY
        };
        assert_eq!(
            reclaim_at(&conn, 100000, policy).unwrap(),
            ReclaimOutcome::DeferredLargeLegacyDatabase
        );
        configure_new_database(&conn).unwrap();
        assert_eq!(
            conn.pragma_query_value(None, "auto_vacuum", |row| row.get::<_, u32>(0))
                .unwrap(),
            0
        );
        assert_records_intact(&conn);
    }
}
