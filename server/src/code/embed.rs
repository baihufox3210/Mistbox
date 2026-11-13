//! Code embeddings for "similar code" search (diagnose / optimise mode).
//!
//! Model: jina-embeddings-v2-base-code, int8 ONNX (≈160 MB), 768 dims, mean
//! pooling, byte-level BPE tokenizer. Runs on the same ONNX Runtime as the
//! text model (portable build: picks SSE / AVX / AVX2 at run time, so CPUs
//! without AVX2 work). Loaded on first use, unloaded when idle.
//!
//! Files are cut into chunks per function / method / PIO program (long ones
//! split into windows); code outside functions is windowed. Vectors are
//! stored as int8 (768 bytes each) and kept in memory for brute-force search.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};

use serde::Serialize;
use sqlx::SqlitePool;

use super::Lang;
use crate::rag::{Embedder, HashEmbedder};

/// Model name accepted by `MISTBOX_CODE_MODEL` / `rag download-model`.
pub const CODE_MODEL: &str = "jina-code-q8";
/// Chunking version: part of the stored model id, so changes re-embed.
const CHUNKER: &str = "f1";
const WINDOW: usize = 40;
const MAX_FUNC_LINES: usize = 80;
const MAX_CHUNK_CHARS: usize = 2400;
const MAX_CHUNKS_PER_FILE: usize = 300;

#[cfg(feature = "embed")]
pub(crate) mod model {
    use std::{path::Path, sync::Mutex};

    use hf_hub::{Cache, Repo, RepoType};
    use ort::{
        session::{Session, builder::GraphOptimizationLevel},
        value::Tensor,
    };

    use crate::rag::fast::{cpu, ort_env, sha256_file};

    const REPO: &str = "jinaai/jina-embeddings-v2-base-code";
    const REV: &str = "516f4baf13dec4ddddda8631e019b5737c8bc250";
    const FILES: [(&str, &str); 2] = [
        (
            "onnx/model_quantized.onnx",
            "ed45870251c9f0cf656e78aab0d37a23489066df8a222bb1c8caf8a45f2cb16d",
        ),
        (
            "tokenizer.json",
            "b01c78a902aa4facb2f47f95449f48e2f7bbfea5d2472ee2f6ce92323c6f86e5",
        ),
    ];
    /// Window incl. <s> and </s> (the model supports 8192; 512 keeps memory low).
    const MAX_TOKENS: usize = 512;

    fn repo() -> Repo {
        Repo::with_revision(REPO.into(), RepoType::Model, REV.into())
    }

    /// Fetch and verify the model files (image build time).
    pub fn download(dir: &Path) -> anyhow::Result<()> {
        let api = hf_hub::api::sync::ApiBuilder::new()
            .with_cache_dir(dir.to_path_buf())
            .with_progress(false)
            .build()?;
        let r = api.repo(repo());
        for (f, want) in FILES {
            let got = sha256_file(&r.get(f)?)?;
            anyhow::ensure!(got == want, "模型檔 {f} 的 SHA-256 不符（{got}），已中止");
        }
        Ok(())
    }

    pub struct CodeModel {
        inner: Mutex<(tokenizers::Tokenizer, Session)>,
    }

    impl CodeModel {
        pub fn load(dir: &Path, threads: usize) -> anyhow::Result<Self> {
            let cache = Cache::new(dir.to_path_buf());
            let r = cache.repo(repo());
            let path = |f: &str| {
                r.get(f).ok_or_else(|| anyhow::anyhow!("找不到程式碼模型檔 {f}（請先執行 `mistbox rag download-model jina-code-q8`）"))
            };
            let tok = tokenizers::Tokenizer::from_file(path(FILES[1].0)?)
                .map_err(|e| anyhow::anyhow!("載入分詞器失敗：{e}"))?;
            let b = |e: ort::Error<ort::session::builder::SessionBuilder>| {
                anyhow::anyhow!(e.to_string())
            };
            let session = Session::builder(&ort_env()?)?
                .with_execution_providers([cpu()])
                .map_err(b)?
                .with_optimization_level(GraphOptimizationLevel::Level3)
                .map_err(b)?
                .with_intra_threads(threads.max(1))
                .map_err(b)?
                .with_inter_threads(1)
                .map_err(b)?
                .commit_from_file(path(FILES[0].0)?)?;
            Ok(Self {
                inner: Mutex::new((tok, session)),
            })
        }

        /// One text → mean-pooled, L2-normalised vector (texts run one at a time:
        /// dynamic quantisation must not depend on batch neighbours).
        fn embed_one(
            tok: &tokenizers::Tokenizer,
            session: &mut Session,
            text: &str,
        ) -> anyhow::Result<Vec<f32>> {
            let enc = tok
                .encode(text, true)
                .map_err(|e| anyhow::anyhow!("分詞失敗：{e}"))?;
            let mut ids: Vec<i64> = enc.get_ids().iter().map(|&i| i as i64).collect();
            if ids.len() > MAX_TOKENS {
                // Keep <s> … and the closing </s>.
                let last = *ids.last().unwrap();
                ids.truncate(MAX_TOKENS - 1);
                ids.push(last);
            }
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
            for t in data.chunks_exact(dim) {
                v.iter_mut().zip(t).for_each(|(a, b)| *a += b);
            }
            crate::rag::normalise(&mut v);
            Ok(v)
        }
    }

    impl crate::rag::Embedder for CodeModel {
        fn id(&self) -> &str {
            super::CODE_MODEL
        }
        fn embed(&self, texts: &[String], _query: bool) -> anyhow::Result<Vec<Vec<f32>>> {
            let mut g = self.inner.lock().unwrap();
            let (tok, session) = &mut *g;
            texts
                .iter()
                .map(|t| Self::embed_one(tok, session, t))
                .collect()
        }
    }
}

/// Download the code model (CLI, image build).
pub fn download(dir: &std::path::Path) -> anyhow::Result<()> {
    #[cfg(feature = "embed")]
    {
        model::download(dir)?;
        let m = model::CodeModel::load(dir, 1)?;
        let v = m.embed(&["int main(void) { return 0; }".to_string()], false)?;
        eprintln!(
            "程式碼模型 {CODE_MODEL} 已就緒（{} 維）：{}",
            v[0].len(),
            dir.display()
        );
        Ok(())
    }
    #[cfg(not(feature = "embed"))]
    {
        let _ = dir;
        anyhow::bail!("此版本未以 `embed` feature 編譯，無法下載模型")
    }
}

// ------------------------------------------------------------------ chunking

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub line_start: u32,
    pub line_end: u32,
    pub symbol: Option<String>,
    pub text: String,
}

fn chunk_text(
    name: &str,
    lines: &[&str],
    start: u32,
    end: u32,
    symbol: Option<&str>,
) -> Option<Chunk> {
    let body: Vec<&str> = lines[(start - 1) as usize..(end as usize).min(lines.len())].to_vec();
    if body.iter().filter(|l| !l.trim().is_empty()).count() < 2 {
        return None;
    }
    let mut text = format!(
        "// {name}{}\n",
        symbol.map(|s| format!(" · {s}")).unwrap_or_default()
    );
    for l in body {
        if text.len() + l.len() > MAX_CHUNK_CHARS {
            break;
        }
        text.push_str(l);
        text.push('\n');
    }
    Some(Chunk {
        line_start: start,
        line_end: end,
        symbol: symbol.map(str::to_string),
        text,
    })
}

/// Chunks for one file: functions / methods / PIO programs (long ones split),
/// plus windows over the code outside them.
pub fn chunks(name: &str, text: &str, symbols: &[(String, String, u32, u32)]) -> Vec<Chunk> {
    let lines: Vec<&str> = text.lines().collect();
    let n = lines.len() as u32;
    if n == 0 {
        return Vec::new();
    }
    let mut covered = vec![false; n as usize + 1];
    let mut out = Vec::new();
    let mut funcs: Vec<&(String, String, u32, u32)> = symbols
        .iter()
        .filter(|(_, kind, s, e)| {
            matches!(kind.as_str(), "function" | "method" | "pio_program")
                && *s >= 1
                && *e >= *s
                && *e <= n
        })
        .collect();
    funcs.sort_by_key(|s| (s.2, std::cmp::Reverse(s.3)));
    for (sym, _, s, e) in funcs {
        // Nested definitions already inside a chunked function are skipped.
        if (*s..=*e).all(|l| covered[l as usize]) {
            continue;
        }
        let mut a = *s;
        while a <= *e {
            let b = if (*e - a) as usize + 1 > MAX_FUNC_LINES {
                a + WINDOW as u32 - 1
            } else {
                *e
            };
            if let Some(c) = chunk_text(name, &lines, a, b, Some(sym)) {
                out.push(c);
            }
            for l in a..=b {
                covered[l as usize] = true;
            }
            if b == *e {
                break;
            }
            a = b - 4; // small overlap
        }
    }
    // Windows over uncovered code.
    let mut l = 1u32;
    while l <= n {
        if covered[l as usize] {
            l += 1;
            continue;
        }
        let start = l;
        let mut end = l;
        while end < n && !covered[end as usize + 1] && (end - start + 1) < WINDOW as u32 {
            end += 1;
        }
        if let Some(c) = chunk_text(name, &lines, start, end, None) {
            out.push(c);
        }
        l = end + 1;
    }
    out.sort_by_key(|c| c.line_start);
    out.truncate(MAX_CHUNKS_PER_FILE);
    out
}

// ------------------------------------------------------------------ vectors

fn quantise(v: &[f32]) -> Vec<u8> {
    v.iter()
        .map(|x| ((x * 127.0).round().clamp(-127.0, 127.0) as i8) as u8)
        .collect()
}

fn dot_q(q: &[f32], v: &[u8]) -> f32 {
    q.iter()
        .zip(v)
        .map(|(a, b)| a * (*b as i8) as f32)
        .sum::<f32>()
        / 127.0
}

/// (node_id, line_start, line_end, symbol, int8 vector)
type VecRow = (String, i64, i64, Option<String>, Vec<u8>);

struct Entry {
    node_id: Arc<str>,
    line_start: u32,
    line_end: u32,
    symbol: Option<Arc<str>>,
    vec: Box<[u8]>,
}

#[derive(Serialize, Debug, Clone)]
pub struct Hit {
    pub node_id: String,
    pub line_start: u32,
    pub line_end: u32,
    pub symbol: Option<String>,
    pub score: f32,
}

/// The code-embedding index: lazy model, in-memory int8 vectors.
pub struct CodeSemantic {
    name: String,
    dir: std::path::PathBuf,
    threads: usize,
    idle: Option<Duration>,
    model: RwLock<Option<Arc<dyn Embedder>>>,
    load_lock: tokio::sync::Mutex<()>,
    last_used: Mutex<Instant>,
    index: RwLock<HashMap<Arc<str>, Vec<Entry>>>,
    loaded: std::sync::atomic::AtomicBool,
    error: RwLock<Option<String>>,
}

#[derive(Serialize, Debug)]
pub struct SemanticStatus {
    pub model: String,
    pub model_loaded: bool,
    pub files: usize,
    pub chunks: usize,
    pub error: Option<String>,
}

impl CodeSemantic {
    /// `None` when disabled (`off`, or `auto` without the `embed` feature).
    pub fn from_config(cfg: &crate::config::Config) -> Option<Arc<Self>> {
        let name = match cfg.code_model.as_str() {
            "off" | "" | "none" => return None,
            "auto" if cfg!(feature = "embed") => CODE_MODEL.to_string(),
            "auto" => return None,
            other => other.to_string(),
        };
        if name != "hash" && !cfg!(feature = "embed") {
            tracing::warn!(model = %name, "程式碼模型需要 embed feature；相似程式碼搜尋停用");
            return None;
        }
        Some(Arc::new(Self {
            name,
            dir: cfg.rag_model_dir.clone(),
            threads: cfg.rag_threads,
            idle: (cfg.rag_idle_unload_secs > 0)
                .then(|| Duration::from_secs(cfg.rag_idle_unload_secs)),
            model: RwLock::new(None),
            load_lock: tokio::sync::Mutex::new(()),
            last_used: Mutex::new(Instant::now()),
            index: RwLock::new(HashMap::new()),
            loaded: false.into(),
            error: RwLock::new(None),
        }))
    }

    fn model_id(&self) -> String {
        format!("{}@{CHUNKER}", self.name)
    }

    async fn ensure_model(&self) -> anyhow::Result<Arc<dyn Embedder>> {
        *self.last_used.lock().unwrap() = Instant::now();
        if let Some(m) = self.model.read().unwrap().clone() {
            return Ok(m);
        }
        let _g = self.load_lock.lock().await;
        if let Some(m) = self.model.read().unwrap().clone() {
            return Ok(m);
        }
        let (name, dir, threads) = (self.name.clone(), self.dir.clone(), self.threads);
        let m: Arc<dyn Embedder> =
            tokio::task::spawn_blocking(move || -> anyhow::Result<Arc<dyn Embedder>> {
                if name == "hash" {
                    return Ok(Arc::new(HashEmbedder));
                }
                #[cfg(feature = "embed")]
                {
                    anyhow::ensure!(name == CODE_MODEL, "未知的程式碼模型 {name}");
                    Ok(Arc::new(model::CodeModel::load(&dir, threads)?))
                }
                #[cfg(not(feature = "embed"))]
                {
                    let _ = (dir, threads);
                    anyhow::bail!("程式碼模型 {name} 需要以 `embed` feature 編譯")
                }
            })
            .await??;
        *self.model.write().unwrap() = Some(m.clone());
        tracing::debug!(model = %self.name, "code embedding model loaded");
        Ok(m)
    }

    /// Drop the model after `idle` without use.
    pub fn maybe_unload(&self) {
        let Some(idle) = self.idle else { return };
        if self.last_used.lock().unwrap().elapsed() < idle {
            return;
        }
        if self.model.write().unwrap().take().is_some() {
            tracing::debug!(model = %self.name, "code embedding model unloaded (idle)");
            crate::rag::trim_heap();
        }
    }

    async fn load_index(&self, db: &SqlitePool) -> anyhow::Result<()> {
        let rows: Vec<VecRow> = sqlx::query_as(
            "SELECT v.node_id, v.line_start, v.line_end, v.symbol, v.vec FROM code_vecs v
             JOIN code_vec_docs d ON d.node_id = v.node_id WHERE d.model = ?",
        )
        .bind(self.model_id())
        .fetch_all(db)
        .await?;
        let mut idx: HashMap<Arc<str>, Vec<Entry>> = HashMap::new();
        let mut keys: HashMap<String, Arc<str>> = HashMap::new();
        for (node, a, b, sym, vec) in rows {
            let key = keys
                .entry(node)
                .or_insert_with_key(|k| k.as_str().into())
                .clone();
            idx.entry(key.clone()).or_default().push(Entry {
                node_id: key,
                line_start: a as u32,
                line_end: b as u32,
                symbol: sym.map(|s| s.as_str().into()),
                vec: vec.into_boxed_slice(),
            });
        }
        *self.index.write().unwrap() = idx;
        self.loaded
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    /// Embed every analysed source file whose content changed. Returns files processed.
    pub async fn index_pending(&self, db: &SqlitePool) -> anyhow::Result<usize> {
        if !self.loaded.load(std::sync::atomic::Ordering::Relaxed) {
            self.load_index(db).await?;
        }
        let model_id = self.model_id();
        // Files no longer analysed (deleted, sensitive, trashed, re-analysis pending) drop out.
        sqlx::query(
            "DELETE FROM code_vec_docs WHERE node_id NOT IN
               (SELECT node_id FROM code_files WHERE status = 'indexed' AND lang NOT IN ('markdown'))
             OR model <> ?",
        )
        .bind(&model_id)
        .execute(db)
        .await?;
        sqlx::query(
            "DELETE FROM code_vecs WHERE node_id NOT IN (SELECT node_id FROM code_vec_docs)",
        )
        .execute(db)
        .await?;
        let live: std::collections::HashSet<String> =
            sqlx::query_scalar("SELECT node_id FROM code_vec_docs")
                .fetch_all(db)
                .await?
                .into_iter()
                .collect();
        self.index
            .write()
            .unwrap()
            .retain(|k, _| live.contains(&**k));

        let mut done = 0;
        loop {
            let batch: Vec<(String, String, String, String)> = sqlx::query_as(
                "SELECT c.node_id, c.sha256, n.name, c.lang FROM code_files c JOIN nodes n ON n.id = c.node_id
                 LEFT JOIN code_vec_docs d ON d.node_id = c.node_id
                 WHERE c.status = 'indexed' AND c.lang IS NOT NULL AND c.lang <> 'markdown'
                   AND (d.node_id IS NULL OR d.sha256 <> c.sha256)
                 LIMIT 20",
            )
            .fetch_all(db)
            .await?;
            if batch.is_empty() {
                break;
            }
            let model = match self.ensure_model().await {
                Ok(m) => m,
                Err(e) => {
                    *self.error.write().unwrap() = Some(e.to_string());
                    return Err(e);
                }
            };
            *self.error.write().unwrap() = None;
            for (node_id, sha, name, _lang) in batch {
                let text: Option<String> =
                    sqlx::query_scalar("SELECT body FROM code_text WHERE node_id = ?")
                        .bind(&node_id)
                        .fetch_optional(db)
                        .await?;
                let symbols: Vec<(String, String, i64, i64)> = sqlx::query_as(
                    "SELECT name, kind, line_start, line_end FROM code_symbols WHERE node_id = ?",
                )
                .bind(&node_id)
                .fetch_all(db)
                .await?;
                let symbols: Vec<_> = symbols
                    .into_iter()
                    .map(|(a, b, c, d)| (a, b, c as u32, d as u32))
                    .collect();
                let cs = chunks(&name, text.as_deref().unwrap_or(""), &symbols);
                let texts: Vec<String> = cs.iter().map(|c| c.text.clone()).collect();
                let m = model.clone();
                let vecs = tokio::task::spawn_blocking(move || m.embed(&texts, false)).await??;
                *self.last_used.lock().unwrap() = Instant::now();

                let mut tx = db.begin().await?;
                sqlx::query("DELETE FROM code_vecs WHERE node_id = ?")
                    .bind(&node_id)
                    .execute(&mut *tx)
                    .await?;
                let key: Arc<str> = node_id.as_str().into();
                let mut entries = Vec::with_capacity(cs.len());
                for (c, v) in cs.iter().zip(&vecs) {
                    let q = quantise(v);
                    sqlx::query("INSERT INTO code_vecs (node_id, line_start, line_end, symbol, vec) VALUES (?, ?, ?, ?, ?)")
                        .bind(&node_id)
                        .bind(c.line_start)
                        .bind(c.line_end)
                        .bind(&c.symbol)
                        .bind(&q)
                        .execute(&mut *tx)
                        .await?;
                    entries.push(Entry {
                        node_id: key.clone(),
                        line_start: c.line_start,
                        line_end: c.line_end,
                        symbol: c.symbol.as_deref().map(Into::into),
                        vec: q.into_boxed_slice(),
                    });
                }
                sqlx::query(
                    "INSERT INTO code_vec_docs (node_id, sha256, model, chunks, indexed_at) VALUES (?, ?, ?, ?, ?)
                     ON CONFLICT(node_id) DO UPDATE SET sha256 = excluded.sha256, model = excluded.model,
                       chunks = excluded.chunks, indexed_at = excluded.indexed_at",
                )
                .bind(&node_id)
                .bind(&sha)
                .bind(&model_id)
                .bind(cs.len() as i64)
                .bind(crate::util::now())
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                self.index.write().unwrap().insert(key, entries);
                done += 1;
            }
        }
        Ok(done)
    }

    /// Chunks most similar to `query` among files accepted by `allow`.
    pub async fn search(
        &self,
        query: &str,
        k: usize,
        allow: &(dyn Fn(&str) -> bool + Sync),
    ) -> anyhow::Result<Vec<Hit>> {
        if self.index.read().unwrap().is_empty() {
            return Ok(Vec::new());
        }
        let model = self.ensure_model().await?;
        let q = query.chars().take(MAX_CHUNK_CHARS).collect::<String>();
        let qv = tokio::task::spawn_blocking(move || model.embed(&[q], true))
            .await??
            .remove(0);
        let idx = self.index.read().unwrap();
        let mut hits: Vec<Hit> = Vec::new();
        for (node, entries) in idx.iter() {
            if !allow(node) {
                continue;
            }
            for e in entries {
                let score = dot_q(&qv, &e.vec);
                hits.push(Hit {
                    node_id: e.node_id.to_string(),
                    line_start: e.line_start,
                    line_end: e.line_end,
                    symbol: e.symbol.as_deref().map(str::to_string),
                    score,
                });
            }
        }
        hits.sort_by(|a, b| b.score.total_cmp(&a.score));
        hits.truncate(k);
        Ok(hits)
    }

    pub fn status(&self) -> SemanticStatus {
        let idx = self.index.read().unwrap();
        SemanticStatus {
            model: self.name.clone(),
            model_loaded: self.model.read().unwrap().is_some(),
            files: idx.len(),
            chunks: idx.values().map(Vec::len).sum(),
            error: self.error.read().unwrap().clone(),
        }
    }
}

/// Languages worth embedding (everything analysed except notes).
pub fn embeddable(lang: Lang) -> bool {
    lang != Lang::Markdown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_follow_functions_and_window_the_rest() {
        let mut src = String::from("#include <stdio.h>\nstatic int g;\n\n");
        src.push_str("int add(int a, int b) {\n    return a + b;\n}\n\n"); // 4-6
        for i in 0..100 {
            src.push_str(&format!("int v{i} = {i};\n")); // 8-107
        }
        src.push_str("void big(void) {\n"); // 108
        for i in 0..120 {
            src.push_str(&format!("    x += {i};\n"));
        }
        src.push_str("}\n"); // 229
        let syms = vec![
            ("add".to_string(), "function".to_string(), 4, 6),
            ("big".to_string(), "function".to_string(), 108, 229),
        ];
        let cs = chunks("a.c", &src, &syms);
        let add = cs
            .iter()
            .find(|c| c.symbol.as_deref() == Some("add"))
            .unwrap();
        assert_eq!((add.line_start, add.line_end), (4, 6));
        assert!(add.text.starts_with("// a.c · add\n"));
        let big: Vec<_> = cs
            .iter()
            .filter(|c| c.symbol.as_deref() == Some("big"))
            .collect();
        assert!(big.len() >= 3, "long function is split: {}", big.len());
        assert_eq!(big.last().unwrap().line_end, 229);
        // Top-level code before `add` and between the functions is windowed.
        assert!(cs.iter().any(|c| c.symbol.is_none() && c.line_start == 1));
        assert!(cs.iter().any(|c| c.symbol.is_none() && c.line_start == 7));
        assert!(cs.iter().all(|c| c.text.len() <= MAX_CHUNK_CHARS + 40));
    }

    #[test]
    fn int8_vectors_keep_ranking() {
        let a = [0.6f32, 0.8, 0.0];
        let b = [0.0f32, 0.6, 0.8];
        let qa = quantise(&a);
        let qb = quantise(&b);
        assert!((dot_q(&a, &qa) - 1.0).abs() < 0.02);
        assert!(dot_q(&a, &qa) > dot_q(&a, &qb));
    }
}

/// Quality / memory probe with the real model:
/// `MISTBOX_TEST_CODE_MODEL_DIR=/path cargo test --release --features embed code_model_probe -- --nocapture`.
#[cfg(all(test, feature = "embed"))]
mod probe {
    use super::*;

    fn rss_mib() -> u64 {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("VmRSS:"))
                    .map(|l| l.split_whitespace().nth(1).unwrap().parse::<u64>().unwrap() / 1024)
            })
            .unwrap_or(0)
    }

    #[test]
    fn code_model_probe() {
        let Ok(dir) = std::env::var("MISTBOX_TEST_CODE_MODEL_DIR") else {
            return;
        };
        let before = rss_mib();
        let t = Instant::now();
        let m = model::CodeModel::load(std::path::Path::new(&dir), 2).unwrap();
        println!("load {:?}, RSS {} → {} MiB", t.elapsed(), before, rss_mib());
        let docs = [
            (
                "dma",
                "void send(uint32_t *buf, int n) {\n  int ch = dma_claim_unused_channel(true);\n  dma_channel_config c = dma_channel_get_default_config(ch);\n  channel_config_set_dreq(&c, pio_get_dreq(pio0, sm, true));\n  dma_channel_configure(ch, &c, &pio0->txf[sm], buf, n, true);\n}",
            ),
            (
                "dma-renamed",
                "static void push_pixels(uint32_t *px, int count) {\n  int chan = dma_claim_unused_channel(true);\n  dma_channel_config cfg = dma_channel_get_default_config(chan);\n  channel_config_set_dreq(&cfg, pio_get_dreq(pio1, sm, true));\n  dma_channel_configure(chan, &cfg, &pio1->txf[sm], px, count, true);\n}",
            ),
            (
                "ros",
                "sub_ = create_subscription<sensor_msgs::msg::LaserScan>(\"scan\", 10,\n  [this](sensor_msgs::msg::LaserScan::ConstSharedPtr msg) { last_ = msg->ranges[0]; });",
            ),
            (
                "omp",
                "double total = 0;\n#pragma omp parallel for reduction(+:total)\nfor (int i = 0; i < n; i++) total += a[i] * b[i];",
            ),
            (
                "bits",
                "static inline bool bit_is_set(uint32_t reg, unsigned n) { return (reg >> n) & 1u; }\nreg |= 1u << 5;\nreg &= ~(1u << 3);",
            ),
        ];
        let texts: Vec<String> = docs.iter().map(|d| d.1.to_string()).collect();
        let t = Instant::now();
        let dv = m.embed(&texts, false).unwrap();
        println!("embed 5 chunks {:?}, RSS {} MiB", t.elapsed(), rss_mib());
        let queries = [
            "DMA transfer from buffer into PIO TX FIFO",
            "subscribe to laser scan topic in ROS 2",
            "parallel dot product with OpenMP",
            "set and clear a bit in a register",
        ];
        for q in queries {
            let qv = m.embed(&[q.to_string()], true).unwrap().remove(0);
            let mut s: Vec<(f32, &str)> = dv
                .iter()
                .zip(&docs)
                .map(|(v, d)| (v.iter().zip(&qv).map(|(a, b)| a * b).sum(), d.0))
                .collect();
            s.sort_by(|a, b| b.0.total_cmp(&a.0));
            println!(
                "{q:45} → {:?}",
                s.iter()
                    .map(|(x, n)| format!("{n}:{x:.2}"))
                    .collect::<Vec<_>>()
            );
        }
        let sim: f32 = dv[0].iter().zip(&dv[1]).map(|(a, b)| a * b).sum();
        let other: f32 = dv[0].iter().zip(&dv[3]).map(|(a, b)| a * b).sum();
        println!(
            "renamed copy similarity {sim:.3}, unrelated {other:.3}, final RSS {} MiB",
            rss_mib()
        );
    }
}
