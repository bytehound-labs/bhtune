//! Full-database backup and restore: a single portable SQLite file out, and back in.
//!
//! Independent of `history-retention`'s age-based deletion of old runs — this is about
//! moving or protecting an *entire* installation (support diagnostics, migrating to a new
//! machine, or a safety net immediately before a risky operation), not about pruning
//! individual runs. Exposed to both the CLI and the GUI.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use chrono::{DateTime, Utc};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};

use crate::{
    error::{DbError, DbResult},
    pool::connect,
};

/// How long the pre-restore exclusivity probe waits on a lock before concluding another
/// connection is genuinely still using the database, rather than just finishing up.
///
/// Deliberately much shorter than [`crate::pool::connect`]'s own busy timeout (10 seconds):
/// that timeout exists so ordinary contended *work* (a retention sweep, a large export) has
/// room to finish rather than erroring out. This one exists only to answer "is anyone here
/// right now", so it should fail fast — a restore command that appears to hang for ten
/// seconds every time a read happened moments earlier would be a poor, confusing experience
/// for what is meant to be a quick, decisive check. A short grace window still lets a
/// genuinely fleeting reader (one that's already committing) avoid a false positive.
const EXCLUSIVITY_PROBE_TIMEOUT: Duration = Duration::from_millis(200);

/// Writes a complete, consistent, compacted snapshot of `pool`'s database to `dest`.
///
/// Uses SQLite's own `VACUUM INTO`: an online operation (it doesn't block `pool`'s other
/// readers/writers) that produces a plain, single-file, non-WAL database — the most portable
/// on-disk form, with no `-wal`/`-shm` sidecar files that would also need copying.
///
/// `dest` must not already exist. `VACUUM INTO` refuses to overwrite a file, and that's the
/// right behavior for a backup command: silently clobbering a previous backup because of a
/// reused filename would be its own data-loss bug, not a convenience worth having.
pub async fn backup_to(pool: &SqlitePool, dest: &Path) -> DbResult<()> {
    if dest.exists() {
        return Err(DbError::BackupDestinationExists(dest.to_path_buf()));
    }

    // `VACUUM INTO`'s target is a string expression, not a path — SQLite has no notion of
    // `std::path::Path`. A lossy conversion (rather than requiring valid UTF-8) is fine here:
    // it only affects the extremely rare non-UTF-8 filename, and failing loudly for that
    // vanishingly unlikely case isn't worth the extra fallible API surface.
    sqlx::query("VACUUM INTO ?")
        .bind(dest.display().to_string())
        .execute(pool)
        .await
        .map_err(DbError::Query)?;

    validate_backup_file(dest).await
}

/// The result of a successful [`restore_from`]: the fresh pool to use going forward, and
/// where the previously-live database was safety-copied to before being overwritten.
#[derive(Debug)]
pub struct RestoreOutcome {
    pub pool: SqlitePool,
    /// `None` only when `db_path` didn't exist yet (a fresh install being restored into for
    /// the first time) — there was nothing to safety-copy.
    pub pre_restore_backup: Option<PathBuf>,
}

/// Restores `db_path` from a [`backup_to`]-produced file, replacing its entire contents.
///
/// Takes `pool` **by value**, not `&SqlitePool`: restoring means the file underneath every
/// existing connection is about to be replaced out from under them, so the caller's old pool
/// must be given up, not merely borrowed. The type system then enforces that the old handle
/// can't accidentally go on being used afterward — every caller must switch to
/// [`RestoreOutcome::pool`].
///
/// Safety, in order:
/// 1. `backup_path` is validated (`PRAGMA integrity_check`, plus confirming a real
///    `tune_runs` table exists) *before* anything about the live database is touched, so a
///    corrupt or unrelated file never gets a chance to destroy good data.
/// 2. The caller's own connections to `db_path` are closed first, so step 3's exclusivity
///    check isn't confused by this process's own still-open pool.
/// 3. If `db_path` already exists, [`exclusive_pre_restore_snapshot`] both confirms no other
///    connection — in this process or another — still holds it open, and, while that's
///    proven true, takes a consistent `VACUUM INTO` copy of it (see
///    [`RestoreOutcome::pre_restore_backup`]). Restoring the wrong backup, or restoring when
///    a fresh backup was what was actually wanted, is still recoverable afterward. Using
///    `VACUUM INTO` here rather than a raw file copy means the safety copy can never be
///    silently missing data that was still sitting in a WAL file — the same reason
///    [`backup_to`] uses it.
/// 4. The backup is copied into place via write-to-a-temp-file-then-rename, so a crash or a
///    full disk mid-copy leaves the original `db_path` untouched rather than half-overwritten
///    (rename onto an existing path is atomic on the same filesystem, which a same-directory
///    temp file guarantees; renaming a file onto an existing file — as opposed to a
///    directory — replaces it on Windows too, via `MOVEFILE_REPLACE_EXISTING`, so no
///    Windows-specific fallback is needed here).
/// 5. Any stale `-wal`/`-shm` sidecar files left over from the old `db_path` are removed —
///    they describe uncommitted changes to a database that, after step 4, no longer exists
///    at that path.
/// 6. `db_path` is reopened via [`connect`], which reapplies the standard pragmas and runs
///    any migrations the backup predates forward — restoring an older backup transparently
///    upgrades its schema, exactly as opening an old database file normally would.
///
/// Restoring while *another* bhtune process (for instance `bhtune-server`, running
/// alongside the CLI) has `db_path` open returns [`DbError::DatabaseInUse`] instead of
/// proceeding — see [`exclusive_pre_restore_snapshot`] for how that's detected and its
/// residual, deliberately-accepted race.
pub async fn restore_from(
    pool: SqlitePool,
    db_path: &Path,
    backup_path: &Path,
    now: DateTime<Utc>,
) -> DbResult<RestoreOutcome> {
    validate_backup_file(backup_path).await?;

    // Wait for every connection to be gracefully closed (not just dropped) before touching
    // the file at the OS level — an open file handle can otherwise block the rename/delete
    // calls below outright, especially on Windows, and so the exclusivity probe below isn't
    // confused by this process's own still-open connections.
    pool.close().await;

    let pre_restore_backup = if db_path.exists() {
        Some(exclusive_pre_restore_snapshot(db_path, now).await?)
    } else {
        None
    };

    let tmp_path = sibling_path(db_path, ".restoring-tmp");
    std::fs::copy(backup_path, &tmp_path).map_err(DbError::Io)?;
    std::fs::rename(&tmp_path, db_path).map_err(DbError::Io)?;

    for suffix in ["-wal", "-shm"] {
        let sidecar = sibling_path(db_path, suffix);
        if sidecar.exists() {
            std::fs::remove_file(&sidecar).map_err(DbError::Io)?;
        }
    }

    let pool = connect(db_path).await?;
    Ok(RestoreOutcome {
        pool,
        pre_restore_backup,
    })
}

/// Confirms nothing else still holds `db_path` open, and — while that exclusivity is
/// proven — takes a `VACUUM INTO` safety copy of it before [`restore_from`] overwrites it.
///
/// The check is `PRAGMA wal_checkpoint(TRUNCATE)`'s own `busy` column: fully truncating the
/// WAL requires that no other connection, in this process or any other, is still reading or
/// writing the database, so a nonzero `busy` result is SQLite's own proof that something
/// else has it open. No separate lock file or advisory-lock scheme is needed to get that
/// answer.
///
/// This is a point-in-time check, not a held lock: nothing stops a different process from
/// opening `db_path` in the moment between this returning and [`restore_from`]'s later file
/// replacement. That residual race is accepted deliberately — it's the "honest fix for the
/// multi-process case" this was designed for, not a claim of a full distributed lock. A
/// truly exclusive, lock-held-for-the-whole-restore guarantee would need every bhtune
/// process to cooperate through a shared lock file from the moment it opens the database,
/// which is a larger change than this finding's scope.
///
/// Deliberately does not go through [`connect`]: that runs migrations, which this must not
/// do against a database that's about to be discarded and replaced wholesale.
async fn exclusive_pre_restore_snapshot(db_path: &Path, now: DateTime<Utc>) -> DbResult<PathBuf> {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .busy_timeout(EXCLUSIVITY_PROBE_TIMEOUT);
    let probe_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(DbError::Connect)?;

    let (busy, _log, _checkpointed): (i64, i64, i64) =
        sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
            .fetch_one(&probe_pool)
            .await
            .map_err(DbError::Query)?;
    if busy != 0 {
        probe_pool.close().await;
        return Err(DbError::DatabaseInUse(db_path.to_path_buf()));
    }

    let sidecar = pre_restore_backup_path(db_path, now);
    sqlx::query("VACUUM INTO ?")
        .bind(sidecar.display().to_string())
        .execute(&probe_pool)
        .await
        .map_err(DbError::Query)?;

    probe_pool.close().await;
    validate_backup_file(&sidecar).await?;
    Ok(sidecar)
}

/// Opens `path` read-only and confirms it's a usable bhtune database: SQLite's own
/// `PRAGMA integrity_check`, plus a real `tune_runs` table (a cheap proxy for "this is a
/// bhtune database", not just any SQLite file). Read-only so validating a backup can never
/// itself be the thing that corrupts or migrates it.
async fn validate_backup_file(path: &Path) -> DbResult<()> {
    if !path.exists() {
        return Err(DbError::InvalidBackup(format!(
            "{} does not exist",
            path.display()
        )));
    }

    let options = SqliteConnectOptions::new().filename(path).read_only(true);
    let check_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|error| {
            DbError::InvalidBackup(format!("failed to open as a SQLite database: {error}"))
        })?;

    let (integrity,): (String,) = sqlx::query_as("PRAGMA integrity_check")
        .fetch_one(&check_pool)
        .await
        .map_err(|error| {
            DbError::InvalidBackup(format!("failed to run integrity_check: {error}"))
        })?;
    if integrity != "ok" {
        return Err(DbError::InvalidBackup(format!(
            "integrity_check reported: {integrity}"
        )));
    }

    let tune_runs_table_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'tune_runs'",
    )
    .fetch_one(&check_pool)
    .await
    .map_err(|error| DbError::InvalidBackup(format!("failed to inspect its schema: {error}")))?;
    if tune_runs_table_count == 0 {
        return Err(DbError::InvalidBackup(
            "missing the tune_runs table -- doesn't look like a bhtune database".to_string(),
        ));
    }

    Ok(())
}

/// A backup taken immediately before `db_path` was overwritten by [`restore_from`], named
/// `<db file name>.pre-restore-<UTC timestamp>.bak` in the same directory.
fn pre_restore_backup_path(db_path: &Path, now: DateTime<Utc>) -> PathBuf {
    sibling_path(
        db_path,
        &format!(".pre-restore-{}.bak", now.format("%Y%m%dT%H%M%SZ")),
    )
}

/// `db_path` with `suffix` appended directly to its file name (not its extension replaced —
/// SQLite's own `-wal`/`-shm` sidecar convention is a literal suffix, e.g. `bhtune.db-wal`,
/// which [`Path::with_extension`] cannot express).
fn sibling_path(db_path: &Path, suffix: &str) -> PathBuf {
    let file_name = db_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("bhtune.db");
    db_path.with_file_name(format!("{file_name}{suffix}"))
}

#[cfg(test)]
mod tests {
    use bhtune_core::built_in_templates;

    use super::*;
    use crate::models::{DcsTemplateRow, TemplateOrigin};

    async fn template_names(pool: &SqlitePool) -> Vec<String> {
        let mut names: Vec<String> = sqlx::query_scalar("SELECT name FROM dcs_templates")
            .fetch_all(pool)
            .await
            .unwrap();
        names.sort();
        names
    }

    async fn seed_one_template(pool: &SqlitePool, name: &str, now: DateTime<Utc>) {
        let mut template = built_in_templates().into_iter().next().unwrap();
        template.name = name.to_string();
        DcsTemplateRow::insert(pool, &template, TemplateOrigin::Builtin, now)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn backup_to_produces_a_reopenable_file_with_the_same_data() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let pool = connect(&dir.path().join("live.db")).await.unwrap();
        seed_one_template(&pool, "Backed Up Template", now).await;

        let dest = dir.path().join("backup.db");
        backup_to(&pool, &dest).await.unwrap();
        assert!(dest.exists());

        let reopened = connect(&dest).await.unwrap();
        assert_eq!(
            template_names(&reopened).await,
            vec!["Backed Up Template".to_string()]
        );
    }

    #[tokio::test]
    async fn backup_to_refuses_to_overwrite_an_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let pool = connect(&dir.path().join("live.db")).await.unwrap();

        let dest = dir.path().join("backup.db");
        std::fs::write(&dest, b"already here").unwrap();

        let err = backup_to(&pool, &dest).await.unwrap_err();
        assert!(matches!(err, DbError::BackupDestinationExists(path) if path == dest));
        // The pre-existing file must be left exactly as it was, not touched or truncated.
        assert_eq!(std::fs::read(&dest).unwrap(), b"already here");
    }

    #[tokio::test]
    async fn restore_from_replaces_the_live_database_with_the_backups_contents() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();

        let backup_source_pool = connect(&dir.path().join("source.db")).await.unwrap();
        seed_one_template(&backup_source_pool, "From Backup", now).await;
        let backup_path = dir.path().join("backup.db");
        backup_to(&backup_source_pool, &backup_path).await.unwrap();

        let live_path = dir.path().join("live.db");
        let live_pool = connect(&live_path).await.unwrap();
        seed_one_template(&live_pool, "Still Live", now).await;

        let outcome = restore_from(live_pool, &live_path, &backup_path, now)
            .await
            .unwrap();

        assert_eq!(
            template_names(&outcome.pool).await,
            vec!["From Backup".to_string()],
            "restoring must replace the live data with the backup's, not merge the two"
        );
    }

    #[tokio::test]
    async fn restore_from_writes_a_pre_restore_safety_copy_of_the_previous_live_file() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();

        let backup_source_pool = connect(&dir.path().join("source.db")).await.unwrap();
        let backup_path = dir.path().join("backup.db");
        backup_to(&backup_source_pool, &backup_path).await.unwrap();

        let live_path = dir.path().join("live.db");
        let live_pool = connect(&live_path).await.unwrap();
        seed_one_template(&live_pool, "About To Be Overwritten", now).await;

        let outcome = restore_from(live_pool, &live_path, &backup_path, now)
            .await
            .unwrap();

        let safety_copy = outcome
            .pre_restore_backup
            .expect("a live db existed before the restore, so a safety copy must be made");
        assert!(safety_copy.exists());
        let safety_copy_pool = connect(&safety_copy).await.unwrap();
        assert_eq!(
            template_names(&safety_copy_pool).await,
            vec!["About To Be Overwritten".to_string()],
            "the safety copy must preserve the old data, not the newly restored data"
        );
    }

    #[tokio::test]
    async fn restore_from_reports_no_pre_restore_backup_when_db_path_is_a_fresh_install() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();

        let backup_source_pool = connect(&dir.path().join("source.db")).await.unwrap();
        let backup_path = dir.path().join("backup.db");
        backup_to(&backup_source_pool, &backup_path).await.unwrap();

        // A path that has never been connect()'d, matching a fresh install choosing to
        // "restore" as its very first action instead of starting empty. `restore_from`
        // doesn't require its `pool` argument to already be open against `db_path` — it
        // only needs a pool it can close before touching the file — so an unrelated
        // in-memory pool exercises this branch just as well as a real one would.
        let live_path = dir.path().join("never-existed.db");
        let live_pool = crate::pool::connect_in_memory().await.unwrap();

        let outcome = restore_from(live_pool, &live_path, &backup_path, now)
            .await
            .unwrap();
        assert_eq!(outcome.pre_restore_backup, None);
    }

    #[tokio::test]
    async fn restore_from_rejects_an_invalid_backup_file_without_touching_the_live_database() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();

        let live_path = dir.path().join("live.db");
        let live_pool = connect(&live_path).await.unwrap();
        seed_one_template(&live_pool, "Must Survive", now).await;

        let bogus_backup = dir.path().join("not-a-database.db");
        std::fs::write(&bogus_backup, b"not a sqlite file at all").unwrap();

        let err = restore_from(live_pool, &live_path, &bogus_backup, now)
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::InvalidBackup(_)));

        // Validation must run before the live database is touched at all.
        let live_pool_again = connect(&live_path).await.unwrap();
        assert_eq!(
            template_names(&live_pool_again).await,
            vec!["Must Survive".to_string()]
        );
    }

    // The remaining tests exercise `validate_backup_file`'s four distinct failure branches
    // directly, since `restore_from`/`backup_to` collapse all of them to the same
    // `DbError::InvalidBackup` variant and none of these specific underlying causes are
    // otherwise reachable through the public API alone.

    #[tokio::test]
    async fn validate_backup_file_reports_a_path_that_does_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("never-written.db");

        let err = validate_backup_file(&missing).await.unwrap_err();
        match err {
            DbError::InvalidBackup(message) => assert!(message.contains("does not exist")),
            other => panic!("expected InvalidBackup, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn validate_backup_file_reports_a_path_that_cannot_be_opened_as_sqlite_at_all() {
        let dir = tempfile::tempdir().unwrap();
        // A directory exists, so the earlier `path.exists()` check passes, but SQLite
        // cannot open a directory as a database file — this fails at connection time,
        // before any query (including `PRAGMA integrity_check`) is ever issued.
        let err = validate_backup_file(dir.path()).await.unwrap_err();
        match err {
            DbError::InvalidBackup(message) => {
                assert!(message.contains("failed to open as a SQLite database"))
            }
            other => panic!("expected InvalidBackup, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn validate_backup_file_reports_a_database_that_fails_its_integrity_check() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("corrupt.db");

        let pool = connect(&db_path).await.unwrap();
        pool.close().await;

        // SQLite's file header stores "total number of freelist pages" at byte offset 36
        // (big-endian u32); a fresh database has no freed pages, so the linked list of
        // freelist trunk pages is empty and this count is 0. Declaring a nonzero count here
        // (without actually creating any freelist pages) is a direct, header-only
        // inconsistency that `PRAGMA integrity_check` detects deterministically -- it
        // doesn't depend on the table schema or how much data is in the file, unlike
        // corrupting page content, which is fragile across page-layout and free-space
        // details `integrity_check` may or may not happen to walk over.
        use std::io::{Seek, SeekFrom, Write};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&db_path)
            .unwrap();
        file.seek(SeekFrom::Start(36)).unwrap();
        file.write_all(&5u32.to_be_bytes()).unwrap();
        drop(file);

        let err = validate_backup_file(&db_path).await.unwrap_err();
        match err {
            DbError::InvalidBackup(message) => {
                assert!(message.contains("integrity_check reported"))
            }
            other => panic!("expected InvalidBackup, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn validate_backup_file_reports_a_valid_sqlite_file_with_no_tune_runs_table() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("not-a-bhtune-db.db");

        // A genuine, valid, empty SQLite database that never went through bhtune's own
        // `connect()`/migrations -- passes `integrity_check` cleanly, but has no schema at
        // all, let alone a `tune_runs` table.
        let options = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        pool.close().await;

        let err = validate_backup_file(&db_path).await.unwrap_err();
        match err {
            DbError::InvalidBackup(message) => assert!(message.contains("tune_runs")),
            other => panic!("expected InvalidBackup, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn restore_from_removes_orphaned_wal_and_shm_sidecars_with_no_backing_connection() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();

        let backup_source_pool = connect(&dir.path().join("source.db")).await.unwrap();
        seed_one_template(&backup_source_pool, "From Backup", now).await;
        let backup_path = dir.path().join("backup.db");
        backup_to(&backup_source_pool, &backup_path).await.unwrap();

        let live_path = dir.path().join("live.db");
        let live_pool = connect(&live_path).await.unwrap();

        // A graceful `Pool::close()` on the *last* connection to a database already makes
        // SQLite clean up its own real WAL/SHM sidecars (confirmed empirically), so they
        // can never be observed as "stale" by the time `restore_from` reaches its own
        // removal loop. Closing here first, then writing garbage bytes to the same sidecar
        // paths, simulates the case the removal loop actually exists for: files orphaned
        // by something with no live connection at all (an unclean shutdown, a killed
        // process, or a previous restore interrupted between steps) — not ordinary ones a
        // clean close already disposes of.
        live_pool.close().await;
        let wal_path = sibling_path(&live_path, "-wal");
        let shm_path = sibling_path(&live_path, "-shm");
        std::fs::write(&wal_path, b"orphaned wal content").unwrap();
        std::fs::write(&shm_path, b"orphaned shm content").unwrap();
        assert!(wal_path.exists());
        assert!(shm_path.exists());

        // `Pool::close()` is safe to call again inside `restore_from` on an already-closed
        // pool, so the same (closed) handle can still be moved in by value here.
        let outcome = restore_from(live_pool, &live_path, &backup_path, now)
            .await
            .unwrap();

        // The proactive removal in `restore_from` (rather than relying on SQLite's own
        // salt-mismatch detection to safely ignore an incompatible WAL) means the restored
        // database reopens cleanly with exactly the backup's data — not blocked, and not
        // confused by the stale bytes that were sitting at the same sidecar paths.
        assert_eq!(
            template_names(&outcome.pool).await,
            vec!["From Backup".to_string()]
        );
    }

    #[tokio::test]
    async fn restore_from_removes_orphaned_sidecars_even_when_db_path_itself_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();

        let backup_source_pool = connect(&dir.path().join("source.db")).await.unwrap();
        seed_one_template(&backup_source_pool, "From Backup", now).await;
        let backup_path = dir.path().join("backup.db");
        backup_to(&backup_source_pool, &backup_path).await.unwrap();

        // `db_path` itself has never existed -- so the pre-restore exclusivity check and
        // safety snapshot are skipped entirely, there being nothing live to check or copy
        // -- but stale `-wal`/`-shm` sidecar files sit at the paths it would use anyway,
        // e.g. left behind by something that removed just the main file by hand. These
        // must still be cleared before the restored file is written there, or the freshly
        // restored database would be confused by unrelated WAL content sitting beside it.
        // This is the one path that can reach the sidecar-removal loop directly: whenever
        // `db_path` already exists, [`exclusive_pre_restore_snapshot`]'s own
        // open-checkpoint-close sequence already disposes of any stale sidecars as a side
        // effect, before the removal loop ever runs.
        let live_path = dir.path().join("live.db");
        let wal_path = sibling_path(&live_path, "-wal");
        let shm_path = sibling_path(&live_path, "-shm");
        std::fs::write(&wal_path, b"orphaned wal content").unwrap();
        std::fs::write(&shm_path, b"orphaned shm content").unwrap();

        let live_pool = crate::pool::connect_in_memory().await.unwrap();
        let outcome = restore_from(live_pool, &live_path, &backup_path, now)
            .await
            .unwrap();

        // `connect()`'s own final reopen legitimately creates a fresh `-wal` file for the
        // new pool (ordinary WAL-mode operation), so the sidecar path may exist again by
        // now -- what the removal loop must guarantee is that the *garbage* content is
        // gone, not that no file is ever present at that path again.
        if let Ok(contents) = std::fs::read(&wal_path) {
            assert_ne!(contents, b"orphaned wal content");
        }
        assert_eq!(
            template_names(&outcome.pool).await,
            vec!["From Backup".to_string()]
        );
    }

    #[tokio::test]
    async fn restore_from_refuses_when_another_connection_still_holds_the_live_database_open() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();

        let backup_source_pool = connect(&dir.path().join("source.db")).await.unwrap();
        let backup_path = dir.path().join("backup.db");
        backup_to(&backup_source_pool, &backup_path).await.unwrap();

        let live_path = dir.path().join("live.db");
        let live_pool = connect(&live_path).await.unwrap();
        seed_one_template(&live_pool, "Must Survive The Refusal", now).await;

        // A second, independent connection to the same file with an open read transaction,
        // simulating another bhtune process (e.g. `bhtune-server` running alongside the
        // CLI) that still has the database open. `restore_from` only closes the pool *it*
        // was handed, so this second connection is exactly what the exclusivity probe
        // inside `exclusive_pre_restore_snapshot` exists to notice. A `SELECT` (not just
        // `BEGIN`) is required to actually establish a WAL read snapshot -- an empty
        // transaction holds no lock a checkpoint would need to wait on.
        let blocker_pool = connect(&live_path).await.unwrap();
        let mut blocker_tx = blocker_pool.begin().await.unwrap();
        sqlx::query("SELECT COUNT(*) FROM tune_runs")
            .fetch_one(&mut *blocker_tx)
            .await
            .unwrap();

        let err = restore_from(live_pool, &live_path, &backup_path, now)
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::DatabaseInUse(path) if path == live_path));

        // The live database must be left completely untouched by the refused attempt.
        drop(blocker_tx);
        blocker_pool.close().await;
        let live_pool_again = connect(&live_path).await.unwrap();
        assert_eq!(
            template_names(&live_pool_again).await,
            vec!["Must Survive The Refusal".to_string()]
        );
    }

    #[tokio::test]
    async fn restore_from_succeeds_once_the_blocking_connection_is_released() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();

        let backup_source_pool = connect(&dir.path().join("source.db")).await.unwrap();
        seed_one_template(&backup_source_pool, "From Backup", now).await;
        let backup_path = dir.path().join("backup.db");
        backup_to(&backup_source_pool, &backup_path).await.unwrap();

        let live_path = dir.path().join("live.db");
        let live_pool = connect(&live_path).await.unwrap();

        let blocker_pool = connect(&live_path).await.unwrap();
        let mut blocker_tx = blocker_pool.begin().await.unwrap();
        sqlx::query("SELECT COUNT(*) FROM tune_runs")
            .fetch_one(&mut *blocker_tx)
            .await
            .unwrap();

        // `restore_from` already closed `live_pool` before discovering the blocker, so that
        // handle is spent regardless of the outcome -- a real caller (e.g. a CLI command)
        // would need to reconnect before trying again, exactly as this test does below.
        let err = restore_from(live_pool, &live_path, &backup_path, now)
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::DatabaseInUse(_)));

        drop(blocker_tx);
        blocker_pool.close().await;

        let live_pool_retry = connect(&live_path).await.unwrap();
        let outcome = restore_from(live_pool_retry, &live_path, &backup_path, now)
            .await
            .unwrap();
        assert_eq!(
            template_names(&outcome.pool).await,
            vec!["From Backup".to_string()],
            "once the other connection releases its read snapshot, the check must clear and the restore proceed"
        );
    }
}
