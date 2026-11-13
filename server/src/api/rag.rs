use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    auth::{AdminSession, Principal},
    error::{ApiError, ApiResult},
    rag::Rag,
    AppState,
};

fn rag(state: &AppState) -> ApiResult<std::sync::Arc<Rag>> {
    state.rag.clone().ok_or_else(|| {
        ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "rag_disabled", "此伺服器未啟用語意搜尋（MISTBOX_RAG_MODEL=off）")
    })
}

fn require_read_flag(p: &Principal) -> ApiResult<()> {
    if let Principal::Key(k) = p
        && !k.can_read {
            return Err(ApiError::forbidden("語意搜尋會回傳檔案內容片段，需要 read 權限"));
        }
    Ok(())
}

pub async fn status(State(state): State<AppState>, _p: Principal) -> ApiResult<Json<Value>> {
    match &state.rag {
        None => Ok(Json(json!({ "enabled": false }))),
        Some(r) => Ok(Json(serde_json::to_value(r.status(&state.db).await?).unwrap())),
    }
}

#[derive(Deserialize)]
pub struct SearchQuery {
    q: String,
    k: Option<usize>,
    max_chars: Option<usize>,
}

async fn run(state: &AppState, p: &Principal, q: &SearchQuery) -> ApiResult<Vec<crate::rag::Hit>> {
    let r = rag(state)?;
    require_read_flag(p)?;
    let query = q.q.trim();
    if query.is_empty() || query.chars().count() > 2000 {
        return Err(ApiError::bad_request("查詢需為 1–2000 字"));
    }
    r.search(state, p, query, q.k.unwrap_or(10).clamp(1, 50)).await.map_err(|e| {
        ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "rag_not_ready", e.to_string())
    })
}

/// Semantic search: top chunks (max 3 per file), each with path, line range and score.
pub async fn search(State(state): State<AppState>, p: Principal, Query(q): Query<SearchQuery>) -> ApiResult<Json<Value>> {
    let hits = run(&state, &p, &q).await?;
    Ok(Json(json!({ "query": q.q, "results": hits })))
}

/// Retrieval context for an external / local LLM: numbered, cited passages.
pub async fn context(State(state): State<AppState>, p: Principal, Query(q): Query<SearchQuery>) -> ApiResult<Json<Value>> {
    let hits = run(&state, &p, &q).await?;
    let budget = q.max_chars.unwrap_or(6000).clamp(200, 50_000);
    let mut used = 0;
    let mut ctx = String::new();
    let mut sources = Vec::new();
    for (i, h) in hits.iter().enumerate() {
        let path = h.node.path.clone().unwrap_or_default();
        let block = format!("[{}] {}:{}-{}\n{}\n\n", i + 1, path, h.line_start, h.line_end, h.text);
        if used + block.chars().count() > budget && !sources.is_empty() {
            break;
        }
        used += block.chars().count();
        ctx.push_str(&block);
        sources.push(json!({ "n": i + 1, "id": h.node.id, "path": path, "line_start": h.line_start, "line_end": h.line_end, "score": h.score }));
    }
    Ok(Json(json!({ "query": q.q, "context": ctx.trim_end(), "sources": sources })))
}

pub async fn reindex(State(state): State<AppState>, _s: AdminSession) -> ApiResult<Json<Value>> {
    let r = rag(&state)?;
    r.reset(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}
