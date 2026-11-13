//! Path-based convenience endpoints for scripts (`curl`). Paths are POSIX
//! style and relative to the caller's root: the drive root for the admin,
//! the scope folder for an API key. They map onto the same authorization
//! checks as the ID-based endpoints.

use axum::{
    body::Body,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    Json,
};
use serde::Deserialize;
use serde_json::Value;

use super::files::{self, UploadQuery};
use crate::{
    auth::Principal,
    authz::{self, Perm},
    error::{ApiError, ApiResult},
    nodes::{self, ConflictPolicy, Node, NodeView},
    AppState,
};

#[derive(Deserialize)]
pub struct PathQuery {
    #[serde(default)]
    path: String,
    #[serde(default, deserialize_with = "crate::util::flag")]
    stats: bool,
    #[serde(default, deserialize_with = "crate::util::flag")]
    inline: bool,
}

async fn resolve(state: &AppState, p: &Principal, path: &str) -> ApiResult<Node> {
    let root = p.root_id();
    if path.split('/').any(|s| s == "..") {
        return Err(ApiError::bad_request("路徑不可包含 .."));
    }
    let n = nodes::resolve_rel(&state.db, root, path, root).await?;
    if !authz::visible(state, p, &n).await? {
        return Err(ApiError::not_found());
    }
    Ok(n)
}

pub async fn list(State(state): State<AppState>, p: Principal, Query(q): Query<PathQuery>) -> ApiResult<Json<Value>> {
    let n = resolve(&state, &p, &q.path).await?;
    files::list_json(&state, &p, &n).await
}

pub async fn info(State(state): State<AppState>, p: Principal, Query(q): Query<PathQuery>) -> ApiResult<Json<Value>> {
    let n = resolve(&state, &p, &q.path).await?;
    files::info_json(&state, &p, &n, q.stats).await
}

pub async fn download(
    State(state): State<AppState>,
    p: Principal,
    Query(q): Query<PathQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let n = resolve(&state, &p, &q.path).await?;
    authz::require(&state, &p, &n, Perm::Read).await?;
    files::serve_file(&state, &n, q.inline, &headers).await
}

#[derive(Deserialize)]
pub struct FsUploadQuery {
    /// Destination path including the file name, e.g. `/reports/2026/q3.pdf`.
    path: String,
    #[serde(default)]
    on_conflict: ConflictPolicy,
    mtime: Option<i64>,
    /// Create missing parent folders (default true).
    #[serde(default, deserialize_with = "opt_flag")]
    mkdirs: Option<bool>,
}

pub async fn upload(
    State(state): State<AppState>,
    p: Principal,
    Query(q): Query<FsUploadQuery>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<(StatusCode, Json<NodeView>)> {
    let segs = nodes::split_rel_path(&q.path)?;
    let root = nodes::get(&state.db, p.root_id()).await?;
    authz::require(&state, &p, &root, Perm::Write).await?;
    let (base, rel): (Node, Vec<String>) = if q.mkdirs.unwrap_or(true) {
        (root, segs)
    } else {
        let parent_path = segs[..segs.len() - 1].join("/");
        (resolve(&state, &p, &parent_path).await?, vec![segs.last().unwrap().clone()])
    };
    authz::require(&state, &p, &base, Perm::Write).await?;
    let uq = UploadQuery { on_conflict: q.on_conflict, mtime: q.mtime, ..Default::default() };
    files::do_upload(&state, &p, &base, &rel, uq.on_conflict, uq.mtime, &headers, body).await
}

pub async fn mkdir(State(state): State<AppState>, p: Principal, Query(q): Query<PathQuery>) -> ApiResult<(StatusCode, Json<NodeView>)> {
    let segs = nodes::split_rel_path(&q.path)?;
    let root = nodes::get(&state.db, p.root_id()).await?;
    authz::require(&state, &p, &root, Perm::Write).await?;
    let n = nodes::ensure_dirs(&state.db, &root, &segs).await?;
    Ok((StatusCode::CREATED, Json(files::view_with_path(&state, &p, &n).await?)))
}

fn opt_flag<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<bool>, D::Error> {
    crate::util::flag(d).map(Some)
}
