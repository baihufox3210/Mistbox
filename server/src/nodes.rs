//! Logical directory tree stored in SQLite.

use serde::{Deserialize, Serialize};
use sqlx::{SqliteConnection, SqlitePool};

use crate::{
    error::{ApiError, ApiResult},
    util::{new_id, now, ts_to_rfc3339},
};

pub const ROOT_ID: &str = "root";

#[derive(sqlx::FromRow, Clone, Debug)]
pub struct Node {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub kind: String,
    pub size: i64,
    pub mime: Option<String>,
    pub sha256: Option<String>,
    pub blob_id: Option<String>,
    pub sensitive: bool,
    pub created_at: i64,
    pub modified_at: i64,
}

impl Node {
    pub fn is_dir(&self) -> bool {
        self.kind == "dir"
    }
    pub fn is_file(&self) -> bool {
        self.kind == "file"
    }
}

/// JSON representation returned by the API.
#[derive(Serialize, Debug)]
pub struct NodeView {
    pub id: String,
    /// `null` for the principal's root (drive root, or the API key's scope).
    pub parent_id: Option<String>,
    pub name: String,
    pub kind: String,
    pub size: i64,
    pub mime: Option<String>,
    pub sha256: Option<String>,
    pub sensitive: bool,
    pub created_at: String,
    pub modified_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl NodeView {
    pub fn from_node(n: &Node, principal_root: &str) -> Self {
        Self {
            id: n.id.clone(),
            parent_id: if n.id == principal_root { None } else { n.parent_id.clone() },
            name: n.name.clone(),
            kind: n.kind.clone(),
            size: n.size,
            mime: n.mime.clone(),
            sha256: n.sha256.clone(),
            sensitive: n.sensitive,
            created_at: ts_to_rfc3339(n.created_at),
            modified_at: ts_to_rfc3339(n.modified_at),
            path: None,
        }
    }
    pub fn with_path(mut self, path: String) -> Self {
        self.path = Some(path);
        self
    }
}

#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConflictPolicy {
    /// Reject with 409 (default). Never overwrites.
    #[default]
    Fail,
    /// Keep both: store under "name (1).ext".
    Rename,
}

/// Reject names that are empty, dot-segments, too long, or contain
/// separators / control characters.
pub fn validate_name(name: &str) -> ApiResult<()> {
    if name.is_empty() || name == "." || name == ".." {
        return Err(ApiError::invalid_name("名稱不可為空白、「.」或「..」"));
    }
    if name.len() > 255 {
        return Err(ApiError::invalid_name("名稱過長（上限 255 位元組）"));
    }
    if name.chars().any(|c| c == '/' || c == '\\' || c.is_control()) {
        return Err(ApiError::invalid_name("名稱不可包含 / \\ 或控制字元"));
    }
    Ok(())
}

/// Split a client-supplied relative path ("a/b/c.txt") into validated segments.
pub fn split_rel_path(path: &str) -> ApiResult<Vec<String>> {
    let segs: Vec<String> = path.split('/').filter(|s| !s.is_empty()).map(str::to_string).collect();
    if segs.is_empty() {
        return Err(ApiError::invalid_name("路徑為空"));
    }
    for s in &segs {
        validate_name(s)?;
    }
    Ok(segs)
}

fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.is_unique_violation())
}

/// "report.pdf" -> "report (n).pdf"; ".env" -> ".env (n)".
pub fn numbered_name(name: &str, n: u32) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 => format!("{} ({n}){}", &name[..i], &name[i..]),
        _ => format!("{name} ({n})"),
    }
}

pub async fn get(db: &SqlitePool, id: &str) -> ApiResult<Node> {
    sqlx::query_as::<_, Node>("SELECT * FROM nodes WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or_else(ApiError::not_found)
}

pub async fn get_opt(db: &SqlitePool, id: &str) -> ApiResult<Option<Node>> {
    Ok(sqlx::query_as::<_, Node>("SELECT * FROM nodes WHERE id = ?").bind(id).fetch_optional(db).await?)
}

pub async fn child_by_name(db: &SqlitePool, parent: &str, name: &str) -> ApiResult<Option<Node>> {
    Ok(sqlx::query_as::<_, Node>("SELECT * FROM nodes WHERE parent_id = ? AND name = ?")
        .bind(parent)
        .bind(name)
        .fetch_optional(db)
        .await?)
}

pub async fn children(db: &SqlitePool, parent: &str) -> ApiResult<Vec<Node>> {
    Ok(sqlx::query_as::<_, Node>(
        "SELECT * FROM nodes WHERE parent_id = ? ORDER BY kind = 'file', name COLLATE NOCASE, name",
    )
    .bind(parent)
    .fetch_all(db)
    .await?)
}

/// Ancestors from the drive root down to (and including) `id`.
pub async fn ancestors(db: &SqlitePool, id: &str) -> ApiResult<Vec<Node>> {
    Ok(sqlx::query_as::<_, Node>(
        "WITH RECURSIVE anc(id, parent_id, depth) AS (
             SELECT id, parent_id, 0 FROM nodes WHERE id = ?
             UNION ALL
             SELECT n.id, n.parent_id, anc.depth + 1 FROM nodes n JOIN anc ON n.id = anc.parent_id
         )
         SELECT nodes.* FROM anc JOIN nodes ON nodes.id = anc.id ORDER BY anc.depth DESC",
    )
    .bind(id)
    .fetch_all(db)
    .await?)
}

/// True if `id` is `ancestor` or lies anywhere below it.
pub async fn is_within(db: &SqlitePool, id: &str, ancestor: &str) -> ApiResult<bool> {
    if id == ancestor {
        return Ok(true);
    }
    let hit: Option<i64> = sqlx::query_scalar(
        "WITH RECURSIVE anc(id, parent_id) AS (
             SELECT id, parent_id FROM nodes WHERE id = ?
             UNION ALL
             SELECT n.id, n.parent_id FROM nodes n JOIN anc ON n.id = anc.parent_id
         )
         SELECT 1 FROM anc WHERE id = ? LIMIT 1",
    )
    .bind(id)
    .bind(ancestor)
    .fetch_optional(db)
    .await?;
    Ok(hit.is_some())
}

/// "/a/b/c" relative to `root_id` (which renders as "/").
pub fn path_string(chain: &[Node], root_id: &str) -> String {
    let start = chain.iter().position(|n| n.id == root_id).map(|i| i + 1).unwrap_or(chain.len());
    let parts: Vec<&str> = chain[start..].iter().map(|n| n.name.as_str()).collect();
    format!("/{}", parts.join("/"))
}

pub async fn path_of(db: &SqlitePool, id: &str, root_id: &str) -> ApiResult<String> {
    Ok(path_string(&ancestors(db, id).await?, root_id))
}

/// Resolve `rel` (POSIX-style, may contain "." / "..") starting at directory
/// `base_dir`, never climbing above `floor` (the principal's root).
pub async fn resolve_rel(db: &SqlitePool, base_dir: &str, rel: &str, floor: &str) -> ApiResult<Node> {
    let (mut chain, segs): (Vec<String>, _) = if rel.starts_with('/') {
        (vec![floor.to_string()], rel)
    } else {
        let anc = ancestors(db, base_dir).await?;
        let start = anc.iter().position(|n| n.id == floor).ok_or_else(ApiError::not_found)?;
        (anc[start..].iter().map(|n| n.id.clone()).collect(), rel)
    };
    for seg in segs.split('/') {
        match seg {
            "" | "." => continue,
            ".." => {
                if chain.len() <= 1 {
                    return Err(ApiError::not_found());
                }
                chain.pop();
            }
            name => {
                let cur = chain.last().unwrap().clone();
                let child = child_by_name(db, &cur, name).await?.ok_or_else(ApiError::not_found)?;
                chain.push(child.id);
            }
        }
    }
    get(db, chain.last().unwrap()).await
}

pub async fn create_dir(db: &SqlitePool, parent: &Node, name: &str) -> ApiResult<Node> {
    validate_name(name)?;
    if !parent.is_dir() {
        return Err(ApiError::bad_request("目標不是資料夾"));
    }
    let id = new_id();
    let t = now();
    let res = sqlx::query(
        "INSERT INTO nodes (id, parent_id, name, kind, created_at, modified_at) VALUES (?, ?, ?, 'dir', ?, ?)",
    )
    .bind(&id)
    .bind(&parent.id)
    .bind(name)
    .bind(t)
    .bind(t)
    .execute(db)
    .await;
    match res {
        Ok(_) => get(db, &id).await,
        Err(e) if is_unique_violation(&e) => Err(ApiError::conflict(format!("「{name}」已存在"))),
        Err(e) => Err(e.into()),
    }
}

/// `mkdir -p` below `parent`. Fails with 409 if a segment exists as a file.
pub async fn ensure_dirs(db: &SqlitePool, parent: &Node, segs: &[String]) -> ApiResult<Node> {
    let mut cur = parent.clone();
    for s in segs {
        cur = match child_by_name(db, &cur.id, s).await? {
            Some(n) if n.is_dir() => n,
            Some(_) => return Err(ApiError::conflict(format!("「{s}」已存在且不是資料夾"))),
            None => match create_dir(db, &cur, s).await {
                Ok(n) => n,
                Err(e) if e.code == "conflict" => {
                    let n = child_by_name(db, &cur.id, s).await?.ok_or_else(ApiError::internal)?;
                    if !n.is_dir() {
                        return Err(ApiError::conflict(format!("「{s}」已存在且不是資料夾")));
                    }
                    n
                }
                Err(e) => return Err(e),
            },
        };
    }
    Ok(cur)
}

pub struct NewFile<'a> {
    pub name: &'a str,
    pub size: u64,
    pub sha256: &'a str,
    pub blob_id: &'a str,
    pub mime: &'a str,
    pub sensitive: bool,
    pub modified_at: Option<i64>,
}

/// Insert a committed blob as a file node. Never overwrites an existing entry.
pub async fn insert_file(db: &SqlitePool, parent: &Node, f: NewFile<'_>, policy: ConflictPolicy) -> ApiResult<Node> {
    validate_name(f.name)?;
    let id = new_id();
    let t = now();
    for attempt in 0..1000u32 {
        let name = if attempt == 0 { f.name.to_string() } else { numbered_name(f.name, attempt) };
        let res = sqlx::query(
            "INSERT INTO nodes (id, parent_id, name, kind, size, mime, sha256, blob_id, sensitive, created_at, modified_at)
             VALUES (?, ?, ?, 'file', ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&parent.id)
        .bind(&name)
        .bind(f.size as i64)
        .bind(f.mime)
        .bind(f.sha256)
        .bind(f.blob_id)
        .bind(f.sensitive)
        .bind(t)
        .bind(f.modified_at.unwrap_or(t))
        .execute(db)
        .await;
        match res {
            Ok(_) => return get(db, &id).await,
            Err(e) if is_unique_violation(&e) && policy == ConflictPolicy::Rename => continue,
            Err(e) if is_unique_violation(&e) => {
                return Err(ApiError::conflict(format!("「{}」已存在；未覆寫既有檔案", f.name))
                    .with_details(serde_json::json!({ "name": f.name })));
            }
            Err(e) => return Err(e.into()),
        }
    }
    Err(ApiError::conflict("無法產生不重複的檔名"))
}

pub async fn rename(db: &SqlitePool, node: &Node, new_name: &str) -> ApiResult<Node> {
    validate_name(new_name)?;
    if node.id == ROOT_ID {
        return Err(ApiError::bad_request("無法重新命名根目錄"));
    }
    let res = sqlx::query("UPDATE nodes SET name = ?, modified_at = ? WHERE id = ?")
        .bind(new_name)
        .bind(now())
        .bind(&node.id)
        .execute(db)
        .await;
    match res {
        Ok(_) => get(db, &node.id).await,
        Err(e) if is_unique_violation(&e) => Err(ApiError::conflict(format!("「{new_name}」已存在"))),
        Err(e) => Err(e.into()),
    }
}

pub async fn move_node(db: &SqlitePool, node: &Node, dest: &Node) -> ApiResult<Node> {
    if node.id == ROOT_ID {
        return Err(ApiError::bad_request("無法移動根目錄"));
    }
    if !dest.is_dir() {
        return Err(ApiError::bad_request("目標不是資料夾"));
    }
    if is_within(db, &dest.id, &node.id).await? {
        return Err(ApiError::bad_request("無法把資料夾移動到自己或其子資料夾內"));
    }
    let res = sqlx::query("UPDATE nodes SET parent_id = ? WHERE id = ?")
        .bind(&dest.id)
        .bind(&node.id)
        .execute(db)
        .await;
    match res {
        Ok(_) => get(db, &node.id).await,
        Err(e) if is_unique_violation(&e) => {
            Err(ApiError::conflict(format!("目標資料夾已有「{}」", node.name)))
        }
        Err(e) => Err(e.into()),
    }
}

#[derive(Serialize, Default, Debug)]
pub struct SubtreeStats {
    pub files: i64,
    pub dirs: i64,
    pub bytes: i64,
}

pub async fn subtree_stats(db: &SqlitePool, id: &str) -> ApiResult<SubtreeStats> {
    let row: (i64, i64, i64) = sqlx::query_as(
        "WITH RECURSIVE sub(id) AS (
             SELECT id FROM nodes WHERE id = ?
             UNION ALL SELECT n.id FROM nodes n JOIN sub ON n.parent_id = sub.id
         )
         SELECT COALESCE(SUM(kind = 'file'), 0), COALESCE(SUM(kind = 'dir'), 0), COALESCE(SUM(size), 0)
         FROM nodes JOIN sub USING (id)",
    )
    .bind(id)
    .fetch_one(db)
    .await?;
    Ok(SubtreeStats { files: row.0, dirs: row.1, bytes: row.2 })
}

/// Delete a node and its subtree. Blobs are queued for GC in the same transaction.
pub async fn delete(db: &SqlitePool, node: &Node) -> ApiResult<SubtreeStats> {
    if node.id == ROOT_ID {
        return Err(ApiError::bad_request("無法刪除根目錄"));
    }
    let stats = subtree_stats(db, &node.id).await?;
    let mut tx = db.begin().await?;
    queue_subtree_blobs(&mut tx, &node.id).await?;
    sqlx::query("DELETE FROM nodes WHERE id = ?").bind(&node.id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(stats)
}

async fn queue_subtree_blobs(conn: &mut SqliteConnection, id: &str) -> ApiResult<()> {
    sqlx::query(
        "WITH RECURSIVE sub(id) AS (
             SELECT id FROM nodes WHERE id = ?
             UNION ALL SELECT n.id FROM nodes n JOIN sub ON n.parent_id = sub.id
         )
         INSERT OR IGNORE INTO blob_gc (blob_id, queued_at)
         SELECT blob_id, ? FROM nodes JOIN sub USING (id) WHERE blob_id IS NOT NULL",
    )
    .bind(id)
    .bind(now())
    .execute(conn)
    .await?;
    Ok(())
}

/// Case-insensitive (ASCII) substring search on names, limited to the subtree of `within`.
pub async fn search(db: &SqlitePool, q: &str, within: &str, limit: i64) -> ApiResult<Vec<Node>> {
    let escaped = q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    let pattern = format!("%{escaped}%");
    Ok(sqlx::query_as::<_, Node>(
        "WITH RECURSIVE sub(id) AS (
             SELECT id FROM nodes WHERE id = ?
             UNION ALL SELECT n.id FROM nodes n JOIN sub ON n.parent_id = sub.id
         )
         SELECT nodes.* FROM nodes JOIN sub USING (id)
         WHERE nodes.id <> ? AND nodes.name LIKE ? ESCAPE '\\'
         ORDER BY nodes.kind = 'file', nodes.name COLLATE NOCASE LIMIT ?",
    )
    .bind(within)
    .bind(within)
    .bind(pattern)
    .bind(limit)
    .fetch_all(db)
    .await?)
}

/// Unlink blobs queued for deletion, unless an online backup holds the lock.
/// The lock check and dequeue happen in one write transaction, so a backup
/// that starts afterwards can never reference a blob we are about to remove.
pub async fn run_blob_gc(db: &SqlitePool, store: &crate::storage::Store) -> anyhow::Result<usize> {
    let mut tx = db.begin().await?;
    let ids: Vec<String> = sqlx::query_scalar(
        "DELETE FROM blob_gc
         WHERE (SELECT backup_until FROM maintenance WHERE id = 1) < ?
           AND blob_id NOT IN (SELECT blob_id FROM nodes WHERE blob_id IS NOT NULL)
         RETURNING blob_id",
    )
    .bind(now())
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    for id in &ids {
        store.remove_blob(id).await;
    }
    Ok(ids.len())
}
