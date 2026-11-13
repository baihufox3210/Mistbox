//! Share links.
//!
//! - Admin (session only): create / list / change / delete links.
//! - Link holders: `POST /share/open` with `X-Share-Token` checks the link and
//!   sets a per-share HttpOnly cookie used by the browser's own GET requests
//!   (images, video, downloads). Every other request carries the header.
//!   Everything else goes through the normal endpoints with the same
//!   scope / permission checks as API keys (`Principal::grant`).

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use crate::{
    AppState,
    auth::{self, AdminSession, Principal, Share},
    error::{ApiError, ApiResult},
    nodes::{self, NodeView},
    util::{now, opt_ts, ts_to_rfc3339},
};

async fn view(state: &AppState, s: &Share) -> ApiResult<Value> {
    let node = nodes::get_opt(&state.db, &s.node_id).await?;
    let (node_json, unavailable) = match &node {
        Some(n) => {
            let path = nodes::path_of(&state.db, &n.id, nodes::ROOT_ID).await?;
            let hidden = n.sensitive || nodes::in_trash(&state.db, &n.id).await?;
            (
                json!({ "id": n.id, "name": n.name, "kind": n.kind, "path": path }),
                hidden,
            )
        }
        None => (Value::Null, true),
    };
    let status = if s.expires_at.is_some_and(|e| e <= now()) {
        "expired"
    } else if unavailable {
        "unavailable"
    } else {
        "active"
    };
    Ok(json!({
        "id": s.id,
        "node": node_json,
        "permission": if s.can_write { "edit" } else { "view" },
        "label": s.label,
        "status": status,
        "created_at": ts_to_rfc3339(s.created_at),
        "expires_at": opt_ts(s.expires_at),
        "last_used_at": opt_ts(s.last_used_at),
        "use_count": s.use_count,
    }))
}

fn parse_expiry(s: Option<&str>) -> ApiResult<Option<i64>> {
    match s.filter(|s| !s.is_empty()) {
        Some(s) => {
            let t = chrono::DateTime::parse_from_rfc3339(s)
                .map_err(|_| ApiError::bad_request("expires_at 需為 RFC 3339 時間"))?
                .timestamp();
            if t <= now() {
                return Err(ApiError::bad_request("到期時間必須在未來"));
            }
            Ok(Some(t))
        }
        None => Ok(None),
    }
}

fn permission(p: &str) -> ApiResult<bool> {
    match p {
        "view" => Ok(false),
        "edit" => Ok(true),
        _ => Err(ApiError::bad_request("permission 需為 view 或 edit")),
    }
}

fn label(l: Option<&str>) -> ApiResult<String> {
    let l = l.unwrap_or("").trim();
    if l.chars().count() > 100 {
        return Err(ApiError::bad_request("備註最多 100 字"));
    }
    Ok(l.to_string())
}

#[derive(Deserialize)]
pub struct ListQuery {
    node_id: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    _s: AdminSession,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    let rows: Vec<Share> = match &q.node_id {
        Some(id) => {
            sqlx::query_as("SELECT * FROM shares WHERE node_id = ? ORDER BY created_at DESC")
                .bind(id)
                .fetch_all(&state.db)
                .await?
        }
        None => {
            sqlx::query_as("SELECT * FROM shares ORDER BY created_at DESC")
                .fetch_all(&state.db)
                .await?
        }
    };
    let mut out = Vec::with_capacity(rows.len());
    for s in &rows {
        out.push(view(&state, s).await?);
    }
    Ok(Json(json!({ "shares": out })))
}

#[derive(Deserialize)]
pub struct CreateBody {
    node_id: String,
    /// "view" (default) or "edit".
    #[serde(default)]
    permission: Option<String>,
    /// RFC 3339; omit / null = never expires.
    expires_at: Option<String>,
    label: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    _s: AdminSession,
    Json(b): Json<CreateBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let node = nodes::get_opt(&state.db, &b.node_id)
        .await?
        .ok_or_else(|| ApiError::bad_request("要分享的項目不存在"))?;
    if node.id == nodes::TRASH_ID || nodes::in_trash(&state.db, &node.id).await? {
        return Err(ApiError::bad_request("垃圾桶中的項目不能分享"));
    }
    if node.sensitive {
        return Err(ApiError::bad_request(
            "標記為敏感的項目不能分享（分享連結一律看不到敏感檔案）",
        ));
    }
    let can_write = permission(b.permission.as_deref().unwrap_or("view"))?;
    let expires_at = parse_expiry(b.expires_at.as_deref())?;
    let label = label(b.label.as_deref())?;
    let (id, secret, hash) = auth::generate_share();
    sqlx::query(
        "INSERT INTO shares (id, node_id, secret_hash, can_write, label, created_at, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&node.id)
    .bind(&hash)
    .bind(can_write)
    .bind(&label)
    .bind(now())
    .bind(expires_at)
    .execute(&state.db)
    .await?;
    tracing::info!(share = %id, node = %node.id, can_write, "share link created");
    let s: Share = sqlx::query_as("SELECT * FROM shares WHERE id = ?")
        .bind(&id)
        .fetch_one(&state.db)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            // Relative link; the secret is never stored and shown only now.
            "link": format!("/s/{id}#{secret}"),
            "share": view(&state, &s).await?,
        })),
    ))
}

/// Distinguishes "field absent" from "explicit null" (null = never expires).
fn some_nullable<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Ok(Some(Option::deserialize(d)?))
}

#[derive(Deserialize)]
pub struct UpdateBody {
    permission: Option<String>,
    #[serde(default, deserialize_with = "some_nullable")]
    expires_at: Option<Option<String>>,
    label: Option<String>,
}

async fn load(state: &AppState, id: &str) -> ApiResult<Share> {
    sqlx::query_as("SELECT * FROM shares WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(ApiError::not_found)
}

pub async fn update(
    State(state): State<AppState>,
    _s: AdminSession,
    Path(id): Path<String>,
    Json(b): Json<UpdateBody>,
) -> ApiResult<Json<Value>> {
    let s = load(&state, &id).await?;
    let can_write = match &b.permission {
        Some(p) => permission(p)?,
        None => s.can_write,
    };
    let expires_at = match &b.expires_at {
        Some(e) => parse_expiry(e.as_deref())?,
        None => s.expires_at,
    };
    let label = match &b.label {
        Some(l) => label(Some(l))?,
        None => s.label.clone(),
    };
    sqlx::query("UPDATE shares SET can_write = ?, expires_at = ?, label = ? WHERE id = ?")
        .bind(can_write)
        .bind(expires_at)
        .bind(&label)
        .bind(&id)
        .execute(&state.db)
        .await?;
    Ok(Json(view(&state, &load(&state, &id).await?).await?))
}

pub async fn remove(
    State(state): State<AppState>,
    _s: AdminSession,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let r = sqlx::query("DELETE FROM shares WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await?;
    if r.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    tracing::info!(share = %id, "share link deleted");
    Ok(StatusCode::NO_CONTENT)
}

/// Link holder: validate the link, describe it, and set the media cookie.
pub async fn open(
    State(state): State<AppState>,
    p: Principal,
    headers: axum::http::HeaderMap,
) -> ApiResult<Response> {
    let Principal::Share(s) = &p else {
        return Err(ApiError::bad_request("需要 X-Share-Token 標頭"));
    };
    let root = nodes::get_opt(&state.db, &s.node_id)
        .await?
        .ok_or_else(auth::share_invalid)?;
    if root.sensitive || nodes::in_trash(&state.db, &root.id).await? {
        return Err(auth::share_invalid());
    }
    sqlx::query("UPDATE shares SET use_count = use_count + 1 WHERE id = ?")
        .bind(&s.id)
        .execute(&state.db)
        .await?;
    // The header was already verified; reuse its secret for the cookie.
    let secret = headers
        .get("x-share-token")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit_once('_').map(|(_, s)| s.to_string()))
        .ok_or_else(auth::share_invalid)?;
    let cookie = auth::share_cookie(state.cfg.cookie_secure, &s.id, &secret);
    let body = json!({
        "id": s.id,
        "permission": if s.can_write { "edit" } else { "view" },
        "label": s.label,
        "expires_at": opt_ts(s.expires_at),
        "root": NodeView::from_node(&root, &root.id).with_path("/".into()),
    });
    Ok(([(header::SET_COOKIE, cookie)], Json(body)).into_response())
}
