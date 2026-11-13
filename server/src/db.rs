use std::{os::unix::fs::PermissionsExt, path::Path, str::FromStr, time::Duration};

use anyhow::Context;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
    SqlitePool,
};

pub const DB_FILE: &str = "mistbox.db";
/// Marker file identifying a Mistbox runtime data directory. The importer
/// refuses to descend into any directory containing it (prevents recursion).
pub const DATA_MARKER: &str = ".mistbox-data";

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Create the data-dir layout if needed and return the DB path.
pub fn prepare_layout(data_dir: &Path) -> anyhow::Result<std::path::PathBuf> {
    for sub in ["", "blobs", "tmp", "cache/latex"] {
        let p = data_dir.join(sub);
        std::fs::create_dir_all(&p).with_context(|| format!("creating {}", p.display()))?;
    }
    let marker = data_dir.join(DATA_MARKER);
    if !marker.exists() {
        std::fs::write(
            &marker,
            "Mistbox runtime data directory (database + blobs). Do not import or edit by hand.\n",
        )?;
    }
    Ok(data_dir.join(DB_FILE))
}

pub fn connect_options(db_path: &Path) -> anyhow::Result<SqliteConnectOptions> {
    Ok(SqliteConnectOptions::from_str("sqlite://")?
        .filename(db_path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(15)))
}

pub async fn open(data_dir: &Path) -> anyhow::Result<SqlitePool> {
    let db_path = prepare_layout(data_dir)?;
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(connect_options(&db_path)?)
        .await
        .with_context(|| format!("opening database {}", db_path.display()))?;
    MIGRATOR.run(&pool).await.context("running migrations")?;
    // Group-writable so a one-off import container running as
    // `--user <host uid>:<app gid>` can share the database (see README).
    // New -wal/-shm files copy the main file's mode, but ones created before
    // this chmod (first start) do not — so fix all three.
    for suffix in ["", "-wal", "-shm"] {
        let p = std::path::PathBuf::from(format!("{}{suffix}", db_path.display()));
        if let Ok(meta) = std::fs::metadata(&p) {
            let mut perm = meta.permissions();
            if perm.mode() & 0o060 != 0o060 {
                perm.set_mode(perm.mode() | 0o060);
                let _ = std::fs::set_permissions(&p, perm);
            }
        }
    }
    Ok(pool)
}
