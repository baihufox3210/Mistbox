//! Semantic search (the retrieval half of RAG) with a local embedding model.
//!
//! - Text-like files (code, Markdown, LaTeX, plain text…) up to
//!   `rag_max_file_bytes` are decoded, split into line-ranged chunks and
//!   embedded by a local ONNX model (fastembed, CPU only, model files baked
//!   into the image — nothing is sent anywhere).
//! - Chunks and vectors live in SQLite (`rag_chunks`); an in-memory copy of the
//!   vectors is searched by brute-force cosine similarity (fine for a personal
//!   drive: 100k chunks × 384 dims ≈ 150 MB, a few ms per query).
//! - A background indexer re-processes any file whose SHA-256 differs from what
//!   was indexed, so uploads, edits, imports (even from the CLI) and model
//!   changes are picked up automatically. Deleted files cascade away.
//! - Sensitive files are never indexed. Every search result is re-authorized
//!   against the caller (API key scope + `read`).

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, RwLock},
    time::Duration,
};

use serde::Serialize;
use sqlx::SqlitePool;
use tokio::{io::AsyncReadExt, sync::Notify};

use crate::{
    auth::Principal,
    authz::{self, Perm},
    nodes::{self, Node, NodeView},
    storage::Store,
    textview,
    util::now,
};

/// Prefix a query with a CTE `t(id)` = the recycle bin subtree.
macro_rules! const_format_trash {
    ($sql:literal) => {
        concat!(
            "WITH RECURSIVE t(id) AS (SELECT 'trash' UNION ALL SELECT n2.id FROM nodes n2 JOIN t ON n2.parent_id = t.id) ",
            $sql
        )
    };
}

// ------------------------------------------------------------------ embedders

pub trait Embedder: Send + Sync {
    /// Identifier stored with every indexed document (changing it re-indexes).
    fn id(&self) -> &str;
    /// Embed texts; `query` selects query vs passage formatting where the model needs it.
    fn embed(&self, texts: &[String], query: bool) -> anyhow::Result<Vec<Vec<f32>>>;
}

fn normalise(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
}

/// Deterministic lexical "embedding" (hashed word / CJK-bigram counts).
/// Used by tests and as an explicit opt-in fallback; not semantic.
pub struct HashEmbedder;

impl HashEmbedder {
    const DIM: usize = 512;
    fn tokens(s: &str) -> Vec<String> {
        let lower = s.to_lowercase();
        let mut out = Vec::new();
        let mut word = String::new();
        let mut prev_cjk: Option<char> = None;
        for c in lower.chars() {
            let cjk = ('\u{3400}'..='\u{9fff}').contains(&c) || ('\u{f900}'..='\u{faff}').contains(&c);
            if cjk {
                if !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                }
                out.push(c.to_string());
                if let Some(p) = prev_cjk {
                    out.push(format!("{p}{c}"));
                }
                prev_cjk = Some(c);
            } else {
                prev_cjk = None;
                if c.is_alphanumeric() {
                    word.push(c);
                } else if !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                }
            }
        }
        if !word.is_empty() {
            out.push(word);
        }
        out
    }
}

impl Embedder for HashEmbedder {
    fn id(&self) -> &str {
        "hash-v1"
    }
    fn embed(&self, texts: &[String], _query: bool) -> anyhow::Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|t| {
                let mut v = vec![0f32; Self::DIM];
                for tok in Self::tokens(t) {
                    let mut h: u64 = 0xcbf29ce484222325;
                    for b in tok.bytes() {
                        h ^= b as u64;
                        h = h.wrapping_mul(0x100000001b3);
                    }
                    v[(h % Self::DIM as u64) as usize] += 1.0;
                }
                normalise(&mut v);
                v
            })
            .collect())
    }
}

/// Supported local models: (config name, description, query prefix, passage prefix).
pub const MODELS: &[(&str, &str, &str, &str)] = &[
    ("multilingual-e5-small", "intfloat/multilingual-e5-small · 384 維 · 100+ 語言（預設）", "query: ", "passage: "),
    ("multilingual-e5-base", "intfloat/multilingual-e5-base · 768 維 · 較準確、較慢", "query: ", "passage: "),
    ("bge-small-zh", "BAAI/bge-small-zh-v1.5 · 512 維 · 中文為主", "为这个句子生成表示以用于检索相关文章：", ""),
    ("paraphrase-multilingual-minilm", "paraphrase-multilingual-MiniLM-L12-v2（量化）· 384 維 · 最小", "", ""),
];

#[cfg(feature = "embed")]
mod fast {
    use std::sync::Mutex;

    use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};

    pub struct FastEmbedder {
        model: Mutex<TextEmbedding>,
        id: String,
        qp: &'static str,
        pp: &'static str,
    }

    fn model_enum(name: &str) -> Option<EmbeddingModel> {
        Some(match name {
            "multilingual-e5-small" => EmbeddingModel::MultilingualE5Small,
            "multilingual-e5-base" => EmbeddingModel::MultilingualE5Base,
            "bge-small-zh" => EmbeddingModel::BGESmallZHV15,
            "paraphrase-multilingual-minilm" => EmbeddingModel::ParaphraseMLMiniLML12V2Q,
            _ => return None,
        })
    }

    pub fn load(name: &str, dir: &std::path::Path, threads: usize) -> anyhow::Result<FastEmbedder> {
        let (_, _, qp, pp) =
            super::MODELS.iter().find(|m| m.0 == name).ok_or_else(|| anyhow::anyhow!("unknown model {name}"))?;
        let m = model_enum(name).ok_or_else(|| anyhow::anyhow!("unknown model {name}"))?;
        let opts = TextInitOptions::new(m)
            .with_cache_dir(dir.to_path_buf())
            .with_show_download_progress(false)
            .with_intra_threads(threads.max(1));
        let model = TextEmbedding::try_new(opts)?;
        // The chunking version is part of the id so changing it re-indexes everything.
        Ok(FastEmbedder { model: Mutex::new(model), id: format!("{name}@c{}", super::CHUNK_CHARS), qp, pp })
    }

    impl super::Embedder for FastEmbedder {
        fn id(&self) -> &str {
            &self.id
        }
        fn embed(&self, texts: &[String], query: bool) -> anyhow::Result<Vec<Vec<f32>>> {
            let prefix = if query { self.qp } else { self.pp };
            let input: Vec<String> = texts.iter().map(|t| format!("{prefix}{t}")).collect();
            // Small batches keep ONNX Runtime's retained arena (peak activations) low.
            let mut out = self.model.lock().unwrap().embed(input, Some(4))?;
            out.iter_mut().for_each(|v| super::normalise(v));
            Ok(out)
        }
    }
}

/// Download a model into `dir` (used at Docker build time; runtime never downloads).
pub fn download_model(name: &str, dir: &std::path::Path) -> anyhow::Result<()> {
    #[cfg(feature = "embed")]
    {
        let e = fast::load(name, dir, 1)?;
        let v = e.embed(&["測試 test".to_string()], true)?;
        eprintln!("模型 {name} 已就緒（{} 維）：{}", v[0].len(), dir.display());
        Ok(())
    }
    #[cfg(not(feature = "embed"))]
    {
        let _ = (name, dir);
        anyhow::bail!("此版本未以 `embed` feature 編譯，無法下載模型")
    }
}

// ------------------------------------------------------------------- chunking

/// Target chunk size in characters. ~500 CJK characters still fit the
/// 512-token window of the e5 / bge models without truncation.
pub const CHUNK_CHARS: usize = 500;

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub text: String,
    pub line_start: usize,
    pub line_end: usize,
}

/// Split text into ~`target`-character chunks on line boundaries, carrying a
/// couple of lines of overlap. Very long lines are split in place.
pub fn chunk_text(text: &str, target: usize) -> Vec<Chunk> {
    const OVERLAP_LINES: usize = 2;
    let mut pieces: Vec<(usize, String)> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        if chars.len() > target * 2 {
            for part in chars.chunks(target) {
                pieces.push((i + 1, part.iter().collect()));
            }
        } else {
            pieces.push((i + 1, line.to_string()));
        }
    }
    let mut out = Vec::new();
    let mut cur: Vec<(usize, String)> = Vec::new();
    let mut size = 0usize;
    let flush = |cur: &Vec<(usize, String)>, out: &mut Vec<Chunk>| {
        let text = cur.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>().join("\n");
        if !text.trim().is_empty() {
            out.push(Chunk { text, line_start: cur[0].0, line_end: cur[cur.len() - 1].0 });
        }
    };
    for p in pieces {
        size += p.1.chars().count() + 1;
        cur.push(p);
        if size >= target {
            flush(&cur, &mut out);
            let keep = if cur.len() > OVERLAP_LINES + 1 { OVERLAP_LINES } else { 0 };
            cur = cur.split_off(cur.len() - keep);
            size = cur.iter().map(|(_, l)| l.chars().count() + 1).sum();
        }
    }
    if !cur.is_empty() && out.last().is_none_or(|c: &Chunk| c.line_end < cur[cur.len() - 1].0) {
        flush(&cur, &mut out);
    }
    out
}

// ---------------------------------------------------------------------- index

struct Entry {
    chunk_id: i64,
    vec: Vec<f32>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Status {
    pub enabled: bool,
    pub model: String,
    pub model_description: String,
    pub state: String,
    pub error: Option<String>,
    pub indexed_files: i64,
    pub chunks: i64,
    pub pending: i64,
    pub skipped: i64,
    pub failed: i64,
}

pub struct Rag {
    pub model_name: String,
    model_dir: PathBuf,
    threads: usize,
    max_bytes: u64,
    embedder: RwLock<Option<Arc<dyn Embedder>>>,
    state: RwLock<(String, Option<String>)>,
    index: RwLock<HashMap<String, Vec<Entry>>>,
    wake: Notify,
}

#[derive(Serialize, Debug)]
pub struct Hit {
    pub node: NodeView,
    pub score: f32,
    pub seq: i64,
    pub line_start: i64,
    pub line_end: i64,
    pub text: String,
}

const TEXT_EXT_DENY: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "avif", "bmp", "ico", "pdf", "zip", "7z", "rar", "gz", "tgz", "xz", "bz2",
    "zst", "tar", "mp3", "wav", "ogg", "flac", "m4a", "aac", "opus", "mp4", "webm", "mov", "mkv", "avi", "exe", "dll",
    "so", "dylib", "bin", "o", "a", "class", "jar", "wasm", "pyc", "db", "sqlite", "ttf", "otf", "woff", "woff2",
    "doc", "docx", "xls", "xlsx", "ppt", "pptx", "psd", "iso", "heic", "tif", "tiff",
];

impl Rag {
    /// `None` when semantic search is disabled.
    pub fn from_config(cfg: &crate::config::Config) -> Option<Arc<Rag>> {
        let name = match cfg.rag_model.as_str() {
            "off" | "" | "none" => return None,
            "auto" if cfg!(feature = "embed") => "multilingual-e5-small".to_string(),
            "auto" => return None,
            other => other.to_string(),
        };
        Some(Arc::new(Rag {
            model_name: name,
            model_dir: cfg.rag_model_dir.clone(),
            threads: cfg.rag_threads,
            max_bytes: cfg.rag_max_file_bytes,
            embedder: RwLock::new(None),
            state: RwLock::new(("loading".into(), None)),
            index: RwLock::new(HashMap::new()),
            wake: Notify::new(),
        }))
    }

    fn embedder(&self) -> Option<Arc<dyn Embedder>> {
        self.embedder.read().unwrap().clone()
    }

    fn set_state(&self, s: &str, err: Option<String>) {
        *self.state.write().unwrap() = (s.into(), err);
    }

    /// Ask the indexer to run soon (after uploads / edits / deletes).
    pub fn poke(&self) {
        self.wake.notify_one();
    }

    /// Load the model (blocking) — separate so tests can call it synchronously.
    pub async fn load_model(&self) -> anyhow::Result<()> {
        let name = self.model_name.clone();
        let dir = self.model_dir.clone();
        let threads = self.threads;
        let e: Arc<dyn Embedder> = tokio::task::spawn_blocking(move || -> anyhow::Result<Arc<dyn Embedder>> {
            if name == "hash" {
                return Ok(Arc::new(HashEmbedder));
            }
            #[cfg(feature = "embed")]
            {
                Ok(Arc::new(fast::load(&name, &dir, threads)?))
            }
            #[cfg(not(feature = "embed"))]
            {
                let _ = (dir, threads);
                anyhow::bail!("模型 {name} 需要以 `embed` feature 編譯")
            }
        })
        .await??;
        *self.embedder.write().unwrap() = Some(e);
        Ok(())
    }

    /// Background loop: load model, load vectors, then index forever.
    pub fn spawn(self: &Arc<Self>, db: SqlitePool, store: Store) {
        let rag = self.clone();
        tokio::spawn(async move {
            if let Err(e) = rag.load_model().await {
                tracing::error!(error = %e, model = %rag.model_name, "embedding model failed to load; semantic search disabled");
                rag.set_state("error", Some(format!("模型載入失敗：{e}")));
                return;
            }
            if let Err(e) = rag.load_index(&db).await {
                tracing::error!(error = %e, "loading vector index failed");
            }
            rag.set_state("ready", None);
            tracing::info!(model = %rag.model_name, "semantic search ready");
            loop {
                rag.set_state("indexing", None);
                match rag.index_pending(&db, &store).await {
                    Ok(n) if n > 0 => tracing::info!(files = n, "indexed for semantic search"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "indexing pass failed"),
                }
                rag.set_state("ready", None);
                let _ = tokio::time::timeout(Duration::from_secs(30), rag.wake.notified()).await;
                // Debounce bursts (folder uploads).
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }

    fn model_id(&self) -> String {
        self.embedder().map(|e| e.id().to_string()).unwrap_or_else(|| self.model_name.clone())
    }

    pub async fn load_index(&self, db: &SqlitePool) -> anyhow::Result<()> {
        let rows: Vec<(i64, String, Vec<u8>)> = sqlx::query_as(
            "SELECT c.id, c.node_id, c.embedding FROM rag_chunks c JOIN rag_docs d ON d.node_id = c.node_id
             WHERE d.model = ? AND d.status = 'indexed'",
        )
        .bind(self.model_id())
        .fetch_all(db)
        .await?;
        let mut map: HashMap<String, Vec<Entry>> = HashMap::new();
        for (id, node, blob) in rows {
            map.entry(node).or_default().push(Entry { chunk_id: id, vec: bytes_to_vec(&blob) });
        }
        *self.index.write().unwrap() = map;
        Ok(())
    }

    async fn record(&self, db: &SqlitePool, n: &Node, status: &str, detail: &str) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO rag_docs (node_id, sha256, model, status, detail, chunks, indexed_at) VALUES (?, ?, ?, ?, ?, 0, ?)
             ON CONFLICT(node_id) DO UPDATE SET sha256 = excluded.sha256, model = excluded.model, status = excluded.status,
               detail = excluded.detail, chunks = 0, indexed_at = excluded.indexed_at",
        )
        .bind(&n.id)
        .bind(n.sha256.as_deref().unwrap_or(""))
        .bind(self.model_id())
        .bind(status)
        .bind(detail)
        .bind(now())
        .execute(db)
        .await?;
        sqlx::query("DELETE FROM rag_chunks WHERE node_id = ?").bind(&n.id).execute(db).await?;
        self.index.write().unwrap().remove(&n.id);
        Ok(())
    }

    /// Index every file whose content (or the model) changed. Returns files processed.
    pub async fn index_pending(&self, db: &SqlitePool, store: &Store) -> anyhow::Result<usize> {
        let Some(embedder) = self.embedder() else { return Ok(0) };
        let model = embedder.id().to_string();
        // Files that became sensitive are dropped from the index.
        sqlx::query("DELETE FROM rag_chunks WHERE node_id IN (SELECT id FROM nodes WHERE sensitive = 1)").execute(db).await?;
        sqlx::query("DELETE FROM rag_docs WHERE node_id IN (SELECT id FROM nodes WHERE sensitive = 1)").execute(db).await?;
        // …and so are items in the recycle bin (re-indexed when restored).
        sqlx::query(const_format_trash!("DELETE FROM rag_chunks WHERE node_id IN (SELECT id FROM t)")).execute(db).await?;
        sqlx::query(const_format_trash!("DELETE FROM rag_docs WHERE node_id IN (SELECT id FROM t)")).execute(db).await?;
        // Drop in-memory entries for deleted / sensitive / stale docs.
        let live: HashSet<String> =
            sqlx::query_scalar("SELECT node_id FROM rag_docs WHERE status = 'indexed' AND model = ?")
                .bind(&model)
                .fetch_all(db)
                .await?
                .into_iter()
                .collect();
        self.index.write().unwrap().retain(|k, _| live.contains(k));

        let mut done = 0;
        loop {
            let batch: Vec<Node> = sqlx::query_as(const_format_trash!(
                "SELECT n.* FROM nodes n LEFT JOIN rag_docs d ON d.node_id = n.id
                 WHERE n.kind = 'file' AND n.sensitive = 0 AND n.id NOT IN (SELECT id FROM t)
                   AND (d.node_id IS NULL OR d.sha256 <> n.sha256 OR d.model <> ?)
                 ORDER BY n.modified_at DESC LIMIT 20"
            ))
            .bind(&model)
            .fetch_all(db)
            .await?;
            if batch.is_empty() {
                break;
            }
            for n in &batch {
                if let Err(e) = self.index_node(db, store, &embedder, n).await {
                    tracing::warn!(node = %n.id, error = %e, "indexing failed");
                    self.record(db, n, "failed", &e.to_string()).await?;
                }
                done += 1;
            }
        }
        Ok(done)
    }

    async fn index_node(&self, db: &SqlitePool, store: &Store, embedder: &Arc<dyn Embedder>, n: &Node) -> anyhow::Result<()> {
        let ext = n.name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        let mime = n.mime.as_deref().unwrap_or("");
        if TEXT_EXT_DENY.contains(&ext.as_str()) || mime.starts_with("image/") || mime.starts_with("audio/") || mime.starts_with("video/") {
            return self.record(db, n, "skipped", "非文字檔").await;
        }
        if n.size as u64 > self.max_bytes {
            return self.record(db, n, "skipped", "超過索引大小上限").await;
        }
        let mut buf = Vec::with_capacity(n.size as usize);
        tokio::fs::File::open(store.blob_path(n.blob_id.as_deref().unwrap_or_default()))
            .await?
            .take(self.max_bytes)
            .read_to_end(&mut buf)
            .await?;
        if textview::looks_binary(&buf) {
            return self.record(db, n, "skipped", "二進位內容").await;
        }
        let (text, _, _) = textview::detect(&buf).decode(&buf);
        let chunks = chunk_text(&text, CHUNK_CHARS);
        if chunks.is_empty() {
            return self.record(db, n, "skipped", "沒有文字內容").await;
        }
        let path = nodes::path_of(db, &n.id, nodes::ROOT_ID).await.map_err(|e| anyhow::anyhow!(e.message))?;
        let inputs: Vec<String> = chunks.iter().map(|c| format!("{path}\n{}", c.text)).collect();
        let e = embedder.clone();
        let vecs = tokio::task::spawn_blocking(move || e.embed(&inputs, false)).await??;

        let mut tx = db.begin().await?;
        sqlx::query("DELETE FROM rag_chunks WHERE node_id = ?").bind(&n.id).execute(&mut *tx).await?;
        let mut entries = Vec::with_capacity(chunks.len());
        for (i, (c, v)) in chunks.iter().zip(vecs).enumerate() {
            let id = sqlx::query(
                "INSERT INTO rag_chunks (node_id, seq, line_start, line_end, text, embedding) VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(&n.id)
            .bind(i as i64)
            .bind(c.line_start as i64)
            .bind(c.line_end as i64)
            .bind(&c.text)
            .bind(vec_to_bytes(&v))
            .execute(&mut *tx)
            .await?
            .last_insert_rowid();
            entries.push(Entry { chunk_id: id, vec: v });
        }
        sqlx::query(
            "INSERT INTO rag_docs (node_id, sha256, model, status, detail, chunks, indexed_at) VALUES (?, ?, ?, 'indexed', NULL, ?, ?)
             ON CONFLICT(node_id) DO UPDATE SET sha256 = excluded.sha256, model = excluded.model, status = 'indexed',
               detail = NULL, chunks = excluded.chunks, indexed_at = excluded.indexed_at",
        )
        .bind(&n.id)
        .bind(n.sha256.as_deref().unwrap_or(""))
        .bind(embedder.id())
        .bind(entries.len() as i64)
        .bind(now())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.index.write().unwrap().insert(n.id.clone(), entries);
        Ok(())
    }

    /// Top-`k` chunks for `query` that the principal may read (max 3 per file).
    pub async fn search(&self, state: &crate::AppState, p: &Principal, query: &str, k: usize) -> anyhow::Result<Vec<Hit>> {
        let Some(embedder) = self.embedder() else { anyhow::bail!("語意搜尋模型尚未就緒") };
        let q = vec![query.to_string()];
        let e = embedder.clone();
        let qv = tokio::task::spawn_blocking(move || e.embed(&q, true)).await??.remove(0);

        let mut scored: Vec<(f32, String, i64)> = {
            let idx = self.index.read().unwrap();
            idx.iter()
                .flat_map(|(node, es)| {
                    let qv = &qv;
                    es.iter().map(move |e| (dot(qv, &e.vec), node.clone(), e.chunk_id))
                })
                .collect()
        };
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));

        let mut allowed: HashMap<String, Option<Node>> = HashMap::new();
        let mut per_file: HashMap<String, usize> = HashMap::new();
        let mut out = Vec::new();
        for (score, node_id, chunk_id) in scored {
            if out.len() >= k {
                break;
            }
            if !allowed.contains_key(&node_id) {
                let n = match nodes::get_opt(&state.db, &node_id).await {
                    Ok(Some(n))
                        if authz::require(state, p, &n, Perm::Read).await.is_ok()
                            && !nodes::in_trash(&state.db, &n.id).await.unwrap_or(true) =>
                    {
                        Some(n)
                    }
                    _ => None,
                };
                allowed.insert(node_id.clone(), n);
            }
            let Some(n) = allowed[&node_id].clone() else { continue };
            let c = per_file.entry(node_id.clone()).or_default();
            if *c >= 3 {
                continue;
            }
            *c += 1;
            let Some((seq, ls, le, text)) = sqlx::query_as::<_, (i64, i64, i64, String)>(
                "SELECT seq, line_start, line_end, text FROM rag_chunks WHERE id = ?",
            )
            .bind(chunk_id)
            .fetch_optional(&state.db)
            .await?
            else {
                continue;
            };
            let path = nodes::path_of(&state.db, &n.id, p.root_id()).await.map_err(|e| anyhow::anyhow!(e.message))?;
            out.push(Hit { node: NodeView::from_node(&n, p.root_id()).with_path(path), score, seq, line_start: ls, line_end: le, text });
        }
        Ok(out)
    }

    pub async fn status(&self, db: &SqlitePool) -> anyhow::Result<Status> {
        let model = self.model_id();
        let counts: (i64, i64, i64, i64) = sqlx::query_as(
            "SELECT COALESCE(SUM(status='indexed'),0), COALESCE(SUM(CASE WHEN status='indexed' THEN chunks END),0),
                    COALESCE(SUM(status='skipped'),0), COALESCE(SUM(status='failed'),0)
             FROM rag_docs WHERE model = ?",
        )
        .bind(&model)
        .fetch_one(db)
        .await?;
        let pending: i64 = sqlx::query_scalar(const_format_trash!(
            "SELECT COUNT(*) FROM nodes n LEFT JOIN rag_docs d ON d.node_id = n.id
             WHERE n.kind = 'file' AND n.sensitive = 0 AND n.id NOT IN (SELECT id FROM t)
               AND (d.node_id IS NULL OR d.sha256 <> n.sha256 OR d.model <> ?)"
        ))
        .bind(&model)
        .fetch_one(db)
        .await?;
        let (state, error) = self.state.read().unwrap().clone();
        let desc = MODELS.iter().find(|m| m.0 == self.model_name).map(|m| m.1).unwrap_or("詞彙雜湊（測試用，非語意）");
        Ok(Status {
            enabled: true,
            model: self.model_name.clone(),
            model_description: desc.to_string(),
            state,
            error,
            indexed_files: counts.0,
            chunks: counts.1,
            pending,
            skipped: counts.2,
            failed: counts.3,
        })
    }

    /// Forget everything; the indexer rebuilds from scratch.
    pub async fn reset(&self, db: &SqlitePool) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM rag_chunks").execute(db).await?;
        sqlx::query("DELETE FROM rag_docs").execute(db).await?;
        self.index.write().unwrap().clear();
        self.poke();
        Ok(())
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return -1.0;
    }
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn vec_to_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn bytes_to_vec(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_keep_line_ranges() {
        let text: String = (1..=100).map(|i| format!("第 {i} 行的內容 line {i}\n")).collect();
        let cs = chunk_text(&text, 200);
        assert!(cs.len() > 5);
        assert_eq!(cs[0].line_start, 1);
        assert_eq!(cs.last().unwrap().line_end, 100);
        for w in cs.windows(2) {
            assert!(w[1].line_start <= w[0].line_end + 1, "contiguous / overlapping");
        }
        assert!(chunk_text("  \n\n ", 100).is_empty());
        let long = "字".repeat(1000);
        let cs = chunk_text(&long, 100);
        assert!(cs.iter().all(|c| c.text.chars().count() <= 250));
    }

    #[test]
    fn hash_embedder_prefers_overlap() {
        let e = HashEmbedder;
        let v = e.embed(&["雲端硬碟 備份".into(), "貓咪 在 墊子 上".into(), "如何備份雲端硬碟".into()], false).unwrap();
        assert!(dot(&v[2], &v[0]) > dot(&v[2], &v[1]));
    }

    #[test]
    fn vec_roundtrip() {
        let v = vec![0.5f32, -1.25, 3.0];
        assert_eq!(bytes_to_vec(&vec_to_bytes(&v)), v);
    }
}
