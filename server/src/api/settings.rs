//! UI settings shared across the admin's devices (background image etc.).
//! Theme / layout preferences that are per-device stay in the browser.

use axum::{extract::State, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    auth::AdminSession,
    error::{ApiError, ApiResult},
    util::now,
    AppState,
};

/// Characters allowed in a background URL. Excludes quotes, parentheses,
/// backslashes and whitespace so the value can never break out of CSS `url("…")`.
fn safe_url_chars(s: &str) -> bool {
    s.chars().all(|c| c.is_ascii_alphanumeric() || "-._~:/?#[]@!$&*+,;=%".contains(c))
}

/// Validate a background URL: empty, a same-origin absolute path
/// (`/theme/bg.jpg`, `/api/v1/nodes/<id>/content?inline=1`) or an http(s) URL.
pub fn validate_background(url: &str) -> Result<(), String> {
    if url.is_empty() {
        return Ok(());
    }
    if url.len() > 2048 || !safe_url_chars(url) {
        return Err("網址含有不允許的字元（不可有空白、引號、括號或反斜線；中文請先 URL 編碼）".into());
    }
    if url.starts_with('/') && !url.starts_with("//") {
        return Ok(());
    }
    if csp_origin(url).is_empty() {
        return Err("請使用 https:// 網址，或以 / 開頭的站內路徑（例如 /theme/bg.jpg）".into());
    }
    Ok(())
}

/// `https://host[:port]` for absolute http(s) URLs, otherwise "".
pub fn csp_origin(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else { return String::new() };
    if scheme != "https" && scheme != "http" {
        return String::new();
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() || host.contains('@') || !safe_url_chars(host) {
        return String::new();
    }
    format!("{scheme}://{host}")
}

async fn load(state: &AppState) -> ApiResult<Value> {
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT key, value FROM settings").fetch_all(&state.db).await?;
    let get = |k: &str| rows.iter().find(|r| r.0 == k).map(|r| r.1.clone());
    Ok(json!({
        "background_url": get("background_url").unwrap_or_default(),
        "background_dim": get("background_dim").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.35),
        "background_blur": get("background_blur").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0),
        "theme_css_url": if state.cfg.theme_dir.is_empty() { Value::Null } else { json!("/theme/custom.css") },
    }))
}

pub async fn get(State(state): State<AppState>, _s: AdminSession) -> ApiResult<Json<Value>> {
    Ok(Json(load(&state).await?))
}

#[derive(Deserialize)]
pub struct Patch {
    background_url: Option<String>,
    background_dim: Option<f64>,
    background_blur: Option<f64>,
}

pub async fn put(State(state): State<AppState>, _s: AdminSession, Json(p): Json<Patch>) -> ApiResult<Json<Value>> {
    let mut kv: Vec<(&str, String)> = Vec::new();
    if let Some(u) = p.background_url {
        let u = u.trim().to_string();
        validate_background(&u).map_err(ApiError::bad_request)?;
        *state.csp_img_extra.write().unwrap() = csp_origin(&u);
        kv.push(("background_url", u));
    }
    if let Some(d) = p.background_dim {
        kv.push(("background_dim", d.clamp(0.0, 1.0).to_string()));
    }
    if let Some(b) = p.background_blur {
        kv.push(("background_blur", b.clamp(0.0, 40.0).to_string()));
    }
    for (k, v) in kv {
        sqlx::query(
            "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(k)
        .bind(v)
        .bind(now())
        .execute(&state.db)
        .await?;
    }
    Ok(Json(load(&state).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn background_validation() {
        assert!(validate_background("").is_ok());
        assert!(validate_background("/theme/bg.jpg").is_ok());
        assert!(validate_background("/api/v1/nodes/abc/content?inline=1").is_ok());
        assert!(validate_background("https://images.example.com/a.jpg?w=1920").is_ok());
        assert!(validate_background("javascript:alert(1)").is_err());
        assert!(validate_background("https://x.com/a.jpg\");}body{x:url(").is_err());
        assert!(validate_background("//evil.com/a.jpg").is_err());
        assert!(validate_background("data:image/png;base64,AAA").is_err());
        assert_eq!(csp_origin("https://img.example.com:8443/a.png"), "https://img.example.com:8443");
        assert_eq!(csp_origin("/theme/a.png"), "");
    }
}
