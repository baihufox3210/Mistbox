use std::time::Duration;

use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    auth::{self, AdminSession, Principal},
    error::{ApiError, ApiResult},
    nodes,
    util::{opt_ts, sha256_hex},
    AppState,
};

#[derive(Deserialize)]
pub struct LoginBody {
    username: String,
    password: String,
}

fn login_response(state: &AppState, username: &str, s: auth::NewSession) -> Response {
    let cookie = auth::session_cookie(state.cfg.cookie_secure, &s.token, state.cfg.session_ttl_hours * 3600);
    (
        [(header::SET_COOKIE, cookie)],
        Json(json!({ "username": username, "csrf_token": s.csrf })),
    )
        .into_response()
}

pub async fn login(State(state): State<AppState>, req: Request) -> ApiResult<Response> {
    let ip = auth::client_ip(req.headers(), req.extensions(), state.cfg.trust_proxy);
    let limit_key = format!("login:{ip}");
    if let Err(retry) = state.limiter.hit(&limit_key, state.cfg.login_attempts, Duration::from_secs(15 * 60)) {
        return Err(auth::rate_limited(retry));
    }
    let bytes = axum::body::to_bytes(req.into_body(), 64 * 1024)
        .await
        .map_err(|_| ApiError::bad_request("請求內容過大"))?;
    let body: LoginBody = serde_json::from_slice(&bytes).map_err(|_| ApiError::bad_request("需要 username 與 password"))?;
    let Some(user) = auth::check_login(&state.db, body.username.trim(), &body.password).await? else {
        return Err(ApiError::new(StatusCode::UNAUTHORIZED, "invalid_credentials", "帳號或密碼錯誤"));
    };
    state.limiter.reset(&limit_key);
    let s = auth::create_session(&state.db, user.id, state.cfg.session_ttl_hours).await?;
    tracing::info!(user = %user.username, "login");
    Ok(login_response(&state, &user.username, s))
}

pub async fn logout(State(state): State<AppState>, _s: AdminSession, req: Request) -> ApiResult<Response> {
    let (parts, _) = req.into_parts();
    if let Some(tok) = auth::session_token_from_headers(&parts, state.cfg.cookie_secure) {
        sqlx::query("DELETE FROM sessions WHERE token_hash = ?").bind(sha256_hex(&tok)).execute(&state.db).await?;
    }
    let cookie = auth::session_cookie(state.cfg.cookie_secure, "", 0);
    Ok(([(header::SET_COOKIE, cookie)], StatusCode::NO_CONTENT).into_response())
}

pub async fn me(s: AdminSession) -> Json<serde_json::Value> {
    Json(json!({ "username": s.username, "csrf_token": s.csrf }))
}

#[derive(Deserialize)]
pub struct PasswordBody {
    current_password: String,
    new_password: String,
}

pub async fn change_password(
    State(state): State<AppState>,
    s: AdminSession,
    Json(body): Json<PasswordBody>,
) -> ApiResult<Response> {
    if auth::check_login(&state.db, &s.username, &body.current_password).await?.is_none() {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "invalid_credentials", "目前密碼不正確"));
    }
    auth::check_password_policy(&body.new_password).map_err(ApiError::bad_request)?;
    auth::set_password(&state.db, s.user_id, &body.new_password).await?;
    let ns = auth::create_session(&state.db, s.user_id, state.cfg.session_ttl_hours).await?;
    Ok(login_response(&state, &s.username, ns))
}

/// Works for both sessions and API keys: describes the caller.
pub async fn whoami(State(state): State<AppState>, p: Principal) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(match &p {
        Principal::Admin { username, .. } => json!({ "type": "session", "username": username }),
        Principal::Key(k) => {
            let scope = nodes::get_opt(&state.db, &k.scope_node_id).await?;
            json!({
                "type": "api_key",
                "id": k.id,
                "name": k.name,
                "permissions": { "list": k.can_list, "read": k.can_read, "write": k.can_write },
                "allow_sensitive": k.allow_sensitive,
                "scope": scope.map(|n| json!({ "id": n.id, "kind": n.kind, "name": n.name })),
                "expires_at": opt_ts(k.expires_at),
            })
        }
    }))
}
