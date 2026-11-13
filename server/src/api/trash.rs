use axum::{
    extract::{Path, State},
    Json,
};
use serde_json::{json, Value};

use crate::{
    auth::AdminSession,
    error::ApiResult,
    nodes::{self, NodeView},
    util::ts_to_rfc3339,
    AppState,
};

/// Recycle bin listing (admin only; API keys never see the bin).
pub async fn list(State(state): State<AppState>, _s: AdminSession) -> ApiResult<Json<Value>> {
    let days = state.cfg.trash_days;
    let mut out = Vec::new();
    for (e, n) in nodes::trash_entries(&state.db).await? {
        let mut v = serde_json::to_value(NodeView::from_node(&n, nodes::ROOT_ID)).unwrap();
        v["name"] = json!(e.original_name);
        v["path"] = json!(e.original_path);
        v["trashed_at"] = json!(ts_to_rfc3339(e.trashed_at));
        v["purge_at"] = if days > 0 { json!(ts_to_rfc3339(e.trashed_at + days * 86_400)) } else { json!(null) };
        if n.is_dir() {
            v["stats"] = serde_json::to_value(nodes::subtree_stats(&state.db, &n.id).await?).unwrap();
        }
        out.push(v);
    }
    Ok(Json(json!({ "items": out, "retention_days": days })))
}

pub async fn restore(State(state): State<AppState>, _s: AdminSession, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let (n, to_root) = nodes::restore(&state.db, &id).await?;
    state.poke_rag();
    let path = nodes::path_of(&state.db, &n.id, nodes::ROOT_ID).await?;
    Ok(Json(json!({ "node": NodeView::from_node(&n, nodes::ROOT_ID).with_path(path), "restored_to_root": to_root })))
}

/// Empty the bin (permanent).
pub async fn empty(State(state): State<AppState>, _s: AdminSession) -> ApiResult<Json<Value>> {
    let n = nodes::purge_trash(&state.db, 0).await?;
    Ok(Json(json!({ "deleted": n })))
}
