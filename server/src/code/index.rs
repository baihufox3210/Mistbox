//! Background code indexer and queries over its tables (`code_*`).
//!
//! Same lifecycle as the semantic-search indexer: files are (re)analysed when
//! their content hash or the engine version changes; sensitive files and the
//! recycle bin are never indexed.

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Duration;

use serde::Serialize;
use sqlx::SqlitePool;
use tokio::sync::Notify;

use super::clones::{self, Match, Region};
use super::{ENGINE, Lang, MAX_FILE_BYTES, analyze};
use crate::{nodes::Node, storage::Store, textview, util::now};

/// Prefix a statement with `t(id)`: the recycle bin and everything in it.
macro_rules! with_trash {
    ($sql:literal) => {
        concat!(
            "WITH RECURSIVE t(id) AS (SELECT 'trash' UNION ALL SELECT n2.id FROM nodes n2 JOIN t ON n2.parent_id = t.id) ",
            $sql
        )
    };
}
/// Prefix a statement with `sub(id)`: the first bound node and everything below it (no trash).
macro_rules! with_subtree {
    ($sql:literal) => {
        concat!(
            "WITH RECURSIVE sub(id) AS (SELECT id FROM nodes WHERE id = ? UNION ALL SELECT n.id FROM nodes n JOIN sub ON n.parent_id = sub.id WHERE n.id <> 'trash') ",
            $sql
        )
    };
}

pub(crate) use with_subtree;

/// Per-file rows, cleared before re-analysing a file.
const CLEAR_FILE: [&str; 6] = [
    "DELETE FROM code_text WHERE node_id = ?",
    "DELETE FROM note_tags WHERE node_id = ?",
    "DELETE FROM code_symbols WHERE node_id = ?",
    "DELETE FROM code_refs WHERE node_id = ?",
    "DELETE FROM code_findings WHERE node_id = ?",
    "DELETE FROM code_fp WHERE node_id = ?",
];
/// Rows of files that are no longer indexed.
const CLEAR_STALE: [&str; 6] = [
    "DELETE FROM note_tags WHERE node_id NOT IN (SELECT node_id FROM code_files WHERE status = 'indexed')",
    "DELETE FROM code_text WHERE node_id NOT IN (SELECT node_id FROM code_files WHERE status = 'indexed')",
    "DELETE FROM code_symbols WHERE node_id NOT IN (SELECT node_id FROM code_files WHERE status = 'indexed')",
    "DELETE FROM code_refs WHERE node_id NOT IN (SELECT node_id FROM code_files WHERE status = 'indexed')",
    "DELETE FROM code_findings WHERE node_id NOT IN (SELECT node_id FROM code_files WHERE status = 'indexed')",
    "DELETE FROM code_fp WHERE node_id NOT IN (SELECT node_id FROM code_files WHERE status = 'indexed')",
];

/// A fingerprint shared by more places than this is boilerplate, not a clone.
const COMMON_HASH: usize = 40;

#[derive(Default)]
pub struct CodeIndex {
    wake: Notify,
    state: RwLock<&'static str>,
}

impl CodeIndex {
    pub fn new() -> Self {
        Self {
            wake: Notify::new(),
            state: RwLock::new("starting"),
        }
    }

    pub fn poke(&self) {
        self.wake.notify_one();
    }

    pub fn state(&self) -> &'static str {
        *self.state.read().unwrap()
    }

    /// Background loop: analyse changed files, then (if enabled) embed them.
    pub fn spawn(
        self: &std::sync::Arc<Self>,
        db: SqlitePool,
        store: Store,
        sem: Option<std::sync::Arc<super::embed::CodeSemantic>>,
    ) {
        let me = self.clone();
        tokio::spawn(async move {
            loop {
                *me.state.write().unwrap() = "indexing";
                match me.index_pending(&db, &store).await {
                    Ok(n) if n > 0 => tracing::info!(files = n, "code index updated"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "code indexing pass failed"),
                }
                if let Some(sem) = &sem {
                    *me.state.write().unwrap() = "embedding";
                    match sem.index_pending(&db).await {
                        Ok(n) if n > 0 => tracing::info!(files = n, "code embeddings updated"),
                        Ok(_) => {}
                        Err(e) => tracing::warn!(error = %e, "code embedding pass failed"),
                    }
                    sem.maybe_unload();
                }
                *me.state.write().unwrap() = "ready";
                let _ = tokio::time::timeout(Duration::from_secs(60), me.wake.notified()).await;
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }

    /// Analyse every file whose content (or the engine) changed. Returns files processed.
    pub async fn index_pending(&self, db: &SqlitePool, store: &Store) -> anyhow::Result<usize> {
        // Sensitive files and the recycle bin leave the index (cascades to rows).
        sqlx::query(
            "DELETE FROM code_files WHERE node_id IN (SELECT id FROM nodes WHERE sensitive = 1)",
        )
        .execute(db)
        .await?;
        sqlx::query(with_trash!(
            "DELETE FROM code_files WHERE node_id IN (SELECT id FROM t)"
        ))
        .execute(db)
        .await?;
        for q in CLEAR_STALE {
            sqlx::query(q).execute(db).await?;
        }
        let mut done = 0;
        loop {
            let batch: Vec<Node> = sqlx::query_as(with_trash!(
                "SELECT n.* FROM nodes n LEFT JOIN code_files c ON c.node_id = n.id
                 WHERE n.kind = 'file' AND n.sensitive = 0 AND n.id NOT IN (SELECT id FROM t)
                   AND (c.node_id IS NULL OR c.sha256 <> COALESCE(n.sha256, '') OR c.engine <> ?)
                 ORDER BY n.modified_at DESC LIMIT 50"
            ))
            .bind(ENGINE)
            .fetch_all(db)
            .await?;
            if batch.is_empty() {
                break;
            }
            for n in &batch {
                if let Err(e) = self.index_node(db, store, n).await {
                    tracing::warn!(node = %n.id, error = %e, "code indexing failed");
                    record(db, n, None, "failed", &e.to_string(), 0, 0).await?;
                }
                done += 1;
            }
        }
        Ok(done)
    }

    async fn index_node(&self, db: &SqlitePool, store: &Store, n: &Node) -> anyhow::Result<()> {
        let Some(lang) = Lang::from_name(&n.name) else {
            return record(db, n, None, "skipped", "不是程式碼", 0, 0).await;
        };
        if n.size > MAX_FILE_BYTES {
            return record(
                db,
                n,
                Some(lang),
                "skipped",
                "超過分析大小上限（1 MiB）",
                0,
                0,
            )
            .await;
        }
        let buf = crate::rag::read_blob(store, n, MAX_FILE_BYTES as u64).await?;
        if textview::looks_binary(&buf) {
            return record(db, n, Some(lang), "skipped", "二進位內容", 0, 0).await;
        }
        let (text, _, _) = textview::detect(&buf).decode(&buf);
        let text = text.into_owned();
        let (a, text) = tokio::task::spawn_blocking(move || (analyze(lang, &text), text)).await?;

        let mut tx = db.begin().await?;
        for q in CLEAR_FILE {
            sqlx::query(q).bind(&n.id).execute(&mut *tx).await?;
        }
        sqlx::query("INSERT INTO code_text (node_id, lang, body) VALUES (?, ?, ?)")
            .bind(&n.id)
            .bind(lang.as_str())
            .bind(&text)
            .execute(&mut *tx)
            .await?;
        if lang == Lang::Markdown {
            for (kind, value) in super::notes::tags(&text) {
                sqlx::query("INSERT INTO note_tags (node_id, kind, value) VALUES (?, ?, ?)")
                    .bind(&n.id)
                    .bind(kind)
                    .bind(&value)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        for s in &a.symbols {
            sqlx::query("INSERT INTO code_symbols (node_id, name, kind, line_start, line_end) VALUES (?, ?, ?, ?, ?)")
                .bind(&n.id)
                .bind(&s.name)
                .bind(s.kind)
                .bind(s.line_start)
                .bind(s.line_end)
                .execute(&mut *tx)
                .await?;
        }
        for r in &a.refs {
            sqlx::query(
                "INSERT INTO code_refs (node_id, name, line, in_symbol) VALUES (?, ?, ?, ?)",
            )
            .bind(&n.id)
            .bind(&r.name)
            .bind(r.line)
            .bind(&r.in_symbol)
            .execute(&mut *tx)
            .await?;
        }
        for f in &a.findings {
            sqlx::query(
                "INSERT INTO code_findings (node_id, rule, severity, line, col, line_end, message, symbol) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&n.id)
            .bind(f.rule)
            .bind(f.severity.as_str())
            .bind(f.line)
            .bind(f.col)
            .bind(f.line_end)
            .bind(&f.message)
            .bind(&f.symbol)
            .execute(&mut *tx)
            .await?;
        }
        for f in &a.fingerprints {
            sqlx::query("INSERT INTO code_fp (hash, node_id, tok, line_start, line_end) VALUES (?, ?, ?, ?, ?)")
                .bind(f.hash)
                .bind(&n.id)
                .bind(f.tok)
                .bind(f.line_start)
                .bind(f.line_end)
                .execute(&mut *tx)
                .await?;
        }
        upsert(
            &mut tx,
            n,
            Some(lang),
            "indexed",
            "",
            a.lines,
            a.tokens as u32,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

async fn upsert(
    conn: &mut sqlx::SqliteConnection,
    n: &Node,
    lang: Option<Lang>,
    status: &str,
    detail: &str,
    lines: u32,
    tokens: u32,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO code_files (node_id, sha256, engine, lang, status, detail, lines, tokens, indexed_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(node_id) DO UPDATE SET sha256 = excluded.sha256, engine = excluded.engine, lang = excluded.lang,
           status = excluded.status, detail = excluded.detail, lines = excluded.lines, tokens = excluded.tokens,
           indexed_at = excluded.indexed_at",
    )
    .bind(&n.id)
    .bind(n.sha256.as_deref().unwrap_or(""))
    .bind(ENGINE)
    .bind(lang.map(|l| l.as_str()))
    .bind(status)
    .bind(detail)
    .bind(lines)
    .bind(tokens)
    .bind(now())
    .execute(conn)
    .await?;
    Ok(())
}

async fn record(
    db: &SqlitePool,
    n: &Node,
    lang: Option<Lang>,
    status: &str,
    detail: &str,
    lines: u32,
    tokens: u32,
) -> anyhow::Result<()> {
    let mut tx = db.begin().await?;
    for q in CLEAR_FILE {
        sqlx::query(q).bind(&n.id).execute(&mut *tx).await?;
    }
    upsert(&mut tx, n, lang, status, detail, lines, tokens).await?;
    tx.commit().await?;
    Ok(())
}

// ------------------------------------------------------------------ queries

#[derive(Serialize, sqlx::FromRow, Debug)]
pub struct StoredFinding {
    pub node_id: String,
    pub rule: String,
    pub severity: String,
    pub line: i64,
    pub col: i64,
    pub line_end: i64,
    pub message: String,
    pub symbol: Option<String>,
}

/// Findings in one file, or in every file below a folder.
pub async fn findings(
    db: &SqlitePool,
    node: &Node,
    severity: Option<&str>,
    limit: i64,
) -> anyhow::Result<Vec<StoredFinding>> {
    let sev = severity.unwrap_or("");
    macro_rules! order {
        () => {
            " ORDER BY CASE f.severity WHEN 'error' THEN 0 WHEN 'warning' THEN 1 WHEN 'perf' THEN 2 ELSE 3 END, f.node_id, f.line LIMIT ?"
        };
    }
    Ok(if node.is_file() {
        sqlx::query_as(concat!(
            "SELECT f.node_id, f.rule, f.severity, f.line, f.col, f.line_end, f.message, f.symbol FROM code_findings f
             WHERE f.node_id = ? AND (? = '' OR f.severity = ?)",
            order!()
        ))
        .bind(&node.id)
        .bind(sev)
        .bind(sev)
        .bind(limit)
        .fetch_all(db)
        .await?
    } else {
        sqlx::query_as(concat!(
            with_subtree!(
                "SELECT f.node_id, f.rule, f.severity, f.line, f.col, f.line_end, f.message, f.symbol
             FROM code_findings f JOIN sub ON sub.id = f.node_id
             WHERE (? = '' OR f.severity = ?)"
            ),
            order!()
        ))
        .bind(&node.id)
        .bind(sev)
        .bind(sev)
        .bind(limit)
        .fetch_all(db)
        .await?
    })
}

#[derive(Serialize, Debug)]
pub struct Clone {
    /// The file asked about (or, for folder queries, the first file of the pair).
    pub node_id: String,
    pub other_id: String,
    #[serde(flatten)]
    pub region: Region,
}

/// (hash, a.tok, a.line_start, a.line_end, b.node_id, b.tok, b.line_start, b.line_end)
type FpPair = (i64, i64, i64, i64, String, i64, i64, i64);

/// Clones of `node`'s code anywhere in `scope` (a folder id), including inside `node` itself.
pub async fn clones_of(db: &SqlitePool, node_id: &str, scope: &str) -> anyhow::Result<Vec<Clone>> {
    let rows: Vec<FpPair> = sqlx::query_as(with_subtree!(
        "SELECT a.hash, a.tok, a.line_start, a.line_end, b.node_id, b.tok, b.line_start, b.line_end
         FROM code_fp a JOIN code_fp b ON b.hash = a.hash JOIN sub ON sub.id = b.node_id
         WHERE a.node_id = ?"
    ))
    .bind(scope)
    .bind(node_id)
    .fetch_all(db)
    .await?;
    // Drop boilerplate hashes (shared by many places).
    let mut freq: HashMap<i64, usize> = HashMap::new();
    for r in &rows {
        *freq.entry(r.0).or_default() += 1;
    }
    let mut by_other: HashMap<String, Vec<Match>> = HashMap::new();
    for (hash, a_tok, a_start, a_end, other, b_tok, b_start, b_end) in rows {
        if freq[&hash] > COMMON_HASH || (other == node_id && a_tok == b_tok) {
            continue;
        }
        by_other.entry(other).or_default().push(Match {
            a_tok: a_tok as u32,
            a_start: a_start as u32,
            a_end: a_end as u32,
            b_tok: b_tok as u32,
            b_start: b_start as u32,
            b_end: b_end as u32,
        });
    }
    let mut out = Vec::new();
    for (other, ms) in by_other {
        let same = other == node_id;
        for region in clones::regions(ms, same) {
            out.push(Clone {
                node_id: node_id.to_string(),
                other_id: other.clone(),
                region,
            });
        }
    }
    out.sort_by_key(|c| (std::cmp::Reverse(c.region.tokens), c.region.a_start));
    Ok(out)
}

/// Largest clones between any two files below `folder` (each pair reported once).
pub async fn clones_in(db: &SqlitePool, folder: &str, limit: usize) -> anyhow::Result<Vec<Clone>> {
    let files: Vec<String> = sqlx::query_scalar(with_subtree!(
        "SELECT c.node_id FROM code_files c JOIN sub ON sub.id = c.node_id WHERE c.status = 'indexed' AND c.tokens > 0 ORDER BY c.node_id LIMIT 3000"
    ))
    .bind(folder)
    .fetch_all(db)
    .await?;
    let mut out = Vec::new();
    for f in &files {
        for c in clones_of(db, f, folder).await? {
            // Each unordered pair once: keep (a, b) with a <= b.
            if c.other_id.as_str() >= f.as_str() {
                out.push(c);
            }
        }
    }
    out.sort_by_key(|c| std::cmp::Reverse(c.region.tokens));
    out.truncate(limit);
    Ok(out)
}

#[derive(Serialize, Debug, Default)]
pub struct Status {
    pub state: &'static str,
    /// Source files analysed.
    pub files: i64,
    /// Markdown notes indexed for full-text search.
    pub notes: i64,
    pub pending: i64,
    pub by_lang: HashMap<String, i64>,
    pub findings: HashMap<String, i64>,
}

pub async fn status(db: &SqlitePool, idx: &CodeIndex) -> anyhow::Result<Status> {
    let by_lang: Vec<(String, i64)> = sqlx::query_as(
        "SELECT lang, COUNT(*) FROM code_files WHERE status = 'indexed' GROUP BY lang",
    )
    .fetch_all(db)
    .await?;
    let findings: Vec<(String, i64)> =
        sqlx::query_as("SELECT severity, COUNT(*) FROM code_findings GROUP BY severity")
            .fetch_all(db)
            .await?;
    let pending: i64 = sqlx::query_scalar(with_trash!(
        "SELECT COUNT(*) FROM nodes n LEFT JOIN code_files c ON c.node_id = n.id
         WHERE n.kind = 'file' AND n.sensitive = 0 AND n.id NOT IN (SELECT id FROM t)
           AND (c.node_id IS NULL OR c.sha256 <> COALESCE(n.sha256, '') OR c.engine <> ?)"
    ))
    .bind(ENGINE)
    .fetch_one(db)
    .await?;
    Ok(Status {
        state: idx.state(),
        files: by_lang
            .iter()
            .filter(|x| x.0 != "markdown")
            .map(|x| x.1)
            .sum(),
        notes: by_lang
            .iter()
            .filter(|x| x.0 == "markdown")
            .map(|x| x.1)
            .sum(),
        pending,
        by_lang: by_lang.into_iter().collect(),
        findings: findings.into_iter().collect(),
    })
}

/// Index state of one file: lets clients tell whether results match the
/// current content (`sha256`) or are still pending.
#[derive(Serialize, sqlx::FromRow, Debug)]
pub struct FileState {
    pub status: String,
    pub sha256: String,
    pub lang: Option<String>,
    pub detail: Option<String>,
    pub lines: i64,
}

pub async fn file_state(db: &SqlitePool, node_id: &str) -> anyhow::Result<Option<FileState>> {
    Ok(sqlx::query_as("SELECT status, sha256, lang, detail, lines FROM code_files WHERE node_id = ? AND engine = ?")
        .bind(node_id)
        .bind(ENGINE)
        .fetch_optional(db)
        .await?)
}

#[derive(Serialize, sqlx::FromRow, Debug)]
pub struct StoredSymbol {
    pub node_id: String,
    pub name: String,
    pub kind: String,
    pub line_start: i64,
    pub line_end: i64,
}

/// Definitions whose name matches `q` exactly or ends with `::q` / `.q`, below `scope`.
pub async fn symbols(
    db: &SqlitePool,
    q: &str,
    scope: &str,
    limit: i64,
) -> anyhow::Result<Vec<StoredSymbol>> {
    Ok(sqlx::query_as(with_subtree!(
        "SELECT s.node_id, s.name, s.kind, s.line_start, s.line_end FROM code_symbols s JOIN sub ON sub.id = s.node_id
         WHERE s.name = ?2 OR s.name LIKE '%::' || ?2 OR s.name LIKE '%.' || ?2
         ORDER BY s.name = ?2 DESC, s.name LIMIT ?3"
    ))
    .bind(scope)
    .bind(q)
    .bind(limit)
    .fetch_all(db)
    .await?)
}
