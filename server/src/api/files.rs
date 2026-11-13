use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures_util::TryStreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::{ReaderStream, StreamReader};

use super::resolve_alias;
use crate::{
    AppState,
    auth::Principal,
    authz::{self, Perm},
    error::{ApiError, ApiResult},
    nodes::{self, ConflictPolicy, Node, NodeView},
    sensitive,
    storage::StoreError,
    util::{content_disposition, guess_mime},
};

// ------------------------------------------------------------------ helpers

pub(crate) async fn view_with_path(
    state: &AppState,
    p: &Principal,
    n: &Node,
) -> ApiResult<NodeView> {
    let path = nodes::path_of(&state.db, &n.id, p.root_id()).await?;
    Ok(NodeView::from_node(n, p.root_id()).with_path(path))
}

pub(crate) async fn info_json(
    state: &AppState,
    p: &Principal,
    n: &Node,
    stats: bool,
) -> ApiResult<Json<Value>> {
    authz::require_info(state, p, n).await?;
    let mut v = serde_json::to_value(view_with_path(state, p, n).await?).unwrap();
    if stats && n.is_dir() {
        v["stats"] = serde_json::to_value(nodes::subtree_stats(&state.db, &n.id).await?).unwrap();
    }
    Ok(Json(v))
}

pub(crate) async fn list_json(
    state: &AppState,
    p: &Principal,
    dir: &Node,
) -> ApiResult<Json<Value>> {
    authz::require(state, p, dir, Perm::List).await?;
    if !dir.is_dir() {
        return Err(ApiError::bad_request("不是資料夾"));
    }
    let chain = nodes::ancestors(&state.db, &dir.id).await?;
    let start = chain.iter().position(|n| n.id == p.root_id()).unwrap_or(0);
    let crumbs: Vec<Value> = chain[start..]
        .iter()
        .map(|n| json!({ "id": n.id, "name": n.name }))
        .collect();
    let kids = authz::filter_visible(state, p, nodes::children(&state.db, &dir.id).await?).await?;
    let kids: Vec<NodeView> = kids
        .iter()
        .map(|n| NodeView::from_node(n, p.root_id()))
        .collect();
    Ok(Json(json!({
        "node": NodeView::from_node(dir, p.root_id()).with_path(nodes::path_string(&chain, p.root_id())),
        "breadcrumbs": crumbs,
        "children": kids,
    })))
}

/// MIME types we are willing to serve inline (rendered by the browser).
/// Everything else is forced to download as application/octet-stream.
fn inline_mime(mime: &str) -> Option<&str> {
    const OK: &[&str] = &[
        "image/png",
        "image/jpeg",
        "image/gif",
        "image/webp",
        "image/avif",
        "image/bmp",
        "image/x-icon",
        "image/vnd.microsoft.icon",
        "image/svg+xml",
        "application/pdf",
        "audio/mpeg",
        "audio/ogg",
        "audio/wav",
        "audio/x-wav",
        "audio/webm",
        "audio/flac",
        "audio/x-flac",
        "audio/mp4",
        "audio/aac",
        "audio/m4a",
        "audio/x-m4a",
        "video/mp4",
        "video/webm",
        "video/ogg",
        "video/quicktime",
    ];
    OK.iter().find(|m| **m == mime).copied()
}

/// Parse a single `bytes=` range. `None` = ignore the header (serve full),
/// `Some(Err)` = unsatisfiable.
fn parse_range(h: &str, size: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = h.strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        if n == 0 {
            return Some(Err(()));
        }
        (size.saturating_sub(n), size.checked_sub(1)?)
    } else {
        let s: u64 = a.parse().ok()?;
        let e: u64 = if b.is_empty() {
            size.saturating_sub(1)
        } else {
            b.parse::<u64>().ok()?.min(size.saturating_sub(1))
        };
        (s, e)
    };
    if size == 0 || start > end || start >= size {
        return Some(Err(()));
    }
    Some(Ok((start, end)))
}

/// Stream a file's bytes. Supports HEAD and single Range requests.
pub(crate) async fn serve_file(
    state: &AppState,
    n: &Node,
    inline: bool,
    req_headers: &HeaderMap,
) -> ApiResult<Response> {
    if !n.is_file() {
        return Err(ApiError::bad_request("資料夾無法直接下載"));
    }
    let path = state
        .store
        .blob_path(n.blob_id.as_deref().unwrap_or_default());
    let mut file = tokio::fs::File::open(&path).await.map_err(|e| {
        tracing::error!(node = %n.id, error = %e, "blob missing");
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "blob_missing",
            "檔案內容遺失，請執行 verify 檢查",
        )
    })?;
    let size = file.metadata().await?.len();
    let stored = n.mime.as_deref().unwrap_or("application/octet-stream");
    let (ctype, disp) = match (inline, inline_mime(stored)) {
        (true, Some(m)) => (m.to_string(), "inline"),
        _ => ("application/octet-stream".to_string(), "attachment"),
    };

    let mut status = StatusCode::OK;
    let (mut start, mut len) = (0u64, size);
    let mut headers = HeaderMap::new();
    if let Some(r) = req_headers.get(header::RANGE).and_then(|v| v.to_str().ok()) {
        let etag_ok = req_headers
            .get(header::IF_RANGE)
            .and_then(|v| v.to_str().ok())
            .is_none_or(|v| {
                n.sha256
                    .as_deref()
                    .is_some_and(|s| v.trim_matches('"') == s)
            });
        if etag_ok {
            match parse_range(r, size) {
                Some(Ok((s, e))) => {
                    status = StatusCode::PARTIAL_CONTENT;
                    start = s;
                    len = e - s + 1;
                    headers.insert(
                        header::CONTENT_RANGE,
                        format!("bytes {s}-{e}/{size}").parse().unwrap(),
                    );
                }
                Some(Err(())) => {
                    let mut r = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
                    r.headers_mut().insert(
                        header::CONTENT_RANGE,
                        format!("bytes */{size}").parse().unwrap(),
                    );
                    return Ok(r);
                }
                None => {}
            }
        }
    }
    if start > 0 {
        file.seek(std::io::SeekFrom::Start(start)).await?;
    }
    let body = Body::from_stream(ReaderStream::with_capacity(file.take(len), 256 * 1024));

    headers.insert(header::CONTENT_TYPE, ctype.parse().unwrap());
    headers.insert(header::CONTENT_LENGTH, len.into());
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&content_disposition(disp, &n.name)).unwrap(),
    );
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(super::CONTENT_CSP),
    );
    if let Some(sha) = &n.sha256 {
        headers.insert(header::ETAG, format!("\"{sha}\"").parse().unwrap());
        headers.insert("x-content-sha256", sha.parse().unwrap());
    }
    Ok((status, headers, body).into_response())
}

// ------------------------------------------------------------------ handlers

#[derive(Deserialize)]
pub struct InfoQuery {
    #[serde(default, deserialize_with = "crate::util::flag")]
    stats: bool,
}

pub async fn info(
    State(state): State<AppState>,
    p: Principal,
    Path(id): Path<String>,
    Query(q): Query<InfoQuery>,
) -> ApiResult<Json<Value>> {
    let n = nodes::get(&state.db, &resolve_alias(&p, &id)).await?;
    info_json(&state, &p, &n, q.stats).await
}

pub async fn children(
    State(state): State<AppState>,
    p: Principal,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let n = nodes::get(&state.db, &resolve_alias(&p, &id)).await?;
    list_json(&state, &p, &n).await
}

#[derive(Deserialize)]
pub struct ContentQuery {
    #[serde(default, deserialize_with = "crate::util::flag")]
    inline: bool,
}

pub async fn content(
    State(state): State<AppState>,
    p: Principal,
    Path(id): Path<String>,
    Query(q): Query<ContentQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let n = authz::load(&state, &p, &resolve_alias(&p, &id), Perm::Read).await?;
    serve_file(&state, &n, q.inline, &headers).await
}

#[derive(Deserialize)]
pub struct MkdirBody {
    name: String,
}

pub async fn mkdir(
    State(state): State<AppState>,
    p: Principal,
    Path(id): Path<String>,
    Json(b): Json<MkdirBody>,
) -> ApiResult<(StatusCode, Json<NodeView>)> {
    let parent = authz::load(&state, &p, &resolve_alias(&p, &id), Perm::Write).await?;
    let n = nodes::create_dir(&state.db, &parent, b.name.trim()).await?;
    Ok((
        StatusCode::CREATED,
        Json(view_with_path(&state, &p, &n).await?),
    ))
}

#[derive(Deserialize)]
pub struct UpdateBody {
    name: Option<String>,
    parent_id: Option<String>,
    sensitive: Option<bool>,
}

/// Rename and/or move. Both ends must be authorized.
pub async fn update(
    State(state): State<AppState>,
    p: Principal,
    Path(id): Path<String>,
    Json(b): Json<UpdateBody>,
) -> ApiResult<Json<NodeView>> {
    let mut n = nodes::get(&state.db, &resolve_alias(&p, &id)).await?;
    authz::require_modify(&state, &p, &n).await?;
    if let Some(dest_id) = &b.parent_id {
        let dest = authz::load(&state, &p, &resolve_alias(&p, dest_id), Perm::Write).await?;
        if n.parent_id.as_deref() != Some(dest.id.as_str()) {
            n = nodes::move_node(&state.db, &n, &dest).await?;
        }
    }
    if let Some(name) = &b.name
        && name != &n.name
    {
        n = nodes::rename(&state.db, &n, name.trim()).await?;
    }
    if let Some(s) = b.sensitive {
        if !p.is_admin() {
            return Err(ApiError::forbidden(format!("{}不能變更敏感標記", p.who())));
        }
        sqlx::query("UPDATE nodes SET sensitive = ? WHERE id = ?")
            .bind(s)
            .bind(&n.id)
            .execute(&state.db)
            .await?;
        n = nodes::get(&state.db, &n.id).await?;
        state.poke_rag();
    }
    Ok(Json(view_with_path(&state, &p, &n).await?))
}

#[derive(Deserialize)]
pub struct RemoveQuery {
    /// Skip the recycle bin (admin only). Items already in the bin are always deleted permanently.
    #[serde(default, deserialize_with = "crate::util::flag")]
    permanent: bool,
}

/// Delete = move to the recycle bin. Permanent deletion only for items
/// already in the bin or with `?permanent=1` (admin sessions only).
pub async fn remove(
    State(state): State<AppState>,
    p: Principal,
    Path(id): Path<String>,
    Query(q): Query<RemoveQuery>,
) -> ApiResult<Json<Value>> {
    let n = nodes::get(&state.db, &resolve_alias(&p, &id)).await?;
    authz::require_modify(&state, &p, &n).await?;
    let in_trash = nodes::in_trash(&state.db, &n.id).await?;
    if (q.permanent || in_trash) && !p.is_admin() {
        return Err(ApiError::forbidden(format!(
            "{}只能把項目移到垃圾桶，不能永久刪除",
            p.who()
        )));
    }
    let (stats, permanent) = if q.permanent || in_trash {
        (nodes::delete(&state.db, &n).await?, true)
    } else {
        (nodes::trash(&state.db, &n).await?, false)
    };
    tracing::info!(node = %n.id, files = stats.files, permanent, "deleted");
    state.poke_rag();
    Ok(Json(
        json!({ "deleted": n.id, "permanent": permanent, "stats": stats }),
    ))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    q: String,
    limit: Option<i64>,
}

pub async fn search(
    State(state): State<AppState>,
    p: Principal,
    Query(q): Query<SearchQuery>,
) -> ApiResult<Json<Value>> {
    let term = q.q.trim();
    if term.is_empty() {
        return Err(ApiError::bad_request("請輸入搜尋字串"));
    }
    let root = nodes::get(&state.db, p.root_id()).await?;
    authz::require(&state, &p, &root, Perm::List).await?;
    let hits = nodes::search(
        &state.db,
        term,
        &root.id,
        q.limit.unwrap_or(100).clamp(1, 500),
    )
    .await?;
    let hits = authz::filter_visible(&state, &p, hits).await?;
    let mut out = Vec::with_capacity(hits.len());
    for h in &hits {
        out.push(view_with_path(&state, &p, h).await?);
    }
    Ok(Json(json!({ "results": out })))
}

#[derive(Deserialize, Default)]
pub struct UploadQuery {
    /// File name (single file into the target folder).
    pub name: Option<String>,
    /// Relative path below the target folder ("sub/dir/file.txt"); missing folders are created.
    pub path: Option<String>,
    #[serde(default)]
    pub on_conflict: ConflictPolicy,
    /// Modification time, Unix seconds.
    pub mtime: Option<i64>,
}

pub async fn upload(
    State(state): State<AppState>,
    p: Principal,
    Path(id): Path<String>,
    Query(q): Query<UploadQuery>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<(StatusCode, Json<NodeView>)> {
    let dir = authz::load(&state, &p, &resolve_alias(&p, &id), Perm::Write).await?;
    let segs = match (&q.path, &q.name) {
        (Some(path), _) => nodes::split_rel_path(path)?,
        (None, Some(name)) => {
            nodes::validate_name(name)?;
            vec![name.clone()]
        }
        _ => return Err(ApiError::bad_request("需要 name 或 path 參數")),
    };
    do_upload(
        &state,
        &p,
        &dir,
        &segs,
        q.on_conflict,
        q.mtime,
        &headers,
        body,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn do_upload(
    state: &AppState,
    p: &Principal,
    dir: &Node,
    segs: &[String],
    policy: ConflictPolicy,
    mtime: Option<i64>,
    headers: &HeaderMap,
    body: Body,
) -> ApiResult<(StatusCode, Json<NodeView>)> {
    if !dir.is_dir() {
        return Err(ApiError::bad_request("目標不是資料夾"));
    }
    let (dirs, name) = segs.split_at(segs.len() - 1);
    let name = &name[0];
    let max = state.cfg.max_upload_bytes;
    let too_large =
        || ApiError::too_large("upload_too_large", format!("檔案超過上傳上限 {max} 位元組"));
    if headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .is_some_and(|l| l > max)
    {
        return Err(too_large());
    }
    // Fail fast on an obvious name conflict before receiving the body.
    if policy == ConflictPolicy::Fail {
        let mut cur = dir.id.clone();
        let mut exists = true;
        for d in dirs {
            match nodes::child_by_name(&state.db, &cur, d).await? {
                Some(n) => cur = n.id,
                None => {
                    exists = false;
                    break;
                }
            }
        }
        if exists && nodes::child_by_name(&state.db, &cur, name).await?.is_some() {
            return Err(
                ApiError::conflict(format!("「{name}」已存在；未覆寫既有檔案"))
                    .with_details(json!({ "name": name })),
            );
        }
    }

    let reader = StreamReader::new(body.into_data_stream().map_err(std::io::Error::other));
    let staged = match state.store.stage(reader, max).await {
        Ok(s) => s,
        Err(StoreError::TooLarge(_)) => return Err(too_large()),
        Err(StoreError::Io(e)) => {
            tracing::warn!(error = %e, "upload interrupted");
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "upload_incomplete",
                "上傳中斷，未儲存任何內容",
            ));
        }
    };
    if let Some(expected) = headers
        .get("x-content-sha256")
        .and_then(|v| v.to_str().ok())
        && !expected.trim().eq_ignore_ascii_case(&staged.sha256)
    {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "checksum_mismatch",
            "SHA-256 不符，檔案未儲存",
        )
        .with_details(json!({ "expected": expected.trim(), "actual": staged.sha256 })));
    }
    let (size, sha) = (staged.size, staged.sha256.clone());
    let parent = nodes::ensure_dirs(&state.db, dir, dirs).await?;
    let blob_id = state.store.commit(staged).await?;
    let sens = sensitive::classify_name(name).is_some();
    let res = nodes::insert_file(
        &state.db,
        &parent,
        nodes::NewFile {
            name,
            size,
            sha256: &sha,
            blob_id: &blob_id,
            mime: &guess_mime(name),
            sensitive: sens,
            modified_at: mtime.filter(|t| *t > 0),
        },
        policy,
    )
    .await;
    match res {
        Ok(n) => {
            state.poke_rag();
            Ok((
                StatusCode::CREATED,
                Json(view_with_path(state, p, &n).await?),
            ))
        }
        Err(e) => {
            state.store.remove_blob(&blob_id).await;
            Err(e)
        }
    }
}

/// Replace a file's content (used by the editor). Requires `If-Match: "<sha256>"`
/// of the version being replaced, so concurrent edits are never silently lost:
/// a stale hash yields 412. The new content is staged and committed atomically;
/// the old blob is queued for GC in the same transaction.
pub async fn put_content(
    State(state): State<AppState>,
    p: Principal,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<Json<NodeView>> {
    let n = authz::load(&state, &p, &resolve_alias(&p, &id), Perm::Write).await?;
    if !n.is_file() {
        return Err(ApiError::bad_request("只能覆寫檔案內容"));
    }
    let expected = headers
        .get(header::IF_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.trim()
                .trim_start_matches("W/")
                .trim_matches('"')
                .to_ascii_lowercase()
        })
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::PRECONDITION_REQUIRED,
                "precondition_required",
                "覆寫內容需要 If-Match 標頭（目前版本的 SHA-256）",
            )
        })?;
    let stale = || {
        ApiError::new(
            StatusCode::PRECONDITION_FAILED,
            "version_conflict",
            "檔案已被其他地方修改，請重新載入後再儲存",
        )
    };
    if n.sha256.as_deref() != Some(expected.as_str()) {
        return Err(stale());
    }
    let max = state.cfg.max_upload_bytes;
    let reader = StreamReader::new(body.into_data_stream().map_err(std::io::Error::other));
    let staged = match state.store.stage(reader, max).await {
        Ok(s) => s,
        Err(StoreError::TooLarge(_)) => {
            return Err(ApiError::too_large(
                "upload_too_large",
                format!("內容超過上限 {max} 位元組"),
            ));
        }
        Err(StoreError::Io(_)) => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "upload_incomplete",
                "傳輸中斷，檔案未變更",
            ));
        }
    };
    let (size, sha) = (staged.size, staged.sha256.clone());
    let blob = state.store.commit(staged).await?;
    let mut tx = state.db.begin().await?;
    let res = sqlx::query(
        "UPDATE nodes SET blob_id = ?, sha256 = ?, size = ?, modified_at = ? WHERE id = ? AND sha256 = ?",
    )
    .bind(&blob)
    .bind(&sha)
    .bind(size as i64)
    .bind(crate::util::now())
    .bind(&n.id)
    .bind(&expected)
    .execute(&mut *tx)
    .await?;
    if res.rows_affected() == 0 {
        drop(tx);
        state.store.remove_blob(&blob).await;
        return Err(stale());
    }
    sqlx::query("INSERT OR IGNORE INTO blob_gc (blob_id, queued_at) VALUES (?, ?)")
        .bind(n.blob_id.as_deref().unwrap_or_default())
        .bind(crate::util::now())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    state.poke_rag();
    let n = nodes::get(&state.db, &n.id).await?;
    Ok(Json(view_with_path(&state, &p, &n).await?))
}
