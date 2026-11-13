//! Consistent online backup, restore and integrity verification.
//!
//! Backup = `VACUUM INTO` snapshot of the database + every blob referenced by
//! that snapshot, streamed as a tar. Blobs are immutable once committed, and
//! while a backup runs the server's blob GC is paused (maintenance lock), so
//! every referenced blob is still present when it is archived.

use std::{
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{bail, Context};
use sqlx::SqlitePool;

use crate::{db, storage::Store, util::now};

const PREFIX: &str = "mistbox-backup";

async fn set_lock(pool: &SqlitePool, until: i64) -> anyhow::Result<()> {
    sqlx::query("UPDATE maintenance SET backup_until = ? WHERE id = 1").bind(until).execute(pool).await?;
    Ok(())
}

pub async fn backup(pool: &SqlitePool, store: &Store, out: &str) -> anyhow::Result<()> {
    set_lock(pool, now() + 3600).await?;
    let refresher = {
        let pool = pool.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(600)).await;
                let _ = set_lock(&pool, now() + 3600).await;
            }
        })
    };
    let res = backup_inner(pool, store, out).await;
    refresher.abort();
    set_lock(pool, 0).await?;
    res
}

async fn backup_inner(pool: &SqlitePool, store: &Store, out: &str) -> anyhow::Result<()> {
    let snap = store.root().join("tmp").join(format!("snapshot-{}.db", crate::util::random_hex(8)));
    sqlx::query("VACUUM INTO ?").bind(snap.to_string_lossy().to_string()).execute(pool).await?;
    let snap_pool = sqlx::SqlitePool::connect_with(db::connect_options(&snap)?.create_if_missing(false)).await?;
    let blobs: Vec<String> =
        sqlx::query_scalar("SELECT blob_id FROM nodes WHERE blob_id IS NOT NULL").fetch_all(&snap_pool).await?;
    let files = blobs.len();
    // Reset the lock inside the snapshot so a restored instance runs GC normally.
    sqlx::query("UPDATE maintenance SET backup_until = 0").execute(&snap_pool).await?;
    snap_pool.close().await;

    let store = store.clone();
    let out = out.to_string();
    let snap2 = snap.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let writer: Box<dyn Write> = if out == "-" {
            Box::new(std::io::stdout().lock())
        } else {
            Box::new(std::fs::File::create(&out).with_context(|| format!("建立 {out}"))?)
        };
        let mut tar = tar::Builder::new(std::io::BufWriter::new(writer));
        let manifest = serde_json::json!({
            "format": 1, "created_at": crate::util::ts_to_rfc3339(now()), "files": files,
            "app_version": env!("CARGO_PKG_VERSION"),
        });
        let m = serde_json::to_vec_pretty(&manifest)?;
        let mut h = tar::Header::new_gnu();
        h.set_size(m.len() as u64);
        h.set_mode(0o640);
        tar.append_data(&mut h, format!("{PREFIX}/manifest.json"), m.as_slice())?;
        tar.append_path_with_name(&snap2, format!("{PREFIX}/{}", db::DB_FILE))?;
        for id in &blobs {
            let p = store.blob_path(id);
            tar.append_path_with_name(&p, format!("{PREFIX}/blobs/{}/{id}", &id[..2]))
                .with_context(|| format!("讀取 blob {id}"))?;
        }
        tar.into_inner()?.flush()?;
        Ok(())
    })
    .await?;
    let _ = std::fs::remove_file(&snap);
    result?;
    eprintln!("備份完成：資料庫快照 + {files} 個檔案");
    Ok(())
}

/// Restore into `data_dir`. The server must be stopped. Existing data is
/// only replaced with `force`, and is moved aside, not deleted.
pub fn restore(data_dir: &Path, input: &str, force: bool) -> anyhow::Result<()> {
    let db_path = data_dir.join(db::DB_FILE);
    std::fs::create_dir_all(data_dir)?;
    if db_path.exists() {
        if !force {
            bail!("{} 已有資料庫；確認要以備份取代時請加 --force（舊資料會移到 pre-restore-* 資料夾）", data_dir.display());
        }
        let aside = data_dir.join(format!("pre-restore-{}", now()));
        std::fs::create_dir_all(&aside)?;
        for name in [db::DB_FILE, "mistbox.db-wal", "mistbox.db-shm", "blobs"] {
            let p = data_dir.join(name);
            if p.exists() {
                std::fs::rename(&p, aside.join(name))?;
            }
        }
        eprintln!("既有資料已移至 {}", aside.display());
    }
    db::prepare_layout(data_dir)?;
    let reader: Box<dyn Read> = if input == "-" {
        Box::new(std::io::stdin().lock())
    } else {
        Box::new(std::fs::File::open(input).with_context(|| format!("開啟 {input}"))?)
    };
    let mut ar = tar::Archive::new(reader);
    let staging_db = data_dir.join("tmp").join("restore.db");
    let (mut saw_db, mut blobs) = (false, 0usize);
    for entry in ar.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path: PathBuf = entry.path()?.into_owned();
        let comps: Vec<String> = path
            .components()
            .map(|c| match c {
                Component::Normal(s) => Ok(s.to_string_lossy().into_owned()),
                _ => Err(anyhow::anyhow!("備份檔內含不安全的路徑：{}", path.display())),
            })
            .collect::<anyhow::Result<_>>()?;
        let comps: Vec<&str> = comps.iter().map(String::as_str).collect();
        let dest = match comps.as_slice() {
            [PREFIX, "manifest.json"] => continue,
            [PREFIX, f] if *f == db::DB_FILE => {
                saw_db = true;
                staging_db.clone()
            }
            [PREFIX, "blobs", shard, id]
                if id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()) && shard == &&id[..2] =>
            {
                blobs += 1;
                let d = data_dir.join("blobs").join(shard);
                std::fs::create_dir_all(&d)?;
                d.join(id)
            }
            _ => bail!("備份檔內含未知項目：{}", path.display()),
        };
        let mut f = std::fs::File::create(&dest)?;
        std::io::copy(&mut entry, &mut f)?;
        f.sync_all()?;
    }
    if !saw_db {
        bail!("備份檔中沒有資料庫");
    }
    std::fs::rename(&staging_db, &db_path)?;
    eprintln!("還原完成：資料庫 + {blobs} 個檔案。建議接著執行 `mistbox verify`。");
    Ok(())
}

#[derive(Default, Debug)]
pub struct VerifyReport {
    pub checked: usize,
    pub missing: Vec<String>,
    pub corrupt: Vec<String>,
    pub orphans: Vec<String>,
}

/// Re-hash every stored file and compare with the recorded SHA-256.
pub async fn verify(pool: &SqlitePool, store: &Store) -> anyhow::Result<VerifyReport> {
    let rows: Vec<(String, String, i64, Option<String>)> =
        sqlx::query_as("SELECT id, blob_id, size, sha256 FROM nodes WHERE kind = 'file'").fetch_all(pool).await?;
    let mut r = VerifyReport::default();
    let mut referenced = std::collections::HashSet::new();
    for (id, blob, size, sha) in rows {
        referenced.insert(blob.clone());
        r.checked += 1;
        let path = crate::nodes::path_of(pool, &id, crate::nodes::ROOT_ID).await.unwrap_or(id.clone());
        match store.hash_blob(&blob).await {
            Err(_) => r.missing.push(path),
            Ok((s, h)) if s as i64 != size || Some(h.as_str()) != sha.as_deref() => r.corrupt.push(path),
            Ok(_) => {}
        }
    }
    let queued: std::collections::HashSet<String> =
        sqlx::query_scalar("SELECT blob_id FROM blob_gc").fetch_all(pool).await?.into_iter().collect();
    for b in store.list_blobs()? {
        if !referenced.contains(&b) && !queued.contains(&b) {
            r.orphans.push(b);
        }
    }
    Ok(r)
}
