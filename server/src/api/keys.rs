use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    auth::{self, AdminSession, ApiKey},
    error::{ApiError, ApiResult},
    nodes,
    util::{now, opt_ts, ts_to_rfc3339},
    AppState,
};

async fn view(state: &AppState, k: &ApiKey) -> ApiResult<Value> {
    let scope = nodes::get_opt(&state.db, &k.scope_node_id).await?;
    let scope_path = match &scope {
        Some(n) => Some(nodes::path_of(&state.db, &n.id, nodes::ROOT_ID).await?),
        None => None,
    };
    let t = now();
    let status = if k.revoked_at.is_some() {
        "revoked"
    } else if k.expires_at.is_some_and(|e| e <= t) {
        "expired"
    } else if scope.is_none() {
        "scope_deleted"
    } else {
        "active"
    };
    Ok(json!({
        "id": k.id,
        "name": k.name,
        "prefix": format!("{}{}_…", auth::KEY_PREFIX, k.id),
        "permissions": { "list": k.can_list, "read": k.can_read, "write": k.can_write },
        "allow_sensitive": k.allow_sensitive,
        "scope": {
            "id": k.scope_node_id,
            "kind": scope.as_ref().map(|n| n.kind.clone()),
            "path": scope_path,
            "covers": if scope.as_ref().is_some_and(|n| n.is_dir()) {
                "folder and all descendants (any depth, evaluated at request time)"
            } else { "this single file" },
        },
        "status": status,
        "created_at": ts_to_rfc3339(k.created_at),
        "expires_at": opt_ts(k.expires_at),
        "last_used_at": opt_ts(k.last_used_at),
        "revoked_at": opt_ts(k.revoked_at),
    }))
}

pub async fn list(State(state): State<AppState>, _s: AdminSession) -> ApiResult<Json<Value>> {
    let keys: Vec<ApiKey> = sqlx::query_as("SELECT * FROM api_keys ORDER BY created_at DESC").fetch_all(&state.db).await?;
    let mut out = Vec::new();
    for k in &keys {
        out.push(view(&state, k).await?);
    }
    Ok(Json(json!({ "keys": out })))
}

#[derive(Deserialize)]
pub struct Perms {
    #[serde(default)]
    list: bool,
    #[serde(default)]
    read: bool,
    #[serde(default)]
    write: bool,
}

#[derive(Deserialize)]
pub struct CreateBody {
    name: String,
    permissions: Perms,
    /// Defaults to the drive root.
    scope_node_id: Option<String>,
    /// RFC 3339 timestamp; omit / null for no expiry.
    expires_at: Option<String>,
    #[serde(default)]
    allow_sensitive: bool,
}

pub async fn create(
    State(state): State<AppState>,
    _s: AdminSession,
    Json(b): Json<CreateBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let name = b.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(ApiError::bad_request("名稱需為 1–100 字"));
    }
    if !(b.permissions.list || b.permissions.read || b.permissions.write) {
        return Err(ApiError::bad_request("至少需要一項權限"));
    }
    let scope = b.scope_node_id.unwrap_or_else(|| nodes::ROOT_ID.to_string());
    nodes::get_opt(&state.db, &scope).await?.ok_or_else(|| ApiError::bad_request("授權範圍不存在"))?;
    let expires_at = match b.expires_at.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => {
            let t = chrono::DateTime::parse_from_rfc3339(s)
                .map_err(|_| ApiError::bad_request("expires_at 需為 RFC 3339 時間"))?
                .timestamp();
            if t <= now() {
                return Err(ApiError::bad_request("到期時間必須在未來"));
            }
            Some(t)
        }
        None => None,
    };
    let (full, id, hash) = auth::generate_key();
    sqlx::query(
        "INSERT INTO api_keys (id, name, secret_hash, can_list, can_read, can_write, allow_sensitive, scope_node_id, created_at, expires_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(name)
    .bind(&hash)
    .bind(b.permissions.list)
    .bind(b.permissions.read)
    .bind(b.permissions.write)
    .bind(b.allow_sensitive)
    .bind(&scope)
    .bind(now())
    .bind(expires_at)
    .execute(&state.db)
    .await?;
    let k: ApiKey = sqlx::query_as("SELECT * FROM api_keys WHERE id = ?").bind(&id).fetch_one(&state.db).await?;
    tracing::info!(key_id = %id, "api key created");
    Ok((StatusCode::CREATED, Json(json!({ "key": full, "api_key": view(&state, &k).await? }))))
}

pub async fn revoke(State(state): State<AppState>, _s: AdminSession, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let res = sqlx::query("UPDATE api_keys SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
        .bind(now())
        .bind(&id)
        .execute(&state.db)
        .await?;
    let k: Option<ApiKey> = sqlx::query_as("SELECT * FROM api_keys WHERE id = ?").bind(&id).fetch_optional(&state.db).await?;
    let k = k.ok_or_else(ApiError::not_found)?;
    if res.rows_affected() > 0 {
        tracing::info!(key_id = %id, "api key revoked");
    }
    Ok(Json(view(&state, &k).await?))
}
