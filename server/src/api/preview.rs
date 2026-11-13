use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;

use super::{files, resolve_alias};
use crate::{
    auth::Principal,
    authz::{self, Perm},
    error::{ApiError, ApiResult},
    latex::{self, LatexClient},
    nodes::{self, Node},
    textview, AppState,
};

#[derive(Deserialize)]
pub struct TextQuery {
    encoding: Option<String>,
    /// Decode only the first `preview_max_bytes` of an over-limit file.
    #[serde(default, deserialize_with = "crate::util::flag")]
    partial: bool,
}

/// Decoded text for the code / Markdown / plain-text viewers.
pub async fn text(
    State(state): State<AppState>,
    p: Principal,
    Path(id): Path<String>,
    Query(q): Query<TextQuery>,
) -> ApiResult<Json<Value>> {
    let n = authz::load(&state, &p, &resolve_alias(&p, &id), Perm::Read).await?;
    if !n.is_file() {
        return Err(ApiError::bad_request("不是檔案"));
    }
    let limit = state.cfg.preview_max_bytes;
    let size = n.size as u64;
    if size > limit && !q.partial {
        return Err(ApiError::too_large("preview_too_large", "檔案超過預覽大小限制，可下載或只載入開頭部分")
            .with_details(json!({ "size": size, "limit": limit })));
    }
    let f = tokio::fs::File::open(state.store.blob_path(n.blob_id.as_deref().unwrap_or_default())).await?;
    let mut buf = Vec::with_capacity(size.min(limit) as usize);
    f.take(limit).read_to_end(&mut buf).await?;
    let out = textview::decode(&buf, q.encoding.as_deref(), size, limit, size > limit)
        .map_err(ApiError::bad_request)?;
    let mut v = serde_json::to_value(out).unwrap();
    v["encodings"] = json!(textview::CHOICES);
    Ok(Json(v))
}

#[derive(Deserialize)]
pub struct RelQuery {
    path: String,
}

/// Directory against which relative links in `n` are resolved.
async fn base_dir(state: &AppState, p: &Principal, n: &Node) -> ApiResult<String> {
    authz::require_info(state, p, n).await?;
    if n.is_dir() {
        Ok(n.id.clone())
    } else {
        n.parent_id.clone().ok_or_else(ApiError::not_found)
    }
}

async fn resolve_from(state: &AppState, p: &Principal, id: &str, rel: &str) -> ApiResult<Node> {
    let n = nodes::get(&state.db, &resolve_alias(p, id)).await?;
    let base = base_dir(state, p, &n).await?;
    let rel = rel.split(['?', '#']).next().unwrap_or("");
    // A relative reference may not climb above the caller's root.
    let target = nodes::resolve_rel(&state.db, &base, rel, p.root_id()).await?;
    if !authz::visible(state, p, &target).await? {
        return Err(ApiError::not_found());
    }
    Ok(target)
}

/// Resolve a relative link (e.g. from Markdown) to node info.
pub async fn resolve(
    State(state): State<AppState>,
    p: Principal,
    Path(id): Path<String>,
    Query(q): Query<RelQuery>,
) -> ApiResult<Json<Value>> {
    let target = resolve_from(&state, &p, &id, &q.path).await?;
    files::info_json(&state, &p, &target, false).await
}

/// Serve a relatively referenced resource (Markdown images) inline,
/// re-checking read permission on the *target*.
pub async fn relative_content(
    State(state): State<AppState>,
    p: Principal,
    Path(id): Path<String>,
    Query(q): Query<RelQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let target = resolve_from(&state, &p, &id, &q.path).await?;
    authz::require(&state, &p, &target, Perm::Read).await?;
    files::serve_file(&state, &target, true, &headers).await
}

async fn latex_main(state: &AppState, p: &Principal, id: &str) -> ApiResult<(Node, std::sync::Arc<LatexClient>)> {
    let n = authz::load(state, p, &resolve_alias(p, id), Perm::Read).await?;
    if !n.is_file() || !n.name.to_ascii_lowercase().ends_with(".tex") {
        return Err(ApiError::bad_request("只能編譯 .tex 檔案"));
    }
    let client = state.latex.clone().ok_or_else(|| {
        ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "latex_disabled", "此伺服器未啟用 LaTeX 編譯服務")
    })?;
    Ok((n, client))
}

/// Compile a .tex file in the isolated worker. On success the PDF is cached
/// (keyed by engine + every input's SHA-256) and served by `latex_pdf`.
pub async fn latex_compile(State(state): State<AppState>, p: Principal, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let (main, client) = latex_main(&state, &p, &id).await?;
    let collected = latex::collect(&state, &p, &main, client.max_input()).await?;
    let cached = tokio::fs::metadata(LatexClient::cache_path(&state, &collected.cache_key)).await.is_ok();
    client.compile(&state, &main, &collected).await?;
    Ok(Json(json!({
        "ok": true,
        "cached": cached,
        "engine": collected.engine,
        "files": collected.files,
        "missing": collected.missing,
        "pdf_url": format!("/api/v1/nodes/{}/latex/pdf", main.id),
    })))
}

pub async fn latex_pdf(State(state): State<AppState>, p: Principal, Path(id): Path<String>) -> ApiResult<Response> {
    let (main, client) = latex_main(&state, &p, &id).await?;
    let collected = latex::collect(&state, &p, &main, client.max_input()).await?;
    let path = LatexClient::cache_path(&state, &collected.cache_key);
    let bytes = tokio::fs::read(&path).await.map_err(|_| {
        ApiError::new(StatusCode::NOT_FOUND, "latex_not_compiled", "尚未編譯目前版本，請先執行編譯")
    })?;
    let pdf_name = format!("{}.pdf", main.name.trim_end_matches(".tex"));
    let mut r = bytes.into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/pdf"));
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&crate::util::content_disposition("inline", &pdf_name)).unwrap(),
    );
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(super::CONTENT_CSP));
    Ok(r)
}
