//! Authentication: Argon2id passwords, cookie sessions with CSRF tokens,
//! API keys (Bearer), share links, and a small in-memory rate limiter.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::{Method, StatusCode, header, request::Parts},
};
use sqlx::SqlitePool;

use crate::{
    AppState,
    error::{ApiError, ApiResult},
    util::{ct_eq, now, random_hex, sha256_hex},
};

// ---------------------------------------------------------------- passwords

pub const MIN_PASSWORD_LEN: usize = 10;

pub fn check_password_policy(pw: &str) -> Result<(), String> {
    if pw.chars().count() < MIN_PASSWORD_LEN {
        return Err(format!("密碼至少需要 {MIN_PASSWORD_LEN} 個字元"));
    }
    if pw.len() > 1024 {
        return Err("密碼過長".into());
    }
    Ok(())
}

pub fn hash_password(pw: &str) -> anyhow::Result<String> {
    Argon2::default()
        .hash_password(pw.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| anyhow::anyhow!("argon2: {e}"))
}

pub fn verify_password(pw: &str, phc: &str) -> bool {
    match PasswordHash::new(phc) {
        Ok(h) => Argon2::default().verify_password(pw.as_bytes(), &h).is_ok(),
        Err(_) => false,
    }
}

/// Hash used to equalise timing when the username does not exist.
fn dummy_hash() -> &'static str {
    static H: OnceLock<String> = OnceLock::new();
    H.get_or_init(|| hash_password("mistbox-timing-equaliser").unwrap())
}

#[derive(sqlx::FromRow, Clone, Debug)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
}

/// Verify credentials (runs Argon2 on a blocking thread).
pub async fn check_login(
    db: &SqlitePool,
    username: &str,
    password: &str,
) -> ApiResult<Option<User>> {
    let user: Option<User> =
        sqlx::query_as("SELECT id, username, password_hash FROM users WHERE username = ?")
            .bind(username)
            .fetch_optional(db)
            .await?;
    let hash = user
        .as_ref()
        .map(|u| u.password_hash.clone())
        .unwrap_or_else(|| dummy_hash().to_string());
    let pw = password.to_string();
    let ok = tokio::task::spawn_blocking(move || verify_password(&pw, &hash))
        .await
        .map_err(|_| ApiError::internal())?;
    Ok(if ok { user } else { None })
}

pub async fn set_password(db: &SqlitePool, user_id: i64, password: &str) -> anyhow::Result<()> {
    let pw = password.to_string();
    let hash = tokio::task::spawn_blocking(move || hash_password(&pw)).await??;
    sqlx::query("UPDATE users SET password_hash = ?, updated_at = ? WHERE id = ?")
        .bind(hash)
        .bind(now())
        .bind(user_id)
        .execute(db)
        .await?;
    // Changing the password logs out every session.
    sqlx::query("DELETE FROM sessions WHERE user_id = ?")
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn create_user(db: &SqlitePool, username: &str, password: &str) -> anyhow::Result<i64> {
    let pw = password.to_string();
    let hash = tokio::task::spawn_blocking(move || hash_password(&pw)).await??;
    let t = now();
    let id = sqlx::query(
        "INSERT INTO users (username, password_hash, created_at, updated_at) VALUES (?, ?, ?, ?)",
    )
    .bind(username)
    .bind(hash)
    .bind(t)
    .bind(t)
    .execute(db)
    .await?
    .last_insert_rowid();
    Ok(id)
}

// ----------------------------------------------------------------- sessions

pub fn cookie_name(secure: bool) -> &'static str {
    // `__Host-` prefix: browser enforces Secure, Path=/ and no Domain.
    if secure {
        "__Host-mistbox_session"
    } else {
        "mistbox_session"
    }
}

pub fn session_cookie(secure: bool, token: &str, max_age_secs: i64) -> String {
    format!(
        "{}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{}",
        cookie_name(secure),
        token,
        max_age_secs,
        if secure { "; Secure" } else { "" }
    )
}

pub struct NewSession {
    pub token: String,
    pub csrf: String,
}

pub async fn create_session(
    db: &SqlitePool,
    user_id: i64,
    ttl_hours: i64,
) -> ApiResult<NewSession> {
    let token = random_hex(32);
    let csrf = random_hex(32);
    let t = now();
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, csrf_token, created_at, expires_at, last_seen_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(sha256_hex(&token))
    .bind(user_id)
    .bind(&csrf)
    .bind(t)
    .bind(t + ttl_hours * 3600)
    .bind(t)
    .execute(db)
    .await?;
    Ok(NewSession { token, csrf })
}

pub fn session_token_from_headers(parts: &Parts, secure: bool) -> Option<String> {
    cookie_value(parts, cookie_name(secure))
}

fn cookie_value(parts: &Parts, name: &str) -> Option<String> {
    parts
        .headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|v| {
            v.split(';').find_map(|kv| {
                let (k, val) = kv.trim().split_once('=')?;
                (k == name && !val.is_empty() && val.len() <= 128).then(|| val.to_string())
            })
        })
}

// ----------------------------------------------------------------- API keys

pub const KEY_PREFIX: &str = "mbk_";

#[derive(sqlx::FromRow, Clone, Debug)]
pub struct ApiKey {
    pub id: String,
    pub name: String,
    pub secret_hash: String,
    pub can_list: bool,
    pub can_read: bool,
    pub can_write: bool,
    pub allow_sensitive: bool,
    pub scope_node_id: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub last_used_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

/// Returns (full key string shown once, public id, secret hash).
pub fn generate_key() -> (String, String, String) {
    let id = random_hex(8);
    let secret = random_hex(32);
    (
        format!("{KEY_PREFIX}{id}_{secret}"),
        id,
        sha256_hex(&secret),
    )
}

fn parse_key(raw: &str) -> Option<(&str, &str)> {
    let rest = raw.strip_prefix(KEY_PREFIX)?;
    let (id, secret) = rest.split_once('_')?;
    let hex_ok = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_hexdigit());
    (hex_ok(id, 16) && hex_ok(secret, 64)).then_some((id, secret))
}

pub async fn verify_api_key(db: &SqlitePool, raw: &str) -> ApiResult<ApiKey> {
    let invalid = || {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_api_key",
            "API Key 無效、已撤銷或已過期",
        )
    };
    let (id, secret) = parse_key(raw).ok_or_else(invalid)?;
    let key: ApiKey = sqlx::query_as("SELECT * FROM api_keys WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or_else(invalid)?;
    if !ct_eq(&sha256_hex(secret), &key.secret_hash) {
        return Err(invalid());
    }
    let t = now();
    if key.revoked_at.is_some() || key.expires_at.is_some_and(|e| e <= t) {
        return Err(invalid());
    }
    if key.last_used_at.is_none_or(|l| t - l >= 30) {
        sqlx::query("UPDATE api_keys SET last_used_at = ? WHERE id = ?")
            .bind(t)
            .bind(&key.id)
            .execute(db)
            .await?;
    }
    Ok(key)
}

// -------------------------------------------------------------- share links

pub const SHARE_PREFIX: &str = "mbs_";

#[derive(sqlx::FromRow, Clone, Debug)]
pub struct Share {
    pub id: String,
    pub node_id: String,
    pub secret_hash: String,
    pub can_write: bool,
    pub label: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub last_used_at: Option<i64>,
    pub use_count: i64,
}

/// Returns (public id, secret, secret hash). The link is `/s/<id>#<secret>`:
/// the secret lives in the URL fragment, which browsers never send to the
/// server, so it cannot end up in access logs, proxies or Referer headers.
pub fn generate_share() -> (String, String, String) {
    let id = random_hex(8);
    let secret = random_hex(32);
    let hash = sha256_hex(&secret);
    (id, secret, hash)
}

fn hex_ok(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Cookie used only by GET requests the browser makes on its own (images,
/// video, downloads) while viewing a share; one cookie per share.
pub fn share_cookie_name(secure: bool, id: &str) -> String {
    if secure {
        format!("__Host-mbs_{id}")
    } else {
        format!("mbs_{id}")
    }
}

pub fn share_cookie(secure: bool, id: &str, secret: &str) -> String {
    format!(
        "{}={}; Path=/; HttpOnly; SameSite=Strict{}",
        share_cookie_name(secure, id),
        secret,
        if secure { "; Secure" } else { "" }
    )
}

pub fn share_invalid() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "share_invalid",
        "分享連結無效、已過期或已被取消",
    )
}

pub async fn verify_share(db: &SqlitePool, id: &str, secret: &str) -> ApiResult<Share> {
    if !(hex_ok(id, 16) && hex_ok(secret, 64)) {
        return Err(share_invalid());
    }
    let sh: Share = sqlx::query_as("SELECT * FROM shares WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or_else(share_invalid)?;
    if !ct_eq(&sha256_hex(secret), &sh.secret_hash) {
        return Err(share_invalid());
    }
    let t = now();
    if sh.expires_at.is_some_and(|e| e <= t) {
        return Err(share_invalid());
    }
    if sh.last_used_at.is_none_or(|l| t - l >= 30) {
        sqlx::query("UPDATE shares SET last_used_at = ? WHERE id = ?")
            .bind(t)
            .bind(&sh.id)
            .execute(db)
            .await?;
    }
    Ok(sh)
}

/// `X-Share-Token: mbs_<id>_<secret>`
fn parse_share_header(raw: &str) -> Option<(&str, &str)> {
    raw.strip_prefix(SHARE_PREFIX)?.split_once('_')
}

/// `?share=<id>` on a GET, plus that share's cookie.
fn share_from_cookie(parts: &Parts, secure: bool) -> Option<(String, String)> {
    if !matches!(parts.method, Method::GET | Method::HEAD) {
        return None;
    }
    let id = parts
        .uri
        .query()?
        .split('&')
        .find_map(|kv| kv.strip_prefix("share="))?;
    if !hex_ok(id, 16) {
        return None;
    }
    let secret = cookie_value(parts, &share_cookie_name(secure, id))?;
    Some((id.to_string(), secret))
}

// ---------------------------------------------------------------- principal

#[derive(Clone, Debug)]
pub enum Principal {
    Admin {
        user_id: i64,
        username: String,
        csrf: String,
    },
    Key(ApiKey),
    /// Someone holding a share link.
    Share(Share),
}

/// What a non-admin principal (API key or share link) may do.
pub struct Grant<'a> {
    pub scope: &'a str,
    pub list: bool,
    pub read: bool,
    pub write: bool,
    pub allow_sensitive: bool,
    /// For error messages: "API Key" / "分享連結".
    pub who: &'static str,
}

impl Principal {
    /// The node this principal sees as "/".
    pub fn root_id(&self) -> &str {
        match self {
            Principal::Admin { .. } => crate::nodes::ROOT_ID,
            Principal::Key(k) => &k.scope_node_id,
            Principal::Share(s) => &s.node_id,
        }
    }

    pub fn is_admin(&self) -> bool {
        matches!(self, Principal::Admin { .. })
    }

    /// `None` for the admin (unrestricted).
    pub fn grant(&self) -> Option<Grant<'_>> {
        match self {
            Principal::Admin { .. } => None,
            Principal::Key(k) => Some(Grant {
                scope: &k.scope_node_id,
                list: k.can_list,
                read: k.can_read,
                write: k.can_write,
                allow_sensitive: k.allow_sensitive,
                who: "API Key",
            }),
            // Shares never expose files marked sensitive.
            Principal::Share(s) => Some(Grant {
                scope: &s.node_id,
                list: true,
                read: true,
                write: s.can_write,
                allow_sensitive: false,
                who: "分享連結",
            }),
        }
    }

    /// "API Key" / "分享連結" for error messages (empty for the admin).
    pub fn who(&self) -> &'static str {
        self.grant().map_or("", |g| g.who)
    }
}

async fn authenticate(parts: &Parts, state: &AppState) -> ApiResult<Principal> {
    if let Some(v) = parts.headers.get(header::AUTHORIZATION) {
        let v = v.to_str().map_err(|_| ApiError::unauthorized())?;
        let raw = v.strip_prefix("Bearer ").ok_or_else(|| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "invalid_api_key",
                "Authorization 必須是 Bearer <API Key>",
            )
        })?;
        return Ok(Principal::Key(verify_api_key(&state.db, raw.trim()).await?));
    }
    if let Some(v) = parts.headers.get("x-share-token") {
        let (id, secret) = v
            .to_str()
            .ok()
            .and_then(parse_share_header)
            .ok_or_else(share_invalid)?;
        return Ok(Principal::Share(verify_share(&state.db, id, secret).await?));
    }
    if let Some((id, secret)) = share_from_cookie(parts, state.cfg.cookie_secure) {
        return Ok(Principal::Share(
            verify_share(&state.db, &id, &secret).await?,
        ));
    }
    let token = session_token_from_headers(parts, state.cfg.cookie_secure)
        .ok_or_else(ApiError::unauthorized)?;
    let row: Option<(i64, String, String)> = sqlx::query_as(
        "SELECT s.user_id, u.username, s.csrf_token FROM sessions s JOIN users u ON u.id = s.user_id
         WHERE s.token_hash = ? AND s.expires_at > ?",
    )
    .bind(sha256_hex(&token))
    .bind(now())
    .fetch_optional(&state.db)
    .await?;
    let (user_id, username, csrf) = row.ok_or_else(ApiError::unauthorized)?;
    // CSRF: every state-changing request with a session must echo the token.
    if !matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS) {
        let sent = parts
            .headers
            .get("x-csrf-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !ct_eq(sent, &csrf) {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "csrf",
                "CSRF 驗證失敗，請重新整理頁面",
            ));
        }
    }
    Ok(Principal::Admin {
        user_id,
        username,
        csrf,
    })
}

impl FromRequestParts<AppState> for Principal {
    type Rejection = ApiError;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some(p) = parts.extensions.get::<Principal>() {
            return Ok(p.clone());
        }
        let p = authenticate(parts, state).await?;
        parts.extensions.insert(p.clone());
        Ok(p)
    }
}

/// Extractor for account / key management: sessions only, never API keys.
pub struct AdminSession {
    pub user_id: i64,
    pub username: String,
    pub csrf: String,
}

impl FromRequestParts<AppState> for AdminSession {
    type Rejection = ApiError;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match Principal::from_request_parts(parts, state).await? {
            Principal::Admin {
                user_id,
                username,
                csrf,
            } => Ok(AdminSession {
                user_id,
                username,
                csrf,
            }),
            Principal::Key(_) => Err(ApiError::forbidden(
                "API Key 不能操作帳號或 API Key 管理功能",
            )),
            Principal::Share(_) => Err(ApiError::forbidden("分享連結不能使用管理功能")),
        }
    }
}

// ------------------------------------------------------------- rate limiter

#[derive(Default)]
pub struct RateLimiter {
    buckets: Mutex<HashMap<String, (Instant, u32)>>,
}

impl RateLimiter {
    /// Fixed-window counter. Err(retry_after_secs) when over `limit`.
    pub fn hit(&self, key: &str, limit: u32, window: Duration) -> Result<(), u64> {
        let mut map = self.buckets.lock().unwrap();
        let now = Instant::now();
        if map.len() > 50_000 {
            map.retain(|_, (start, _)| now.duration_since(*start) < Duration::from_secs(3600));
        }
        let e = map.entry(key.to_string()).or_insert((now, 0));
        if now.duration_since(e.0) >= window {
            *e = (now, 0);
        }
        e.1 += 1;
        if e.1 > limit {
            Err(window
                .saturating_sub(now.duration_since(e.0))
                .as_secs()
                .max(1))
        } else {
            Ok(())
        }
    }

    pub fn reset(&self, key: &str) {
        self.buckets.lock().unwrap().remove(key);
    }
}

pub fn rate_limited(retry_after: u64) -> ApiError {
    ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limited",
        format!("請求過於頻繁，請 {retry_after} 秒後再試"),
    )
    .with_details(serde_json::json!({ "retry_after": retry_after }))
}

pub fn client_ip(
    parts_headers: &axum::http::HeaderMap,
    ext: &axum::http::Extensions,
    trust_proxy: bool,
) -> IpAddr {
    if trust_proxy
        && let Some(ip) = parts_headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit(',').next())
            .and_then(|s| s.trim().parse().ok())
    {
        return ip;
    }
    ext.get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip())
        .unwrap_or(IpAddr::from([0, 0, 0, 0]))
}
