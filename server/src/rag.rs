//! Semantic search (the retrieval half of RAG) with a local embedding model.
//!
//! - Text-like files (code, Markdown, LaTeX, plain text…) up to
//!   `rag_max_file_bytes` are decoded, split into line-ranged chunks and
//!   embedded by a local ONNX model (fastembed, CPU only, model files baked
//!   into the image — nothing is sent anywhere).
//! - PDFs (up to `rag_max_pdf_bytes`) have their text extracted by the
//!   sandboxed worker container (poppler), then are chunked per page.
//! - The default model is an int8-quantised multilingual-e5-small; ONNX
//!   Runtime's memory arena is disabled and the model is unloaded after
//!   `rag_idle_unload_secs` without use, so an idle server stays small.
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
    time::{Duration, Instant},
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
            let cjk =
                ('\u{3400}'..='\u{9fff}').contains(&c) || ('\u{f900}'..='\u{faff}').contains(&c);
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
    (
        "multilingual-e5-small-q8",
        "multilingual-e5-small int8 量化 · 384 維 · 100+ 語言 · 約 120 MB（預設）",
        "query: ",
        "passage: ",
    ),
    (
        "multilingual-e5-small",
        "intfloat/multilingual-e5-small · 384 維 · 100+ 語言 · 原始精度 470 MB",
        "query: ",
        "passage: ",
    ),
    (
        "multilingual-e5-base",
        "intfloat/multilingual-e5-base · 768 維 · 較準確、較慢",
        "query: ",
        "passage: ",
    ),
    (
        "bge-small-zh",
        "BAAI/bge-small-zh-v1.5 · 512 維 · 中文為主",
        "为这个句子生成表示以用于检索相关文章：",
        "",
    ),
    (
        "paraphrase-multilingual-minilm",
        "paraphrase-multilingual-MiniLM-L12-v2（量化）· 384 維 · 最小",
        "",
        "",
    ),
];

/// Default model when built with the `embed` feature.
pub const DEFAULT_MODEL: &str = "multilingual-e5-small-q8";

#[cfg(feature = "embed")]
mod fast {
    use std::{
        io::Read,
        path::Path,
        sync::{Mutex, OnceLock},
    };

    use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
    use hf_hub::{Cache, Repo, RepoType};
    use ort::{
        environment::Environment,
        session::{Session, builder::GraphOptimizationLevel},
        value::Tensor,
    };
    use sha2::Digest;

    /// The int8 model: pinned repository revision + SHA-256 of every weight /
    /// vocabulary file (verified at download time).
    const Q8_REPO: &str = "Xenova/multilingual-e5-small";
    const Q8_REV: &str = "761b726dd34fb83930e26aab4e9ac3899aa1fa78";
    const Q8_FILES: [(&str, &str); 2] = [
        (
            "onnx/model_quantized.onnx",
            "f80102d3f2a1229f387d3c81909990d8945513e347b0eab049f7de3c6f98c193",
        ),
        (
            "tokenizer.json",
            "0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39",
        ),
    ];
    /// XLM-R window incl. <s> and </s>.
    const MAX_TOKENS: usize = 512;

    fn q8_repo() -> Repo {
        Repo::with_revision(Q8_REPO.into(), RepoType::Model, Q8_REV.into())
    }

    fn sha256_file(p: &Path) -> anyhow::Result<String> {
        let mut f = std::fs::File::open(p)?;
        let mut h = sha2::Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        Ok(hex::encode(h.finalize()))
    }

    /// Fetch the int8 model files into `dir` (build time only) and verify them.
    pub fn download_q8(dir: &Path) -> anyhow::Result<()> {
        let api = hf_hub::api::sync::ApiBuilder::new()
            .with_cache_dir(dir.to_path_buf())
            .with_progress(false)
            .build()?;
        let repo = api.repo(q8_repo());
        for (f, want) in Q8_FILES {
            let got = sha256_file(&repo.get(f)?)?;
            anyhow::ensure!(got == want, "模型檔 {f} 的 SHA-256 不符（{got}），已中止");
        }
        Ok(())
    }

    /// CPU execution provider without ONNX Runtime's growing memory arena.
    fn cpu() -> fastembed::ExecutionProviderDispatch {
        ort::ep::CPU::default().with_arena_allocator(false).build()
    }

    fn ort_env() -> anyhow::Result<Environment> {
        static ENV: OnceLock<Environment> = OnceLock::new();
        if let Some(e) = ENV.get() {
            return Ok(e.clone());
        }
        let e = ort::init().with_name("mistbox").build()?;
        Ok(ENV.get_or_init(|| e).clone())
    }

    enum Backend {
        /// multilingual-e5-small int8: light Unigram encoder + ONNX Runtime directly.
        Q8 {
            tok: Box<crate::unigram::Unigram>,
            session: Session,
        },
        /// Other models via fastembed (Hugging Face tokenizer).
        Fast(Box<TextEmbedding>),
    }

    pub struct FastEmbedder {
        backend: Mutex<Backend>,
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

    fn load_q8(dir: &Path, threads: usize) -> anyhow::Result<Backend> {
        let cache = Cache::new(dir.to_path_buf());
        let repo = cache.repo(q8_repo());
        let path = |f: &str| {
            repo.get(f).ok_or_else(|| {
                anyhow::anyhow!("找不到模型檔 {f}（請先執行 `mistbox rag download-model`）")
            })
        };
        let tok = crate::unigram::Unigram::from_json(&std::fs::read(path(Q8_FILES[1].0)?)?)?;
        let b =
            |e: ort::Error<ort::session::builder::SessionBuilder>| anyhow::anyhow!(e.to_string());
        let session = Session::builder(&ort_env()?)?
            .with_execution_providers([cpu()])
            .map_err(b)?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(b)?
            .with_intra_threads(threads.max(1))
            .map_err(b)?
            .with_inter_threads(1)
            .map_err(b)?
            .commit_from_file(path(Q8_FILES[0].0)?)?;
        Ok(Backend::Q8 {
            tok: Box::new(tok),
            session,
        })
    }

    /// `<s> pieces… </s>`, truncated to the model window (as the HF post-processor does).
    fn xlmr_ids(tok: &crate::unigram::Unigram, text: &str) -> anyhow::Result<Vec<i64>> {
        let pieces = tok.encode(text)?;
        let mut ids = Vec::with_capacity(pieces.len().min(MAX_TOKENS));
        ids.push(0);
        ids.extend(pieces.iter().take(MAX_TOKENS - 2).map(|&p| p as i64));
        ids.push(2);
        Ok(ids)
    }

    /// One text → mean-pooled last hidden state. Texts are run one at a time:
    /// the model is dynamically quantised, so activation ranges (and vectors)
    /// must not depend on batch neighbours; it also keeps peak memory minimal.
    fn embed_q8(
        tok: &crate::unigram::Unigram,
        session: &mut Session,
        text: &str,
    ) -> anyhow::Result<Vec<f32>> {
        let ids = xlmr_ids(tok, text)?;
        let n = ids.len();
        let mut inputs = ort::inputs![
            "input_ids" => Tensor::from_array(([1usize, n], ids))?,
            "attention_mask" => Tensor::from_array(([1usize, n], vec![1i64; n]))?,
        ];
        if session
            .inputs()
            .iter()
            .any(|i| i.name() == "token_type_ids")
        {
            inputs.push((
                "token_type_ids".into(),
                Tensor::from_array(([1usize, n], vec![0i64; n]))?.into(),
            ));
        }
        let out = session.run(inputs)?;
        let (shape, data) = out["last_hidden_state"].try_extract_tensor::<f32>()?;
        let dim = *shape
            .last()
            .ok_or_else(|| anyhow::anyhow!("模型輸出格式錯誤"))? as usize;
        anyhow::ensure!(dim > 0 && data.len() == n * dim, "模型輸出格式錯誤");
        let mut v = vec![0f32; dim];
        for tok in data.chunks_exact(dim) {
            v.iter_mut().zip(tok).for_each(|(a, b)| *a += b);
        }
        v.iter_mut().for_each(|x| *x /= n as f32);
        Ok(v)
    }

    pub fn load(name: &str, dir: &Path, threads: usize) -> anyhow::Result<FastEmbedder> {
        let (_, _, qp, pp) = super::MODELS
            .iter()
            .find(|m| m.0 == name)
            .ok_or_else(|| anyhow::anyhow!("unknown model {name}"))?;
        let backend = if name == super::DEFAULT_MODEL {
            load_q8(dir, threads)?
        } else {
            let m = model_enum(name).ok_or_else(|| anyhow::anyhow!("unknown model {name}"))?;
            let opts = TextInitOptions::new(m)
                .with_cache_dir(dir.to_path_buf())
                .with_show_download_progress(false)
                .with_environment(ort_env()?)
                .with_execution_providers(vec![cpu()])
                .with_intra_threads(threads.max(1));
            Backend::Fast(Box::new(TextEmbedding::try_new(opts)?))
        };
        Ok(FastEmbedder {
            backend: Mutex::new(backend),
            id: super::model_id(name),
            qp,
            pp,
        })
    }

    impl super::Embedder for FastEmbedder {
        fn id(&self) -> &str {
            &self.id
        }
        fn embed(&self, texts: &[String], query: bool) -> anyhow::Result<Vec<Vec<f32>>> {
            let prefix = if query { self.qp } else { self.pp };
            let input: Vec<String> = texts.iter().map(|t| format!("{prefix}{t}")).collect();
            let mut out = match &mut *self.backend.lock().unwrap() {
                Backend::Q8 { tok, session } => input
                    .iter()
                    .map(|t| embed_q8(tok, session, t))
                    .collect::<anyhow::Result<Vec<_>>>()?,
                Backend::Fast(m) => m.embed(input, Some(4))?,
            };
            out.iter_mut().for_each(|v| super::normalise(v));
            Ok(out)
        }
    }
}

/// Identifier stored with indexed documents. The chunking version is part of
/// it so changing either the model or the chunker re-indexes everything.
pub fn model_id(name: &str) -> String {
    if name == "hash" {
        "hash-v1".into()
    } else {
        format!("{name}@c{CHUNK_CHARS}")
    }
}

/// Download a model into `dir` (used at Docker build time; runtime never downloads).
pub fn download_model(name: &str, dir: &std::path::Path) -> anyhow::Result<()> {
    #[cfg(feature = "embed")]
    {
        if name == DEFAULT_MODEL {
            fast::download_q8(dir)?;
        }
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

/// Give freed model memory back to the OS (glibc keeps it otherwise).
fn trim_heap() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        libc::malloc_trim(0);
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
        let text = cur
            .iter()
            .map(|(_, l)| l.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if !text.trim().is_empty() {
            out.push(Chunk {
                text,
                line_start: cur[0].0,
                line_end: cur[cur.len() - 1].0,
            });
        }
    };
    for p in pieces {
        size += p.1.chars().count() + 1;
        cur.push(p);
        if size >= target {
            flush(&cur, &mut out);
            let keep = if cur.len() > OVERLAP_LINES + 1 {
                OVERLAP_LINES
            } else {
                0
            };
            cur = cur.split_off(cur.len() - keep);
            size = cur.iter().map(|(_, l)| l.chars().count() + 1).sum();
        }
    }
    if !cur.is_empty()
        && out
            .last()
            .is_none_or(|c: &Chunk| c.line_end < cur[cur.len() - 1].0)
    {
        flush(&cur, &mut out);
    }
    out
}

/// Chunk extracted PDF text page by page (pdftotext separates pages with a
/// form feed); `line_start` / `line_end` hold the page number.
pub fn chunk_pages(text: &str, target: usize) -> Vec<Chunk> {
    let mut out = Vec::new();
    for (i, page) in text.split('\x0c').enumerate() {
        for mut c in chunk_text(page, target) {
            c.line_start = i + 1;
            c.line_end = i + 1;
            out.push(c);
        }
    }
    out
}

fn is_pdf(n: &Node) -> bool {
    n.mime.as_deref() == Some("application/pdf") || n.name.to_ascii_lowercase().ends_with(".pdf")
}

/// Upper bound on extracted PDF text that is indexed (characters).
const MAX_PDF_TEXT_CHARS: usize = 2_000_000;

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
    /// Whether the model is currently in memory (it is unloaded when idle).
    pub model_loaded: bool,
    pub pdf: bool,
}

pub struct Rag {
    pub model_name: String,
    id: String,
    model_dir: PathBuf,
    threads: usize,
    max_bytes: u64,
    max_pdf_bytes: u64,
    idle_unload: Option<Duration>,
    latex: Option<Arc<crate::latex::LatexClient>>,
    embedder: RwLock<Option<Arc<dyn Embedder>>>,
    load_lock: tokio::sync::Mutex<()>,
    last_used: std::sync::Mutex<Instant>,
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
    /// What `line_start` / `line_end` count: "line" or "page" (PDF).
    pub unit: &'static str,
    pub text: String,
}

const TEXT_EXT_DENY: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "avif", "bmp", "ico", "pdf", "zip", "7z", "rar", "gz",
    "tgz", "xz", "bz2", "zst", "tar", "mp3", "wav", "ogg", "flac", "m4a", "aac", "opus", "mp4",
    "webm", "mov", "mkv", "avi", "exe", "dll", "so", "dylib", "bin", "o", "a", "class", "jar",
    "wasm", "pyc", "db", "sqlite", "ttf", "otf", "woff", "woff2", "doc", "docx", "xls", "xlsx",
    "ppt", "pptx", "psd", "iso", "heic", "tif", "tiff",
];

impl Rag {
    /// `None` when semantic search is disabled.
    pub fn from_config(
        cfg: &crate::config::Config,
        latex: Option<Arc<crate::latex::LatexClient>>,
    ) -> Option<Arc<Rag>> {
        let name = match cfg.rag_model.as_str() {
            "off" | "" | "none" => return None,
            "auto" if cfg!(feature = "embed") => DEFAULT_MODEL.to_string(),
            "auto" => return None,
            other => other.to_string(),
        };
        Some(Arc::new(Rag {
            id: model_id(&name),
            model_name: name,
            model_dir: cfg.rag_model_dir.clone(),
            threads: cfg.rag_threads,
            max_bytes: cfg.rag_max_file_bytes,
            max_pdf_bytes: cfg.rag_max_pdf_bytes,
            idle_unload: (cfg.rag_idle_unload_secs > 0)
                .then(|| Duration::from_secs(cfg.rag_idle_unload_secs)),
            latex,
            embedder: RwLock::new(None),
            load_lock: tokio::sync::Mutex::new(()),
            last_used: std::sync::Mutex::new(Instant::now()),
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

    /// The loaded model, loading it first if it was unloaded while idle.
    async fn ensure_embedder(&self) -> anyhow::Result<Arc<dyn Embedder>> {
        *self.last_used.lock().unwrap() = Instant::now();
        if let Some(e) = self.embedder() {
            return Ok(e);
        }
        let _g = self.load_lock.lock().await;
        if let Some(e) = self.embedder() {
            return Ok(e);
        }
        self.load_model().await?;
        tracing::debug!(model = %self.model_name, "embedding model loaded");
        self.embedder().ok_or_else(|| anyhow::anyhow!("模型未載入"))
    }

    /// Drop the model if it has not been used for `idle_unload`.
    fn maybe_unload(&self) {
        let Some(idle) = self.idle_unload else { return };
        if self.last_used.lock().unwrap().elapsed() < idle {
            return;
        }
        if self.embedder.write().unwrap().take().is_some() {
            tracing::debug!(model = %self.model_name, "embedding model unloaded (idle)");
            trim_heap();
        }
    }

    /// Load the model (blocking) — separate so tests can call it synchronously.
    pub async fn load_model(&self) -> anyhow::Result<()> {
        let name = self.model_name.clone();
        let dir = self.model_dir.clone();
        let threads = self.threads;
        let e: Arc<dyn Embedder> =
            tokio::task::spawn_blocking(move || -> anyhow::Result<Arc<dyn Embedder>> {
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
            *rag.last_used.lock().unwrap() = Instant::now();
            tracing::info!(model = %rag.model_name, "semantic search ready");
            loop {
                rag.set_state("indexing", None);
                match rag.index_pending(&db, &store).await {
                    Ok(n) if n > 0 => tracing::info!(files = n, "indexed for semantic search"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "indexing pass failed"),
                }
                rag.set_state("ready", None);
                rag.maybe_unload();
                let _ = tokio::time::timeout(Duration::from_secs(30), rag.wake.notified()).await;
                // Debounce bursts (folder uploads).
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }

    fn model_id(&self) -> String {
        self.id.clone()
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
            map.entry(node).or_default().push(Entry {
                chunk_id: id,
                vec: bytes_to_vec(&blob),
            });
        }
        *self.index.write().unwrap() = map;
        Ok(())
    }

    async fn record(
        &self,
        db: &SqlitePool,
        n: &Node,
        status: &str,
        detail: &str,
    ) -> anyhow::Result<()> {
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
        sqlx::query("DELETE FROM rag_chunks WHERE node_id = ?")
            .bind(&n.id)
            .execute(db)
            .await?;
        self.index.write().unwrap().remove(&n.id);
        Ok(())
    }

    /// Index every file whose content (or the model) changed. Returns files processed.
    pub async fn index_pending(&self, db: &SqlitePool, store: &Store) -> anyhow::Result<usize> {
        let model = self.model_id();
        // Files that became sensitive are dropped from the index.
        sqlx::query(
            "DELETE FROM rag_chunks WHERE node_id IN (SELECT id FROM nodes WHERE sensitive = 1)",
        )
        .execute(db)
        .await?;
        sqlx::query(
            "DELETE FROM rag_docs WHERE node_id IN (SELECT id FROM nodes WHERE sensitive = 1)",
        )
        .execute(db)
        .await?;
        // …and so are items in the recycle bin (re-indexed when restored).
        sqlx::query(const_format_trash!(
            "DELETE FROM rag_chunks WHERE node_id IN (SELECT id FROM t)"
        ))
        .execute(db)
        .await?;
        sqlx::query(const_format_trash!(
            "DELETE FROM rag_docs WHERE node_id IN (SELECT id FROM t)"
        ))
        .execute(db)
        .await?;
        // Drop in-memory entries for deleted / sensitive / stale docs.
        let live: HashSet<String> = sqlx::query_scalar(
            "SELECT node_id FROM rag_docs WHERE status = 'indexed' AND model = ?",
        )
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
            // Only now (something to do) is the model loaded, if it was idle.
            let embedder = self.ensure_embedder().await?;
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

    async fn index_node(
        &self,
        db: &SqlitePool,
        store: &Store,
        embedder: &Arc<dyn Embedder>,
        n: &Node,
    ) -> anyhow::Result<()> {
        let ext = n
            .name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        let mime = n.mime.as_deref().unwrap_or("");
        let chunks = if is_pdf(n) {
            match self.pdf_chunks(store, n).await? {
                Ok(c) => c,
                Err(reason) => return self.record(db, n, "skipped", &reason).await,
            }
        } else {
            if TEXT_EXT_DENY.contains(&ext.as_str())
                || mime.starts_with("image/")
                || mime.starts_with("audio/")
                || mime.starts_with("video/")
            {
                return self.record(db, n, "skipped", "非文字檔").await;
            }
            if n.size as u64 > self.max_bytes {
                return self.record(db, n, "skipped", "超過索引大小上限").await;
            }
            let buf = read_blob(store, n, self.max_bytes).await?;
            if textview::looks_binary(&buf) {
                return self.record(db, n, "skipped", "二進位內容").await;
            }
            let (text, _, _) = textview::detect(&buf).decode(&buf);
            chunk_text(&text, CHUNK_CHARS)
        };
        if chunks.is_empty() {
            return self.record(db, n, "skipped", "沒有文字內容").await;
        }
        let path = nodes::path_of(db, &n.id, nodes::ROOT_ID)
            .await
            .map_err(|e| anyhow::anyhow!(e.message))?;
        let inputs: Vec<String> = chunks
            .iter()
            .map(|c| format!("{path}\n{}", c.text))
            .collect();
        let e = embedder.clone();
        let vecs = tokio::task::spawn_blocking(move || e.embed(&inputs, false)).await??;

        let mut tx = db.begin().await?;
        sqlx::query("DELETE FROM rag_chunks WHERE node_id = ?")
            .bind(&n.id)
            .execute(&mut *tx)
            .await?;
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
            entries.push(Entry {
                chunk_id: id,
                vec: v,
            });
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

    /// Extract and chunk a PDF. `Ok(Err(reason))` = skip this document.
    async fn pdf_chunks(
        &self,
        store: &Store,
        n: &Node,
    ) -> anyhow::Result<Result<Vec<Chunk>, String>> {
        let Some(latex) = &self.latex else {
            return Ok(Err("未設定 PDF 擷取服務（MISTBOX_LATEX_URL）".into()));
        };
        if n.size as u64 > self.max_pdf_bytes {
            return Ok(Err("PDF 超過索引大小上限".into()));
        }
        let data = read_blob(store, n, self.max_pdf_bytes).await?;
        let text = match latex.pdf_text(data).await? {
            crate::latex::PdfText::Text(t) => t,
            crate::latex::PdfText::Unreadable(why) => return Ok(Err(why)),
        };
        let text: String = text.chars().take(MAX_PDF_TEXT_CHARS).collect();
        if text
            .trim_matches(|c: char| c.is_whitespace() || c == '\x0c')
            .is_empty()
        {
            return Ok(Err("PDF 沒有可擷取的文字（可能是掃描影像）".into()));
        }
        Ok(Ok(chunk_pages(&text, CHUNK_CHARS)))
    }

    /// Top-`k` chunks for `query` that the principal may read (max 3 per file).
    pub async fn search(
        &self,
        state: &crate::AppState,
        p: &Principal,
        query: &str,
        k: usize,
    ) -> anyhow::Result<Vec<Hit>> {
        if self.embedder().is_none() && self.state.read().unwrap().0 == "loading" {
            anyhow::bail!("語意搜尋模型尚未就緒");
        }
        let embedder = self.ensure_embedder().await?;
        let q = vec![query.to_string()];
        let e = embedder.clone();
        let qv = tokio::task::spawn_blocking(move || e.embed(&q, true))
            .await??
            .remove(0);

        let mut scored: Vec<(f32, String, i64)> = {
            let idx = self.index.read().unwrap();
            idx.iter()
                .flat_map(|(node, es)| {
                    let qv = &qv;
                    es.iter()
                        .map(move |e| (dot(qv, &e.vec), node.clone(), e.chunk_id))
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
            let Some(n) = allowed[&node_id].clone() else {
                continue;
            };
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
            let path = nodes::path_of(&state.db, &n.id, p.root_id())
                .await
                .map_err(|e| anyhow::anyhow!(e.message))?;
            let unit = if is_pdf(&n) { "page" } else { "line" };
            out.push(Hit {
                node: NodeView::from_node(&n, p.root_id()).with_path(path),
                score,
                seq,
                line_start: ls,
                line_end: le,
                unit,
                text,
            });
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
        let desc = MODELS
            .iter()
            .find(|m| m.0 == self.model_name)
            .map(|m| m.1)
            .unwrap_or("詞彙雜湊（測試用，非語意）");
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
            model_loaded: self.embedder().is_some(),
            pdf: self.latex.is_some(),
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

async fn read_blob(store: &Store, n: &Node, limit: u64) -> anyhow::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity((n.size as u64).min(limit) as usize);
    tokio::fs::File::open(store.blob_path(n.blob_id.as_deref().unwrap_or_default()))
        .await?
        .take(limit)
        .read_to_end(&mut buf)
        .await?;
    Ok(buf)
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
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_keep_line_ranges() {
        let text: String = (1..=100)
            .map(|i| format!("第 {i} 行的內容 line {i}\n"))
            .collect();
        let cs = chunk_text(&text, 200);
        assert!(cs.len() > 5);
        assert_eq!(cs[0].line_start, 1);
        assert_eq!(cs.last().unwrap().line_end, 100);
        for w in cs.windows(2) {
            assert!(
                w[1].line_start <= w[0].line_end + 1,
                "contiguous / overlapping"
            );
        }
        assert!(chunk_text("  \n\n ", 100).is_empty());
        let long = "字".repeat(1000);
        let cs = chunk_text(&long, 100);
        assert!(cs.iter().all(|c| c.text.chars().count() <= 250));
    }

    #[test]
    fn hash_embedder_prefers_overlap() {
        let e = HashEmbedder;
        let v = e
            .embed(
                &[
                    "雲端硬碟 備份".into(),
                    "貓咪 在 墊子 上".into(),
                    "如何備份雲端硬碟".into(),
                ],
                false,
            )
            .unwrap();
        assert!(dot(&v[2], &v[0]) > dot(&v[2], &v[1]));
    }

    #[test]
    fn pdf_pages_become_chunk_locations() {
        let cs = chunk_pages("第一頁 hello\n\x0c\x0c第三頁 world\n\x0c", 100);
        assert_eq!(cs.len(), 2);
        assert_eq!((cs[0].line_start, cs[0].line_end), (1, 1));
        assert_eq!((cs[1].line_start, cs[1].line_end), (3, 3));
        assert!(cs[1].text.contains("第三頁"));
    }

    #[test]
    fn vec_roundtrip() {
        let v = vec![0.5f32, -1.25, 3.0];
        assert_eq!(bytes_to_vec(&vec_to_bytes(&v)), v);
    }
}
