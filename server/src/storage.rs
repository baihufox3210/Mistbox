//! Blob store. File bodies are streamed into `tmp/` while hashing, fsynced,
//! then atomically renamed into `blobs/<xx>/<blob_id>`. Blob ids are
//! server-generated; user-supplied names never touch host paths.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

#[derive(Clone)]
pub struct Store {
    root: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("file exceeds the size limit of {0} bytes")]
    TooLarge(u64),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

/// A fully written, hashed file in `tmp/` that is not yet visible anywhere.
/// Dropping it without `commit` removes the temp file.
pub struct Staged {
    tmp: Option<PathBuf>,
    pub size: u64,
    pub sha256: String,
}

impl Drop for Staged {
    fn drop(&mut self) {
        if let Some(p) = self.tmp.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

fn valid_blob_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

impl Store {
    pub fn new(data_dir: &Path) -> std::io::Result<Self> {
        let root = data_dir.to_path_buf();
        std::fs::create_dir_all(root.join("blobs"))?;
        std::fs::create_dir_all(root.join("tmp"))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn blob_path(&self, blob_id: &str) -> PathBuf {
        assert!(valid_blob_id(blob_id), "invalid blob id");
        self.root.join("blobs").join(&blob_id[..2]).join(blob_id)
    }

    pub fn latex_cache_dir(&self) -> PathBuf {
        self.root.join("cache").join("latex")
    }

    /// Remove leftovers from interrupted uploads. Call at startup only.
    pub fn clean_tmp(&self) {
        if let Ok(rd) = std::fs::read_dir(self.root.join("tmp")) {
            for e in rd.flatten() {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }

    /// Stream `reader` into a temp file, enforcing `max` bytes and hashing as we go.
    pub async fn stage<R: AsyncRead + Unpin>(&self, mut reader: R, max: u64) -> Result<Staged, StoreError> {
        let tmp = self.root.join("tmp").join(format!("{}.part", crate::util::random_hex(16)));
        let mut staged = Staged { tmp: Some(tmp.clone()), size: 0, sha256: String::new() };
        let mut file = tokio::fs::OpenOptions::new().write(true).create_new(true).open(&tmp).await?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = reader.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            staged.size += n as u64;
            if staged.size > max {
                return Err(StoreError::TooLarge(max));
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n]).await?;
        }
        file.flush().await?;
        file.sync_all().await?;
        staged.sha256 = hex::encode(hasher.finalize());
        Ok(staged)
    }

    /// Atomically move a staged file into the blob store; returns the new blob id.
    pub async fn commit(&self, mut staged: Staged) -> std::io::Result<String> {
        let blob_id = crate::util::random_hex(16);
        let dest = self.blob_path(&blob_id);
        let dir = dest.parent().unwrap().to_path_buf();
        tokio::fs::create_dir_all(&dir).await?;
        let tmp = staged.tmp.take().unwrap();
        if let Err(e) = tokio::fs::rename(&tmp, &dest).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e);
        }
        // Persist the directory entry.
        if let Ok(d) = tokio::fs::File::open(&dir).await {
            let _ = d.sync_all().await;
        }
        Ok(blob_id)
    }

    pub async fn remove_blob(&self, blob_id: &str) {
        if valid_blob_id(blob_id) {
            let _ = tokio::fs::remove_file(self.blob_path(blob_id)).await;
        }
    }

    /// Hash an existing blob (used by `verify`).
    pub async fn hash_blob(&self, blob_id: &str) -> std::io::Result<(u64, String)> {
        let mut f = tokio::fs::File::open(self.blob_path(blob_id)).await?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 256 * 1024];
        let mut size = 0u64;
        loop {
            let n = f.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            size += n as u64;
            hasher.update(&buf[..n]);
        }
        Ok((size, hex::encode(hasher.finalize())))
    }

    /// All blob ids physically present.
    pub fn list_blobs(&self) -> std::io::Result<Vec<String>> {
        let mut out = Vec::new();
        for shard in std::fs::read_dir(self.root.join("blobs"))? {
            let shard = shard?;
            if !shard.file_type()?.is_dir() {
                continue;
            }
            for e in std::fs::read_dir(shard.path())? {
                let name = e?.file_name().to_string_lossy().into_owned();
                if valid_blob_id(&name) {
                    out.push(name);
                }
            }
        }
        Ok(out)
    }
}
