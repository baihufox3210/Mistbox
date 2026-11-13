//! Code index: static-analysis findings, duplicate code, symbols.
//!
//! Results are derived from file content, so they need `read` permission and
//! stay inside the caller's scope. Share links have no access (like semantic search).

use std::collections::HashMap;

use axum::{
    Json,
    extract::{Query, State},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::files::view_with_path;
use crate::{
    AppState,
    auth::{AdminSession, Principal},
    authz::{self, Perm},
    code::{self, index},
    error::{ApiError, ApiResult},
    nodes::{self, Node, NodeView},
};

fn require_read_flag(p: &Principal) -> ApiResult<()> {
    if matches!(p, Principal::Share(_)) {
        return Err(ApiError::forbidden("分享連結不提供程式碼分析"));
    }
    if let Principal::Key(k) = p
        && !k.can_read
    {
        return Err(ApiError::forbidden(
            "程式碼分析結果來自檔案內容，需要 read 權限",
        ));
    }
    Ok(())
}

/// The node to look at (default: the caller's root), checked for read access.
async fn target(state: &AppState, p: &Principal, id: Option<&str>) -> ApiResult<Node> {
    require_read_flag(p)?;
    let id = super::resolve_alias(p, id.unwrap_or("root"));
    let n = nodes::get(&state.db, &id).await?;
    authz::require(
        state,
        p,
        &n,
        if n.is_dir() { Perm::List } else { Perm::Read },
    )
    .await?;
    Ok(n)
}

/// Node views (with paths) for result rows, looked up once per node.
async fn views(
    state: &AppState,
    p: &Principal,
    ids: impl Iterator<Item = String>,
) -> ApiResult<HashMap<String, NodeView>> {
    let mut out = HashMap::new();
    for id in ids {
        if out.contains_key(&id) {
            continue;
        }
        if let Some(n) = nodes::get_opt(&state.db, &id).await? {
            out.insert(id, view_with_path(state, p, &n).await?);
        }
    }
    Ok(out)
}

pub async fn status(State(state): State<AppState>, p: Principal) -> ApiResult<Json<Value>> {
    require_read_flag(&p)?;
    Ok(Json(
        serde_json::to_value(index::status(&state.db, &state.code).await?).unwrap(),
    ))
}

/// The rule catalogue (ids, severity, category, title, how to fix).
pub async fn rules(_p: Principal) -> Json<Value> {
    Json(json!({ "rules": code::RULES }))
}

#[derive(Deserialize)]
pub struct FindingsQuery {
    node_id: Option<String>,
    severity: Option<String>,
    limit: Option<i64>,
}

/// Findings in a file, or in every file below a folder.
pub async fn findings(
    State(state): State<AppState>,
    p: Principal,
    Query(q): Query<FindingsQuery>,
) -> ApiResult<Json<Value>> {
    let n = target(&state, &p, q.node_id.as_deref()).await?;
    let sev = q.severity.as_deref().filter(|s| !s.is_empty());
    if let Some(s) = sev
        && !matches!(s, "error" | "warning" | "perf" | "info")
    {
        return Err(ApiError::bad_request(
            "severity 需為 error、warning、perf 或 info",
        ));
    }
    let rows = index::findings(&state.db, &n, sev, q.limit.unwrap_or(500).clamp(1, 5000)).await?;
    let files = views(&state, &p, rows.iter().map(|r| r.node_id.clone())).await?;
    let out: Vec<Value> = rows
        .iter()
        .filter_map(|r| {
            let f = files.get(&r.node_id)?;
            let mut v = serde_json::to_value(r).unwrap();
            v["path"] = json!(f.path);
            v["name"] = json!(f.name);
            Some(v)
        })
        .collect();
    // For a file: whether the results match its current content.
    let index = if n.is_file() {
        serde_json::to_value(index::file_state(&state.db, &n.id).await?).unwrap()
    } else {
        Value::Null
    };
    Ok(Json(
        json!({ "node": view_with_path(&state, &p, &n).await?, "index": index, "findings": out }),
    ))
}

#[derive(Deserialize)]
pub struct NodeQuery {
    node_id: Option<String>,
    limit: Option<usize>,
}

/// Duplicate code. For a file: every place (within the caller's scope) that
/// repeats part of it. For a folder: the largest clones between its files.
pub async fn duplicates(
    State(state): State<AppState>,
    p: Principal,
    Query(q): Query<NodeQuery>,
) -> ApiResult<Json<Value>> {
    let n = target(&state, &p, q.node_id.as_deref()).await?;
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let mut clones = if n.is_file() {
        index::clones_of(&state.db, &n.id, p.root_id()).await?
    } else {
        index::clones_in(&state.db, &n.id, limit).await?
    };
    clones.truncate(limit);
    let files = views(
        &state,
        &p,
        clones
            .iter()
            .flat_map(|c| [c.node_id.clone(), c.other_id.clone()]),
    )
    .await?;
    let out: Vec<Value> = clones
        .iter()
        .filter_map(|c| {
            let a = files.get(&c.node_id)?;
            let b = files.get(&c.other_id)?;
            Some(json!({
                "a": { "id": a.id, "name": a.name, "path": a.path, "line_start": c.region.a_start, "line_end": c.region.a_end },
                "b": { "id": b.id, "name": b.name, "path": b.path, "line_start": c.region.b_start, "line_end": c.region.b_end },
                "tokens": c.region.tokens,
            }))
        })
        .collect();
    Ok(Json(
        json!({ "node": view_with_path(&state, &p, &n).await?, "duplicates": out }),
    ))
}

#[derive(Deserialize)]
pub struct SymbolQuery {
    q: String,
    limit: Option<i64>,
}

/// Where a function / type / macro / PIO program is defined.
pub async fn symbols(
    State(state): State<AppState>,
    p: Principal,
    Query(q): Query<SymbolQuery>,
) -> ApiResult<Json<Value>> {
    let root = target(&state, &p, None).await?;
    let name = q.q.trim();
    if name.is_empty() || name.len() > 200 {
        return Err(ApiError::bad_request("請輸入 1–200 字的名稱"));
    }
    let rows = index::symbols(
        &state.db,
        name,
        &root.id,
        q.limit.unwrap_or(50).clamp(1, 500),
    )
    .await?;
    let files = views(&state, &p, rows.iter().map(|r| r.node_id.clone())).await?;
    let out: Vec<Value> = rows
        .iter()
        .filter_map(|r| {
            let f = files.get(&r.node_id)?;
            let mut v = serde_json::to_value(r).unwrap();
            v["path"] = json!(f.path);
            Some(v)
        })
        .collect();
    Ok(Json(json!({ "symbols": out })))
}

/// Re-analyse everything (e.g. after upgrading). Admin session only.
pub async fn reindex(State(state): State<AppState>, _s: AdminSession) -> ApiResult<Json<Value>> {
    sqlx::query("DELETE FROM code_files")
        .execute(&state.db)
        .await?;
    state.code.poke();
    Ok(Json(json!({ "ok": true })))
}
