//! Mistbox — a single-user private cloud drive.
//!
//! Module map:
//! - `config`    runtime configuration (env / CLI)
//! - `db`        SQLite pool, migrations, data-dir layout
//! - `storage`   blob store: streaming writes, temp files, atomic commit
//! - `nodes`     logical directory tree (names, paths, moves, deletes, search)
//! - `auth`      passwords, sessions, CSRF, API keys, rate limiting
//! - `authz`     per-request permission checks for sessions and API keys
//! - `sensitive` detection of secrets (.env, private keys, certificates)
//! - `textview`  encoding-aware text preview
//! - `latex`     resource collection + client for the isolated LaTeX worker
//! - `rag`       local-embedding semantic search (indexer + vector search)
//! - `code`      code index: symbols, static-analysis findings, duplicate code
//! - `api`       HTTP handlers (`/api/v1`)
//! - `import`    read-only source folder import (CLI)
//! - `backup`    consistent online backup / restore / verify (CLI)

pub mod api;
pub mod auth;
pub mod authz;
pub mod backup;
pub mod code;
pub mod config;
pub mod db;
pub mod error;
pub mod import;
pub mod latex;
pub mod nodes;
pub mod rag;
pub mod sensitive;
pub mod storage;
pub mod textview;
#[cfg(feature = "embed")]
pub mod unigram;
pub mod util;

use std::sync::{Arc, RwLock};

use sqlx::SqlitePool;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub cfg: Arc<config::Config>,
    pub store: storage::Store,
    pub limiter: Arc<auth::RateLimiter>,
    pub latex: Option<Arc<latex::LatexClient>>,
    pub rag: Option<Arc<rag::Rag>>,
    pub code: Arc<code::index::CodeIndex>,
    /// Extra origin allowed in CSP `img-src` (the configured background image).
    pub csp_img_extra: Arc<RwLock<String>>,
}

impl AppState {
    /// Nudge the background indexers (semantic search, code index) after content changes.
    pub fn poke_rag(&self) {
        if let Some(r) = &self.rag {
            r.poke();
        }
        self.code.poke();
    }

    pub async fn new(cfg: config::Config) -> anyhow::Result<Self> {
        let db = db::open(&cfg.data_dir).await?;
        let store = storage::Store::new(&cfg.data_dir)?;
        let latex = cfg
            .latex_url
            .as_ref()
            .filter(|u| !u.is_empty())
            .map(|u| Arc::new(latex::LatexClient::new(u.clone(), &cfg)));
        let rag = rag::Rag::from_config(&cfg, latex.clone());
        let bg: Option<String> =
            sqlx::query_scalar("SELECT value FROM settings WHERE key = 'background_url'")
                .fetch_optional(&db)
                .await?;
        Ok(Self {
            db,
            store,
            limiter: Arc::new(auth::RateLimiter::default()),
            latex,
            rag,
            code: Arc::new(code::index::CodeIndex::new()),
            csp_img_extra: Arc::new(RwLock::new(api::settings::csp_origin(
                bg.as_deref().unwrap_or(""),
            ))),
            cfg: Arc::new(cfg),
        })
    }
}
