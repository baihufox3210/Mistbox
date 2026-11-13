//! HTTP routes. Everything under `/api/v1`; the built frontend is served
//! from the same origin so no CORS is needed (and none is enabled).

mod code;
mod auth;
mod files;
mod fs;
mod keys;
mod preview;
mod rag;
pub mod settings;
mod shares;
mod trash;

use std::time::Duration;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderValue, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
};
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::{DefaultOnResponse, TraceLayer},
};

use crate::{AppState, auth::Principal, error::ApiError};

/// CSP for the application shell. No inline scripts, no third-party
/// origins: external images/fonts/scripts in previews cannot load.
pub const APP_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; media-src 'self' blob:; \
worker-src 'self' blob:; object-src 'none'; frame-src 'none'; base-uri 'none'; form-action 'self'; \
frame-ancestors 'none'";

/// CSP for user file content: a unique opaque origin with scripts disabled,
/// so HTML/SVG opened directly can never run in the app's origin.
pub const CONTENT_CSP: &str = "sandbox; default-src 'none'; img-src 'self' data:; media-src 'self'; style-src 'unsafe-inline'";

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route(
            "/health",
            get(|| async { Json(serde_json::json!({ "status": "ok" })) }),
        )
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me", get(auth::me))
        .route("/auth/password", post(auth::change_password))
        .route("/whoami", get(auth::whoami))
        .route("/keys", get(keys::list).post(keys::create))
        .route("/keys/{id}", delete(keys::revoke))
        .route("/shares", get(shares::list).post(shares::create))
        .route("/shares/{id}", patch(shares::update).delete(shares::remove))
        .route("/share/open", post(shares::open))
        .route("/search", get(files::search))
        .route(
            "/nodes/{id}",
            get(files::info).patch(files::update).delete(files::remove),
        )
        .route(
            "/nodes/{id}/children",
            get(files::children).post(files::mkdir),
        )
        .route("/nodes/{id}/files", post(files::upload).put(files::upload))
        .route(
            "/nodes/{id}/content",
            get(files::content).put(files::put_content),
        )
        .route("/nodes/{id}/text", get(preview::text))
        .route("/nodes/{id}/resolve", get(preview::resolve))
        .route("/nodes/{id}/relative", get(preview::relative_content))
        .route("/nodes/{id}/latex", post(preview::latex_compile))
        .route("/nodes/{id}/latex/pdf", get(preview::latex_pdf))
        .route("/settings", get(settings::get).put(settings::put))
        .route("/trash", get(trash::list).delete(trash::empty))
        .route("/trash/{id}/restore", post(trash::restore))
        .route("/rag/status", get(rag::status))
        .route("/rag/search", get(rag::search))
        .route("/rag/context", get(rag::context))
        .route("/rag/reindex", post(rag::reindex))
        .route("/code/status", get(code::status))
        .route("/code/rules", get(code::rules))
        .route("/code/findings", get(code::findings))
        .route("/code/duplicates", get(code::duplicates))
        .route("/code/symbols", get(code::symbols))
        .route("/code/reindex", post(code::reindex))
        .route("/fs/list", get(fs::list))
        .route("/fs/info", get(fs::info))
        .route("/fs/download", get(fs::download))
        .route("/fs/upload", post(fs::upload).put(fs::upload))
        .route("/fs/mkdir", post(fs::mkdir))
        .fallback(|| async {
            ApiError::new(
                axum::http::StatusCode::NOT_FOUND,
                "no_route",
                "API 路徑不存在",
            )
        })
        // JSON bodies are small; uploads stream a raw body that is limited separately.
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), rate_limit))
        .layer(middleware::from_fn(no_store));

    let mut app = Router::new().nest("/api/v1", api).route(
        "/api/{*rest}",
        get(|| async {
            ApiError::new(
                axum::http::StatusCode::NOT_FOUND,
                "no_route",
                "API 路徑不存在",
            )
        }),
    );

    if !state.cfg.theme_dir.is_empty() {
        app = app.nest_service("/theme", ServeDir::new(&state.cfg.theme_dir));
    }
    if !state.cfg.static_dir.is_empty() {
        let dir = std::path::PathBuf::from(&state.cfg.static_dir);
        let spa = ServeDir::new(&dir).fallback(ServeFile::new(dir.join("index.html")));
        app = app
            .fallback_service(spa)
            .layer(middleware::from_fn(static_cache));
    }

    app.layer(middleware::from_fn_with_state(state.clone(), security_headers))
        .layer(
            TraceLayer::new_for_http()
                // Log method + path only: never query strings, headers or bodies.
                .make_span_with(|req: &Request| {
                    tracing::info_span!("http", method = %req.method(), path = %req.uri().path())
                })
                .on_request(())
                .on_response(DefaultOnResponse::new().level(tracing::Level::INFO)),
        )
        .with_state(state)
}

async fn security_headers(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let theme = req.uri().path().starts_with("/theme/");
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        "cross-origin-opener-policy",
        HeaderValue::from_static("same-origin"),
    );
    h.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    h.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), microphone=(), geolocation=(), payment=()"),
    );
    if theme {
        // Theme files are used as stylesheets / images only; never as documents.
        h.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CONTENT_CSP),
        );
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    } else if !h.contains_key(header::CONTENT_SECURITY_POLICY) {
        let extra = state.csp_img_extra.read().unwrap().clone();
        let csp = if extra.is_empty() {
            HeaderValue::from_static(APP_CSP)
        } else {
            // Only the configured background image origin is added to img-src.
            HeaderValue::from_str(&APP_CSP.replacen(
                "img-src 'self' data: blob:",
                &format!("img-src 'self' data: blob: {extra}"),
                1,
            ))
            .unwrap_or(HeaderValue::from_static(APP_CSP))
        };
        h.insert(header::CONTENT_SECURITY_POLICY, csp);
    }
    res
}

async fn no_store(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    res.headers_mut()
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("no-store"));
    res
}

async fn static_cache(req: Request, next: Next) -> Response {
    let immutable = req.uri().path().starts_with("/assets/");
    let api = req.uri().path().starts_with("/api/");
    let mut res = next.run(req).await;
    if !api {
        let v = if immutable {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        res.headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(v));
    }
    res
}

async fn rate_limit(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let ip = crate::auth::client_ip(req.headers(), req.extensions(), state.cfg.trust_proxy);
    if let Err(retry) = state.limiter.hit(
        &format!("api:{ip}"),
        state.cfg.api_rate_per_min,
        Duration::from_secs(60),
    ) {
        return crate::auth::rate_limited(retry).into_response();
    }
    next.run(req).await
}

/// `root` is an alias for the principal's own root (the drive root for the
/// admin, the scope node for an API key).
pub(crate) fn resolve_alias(p: &Principal, id: &str) -> String {
    if id == "root" {
        p.root_id().to_string()
    } else {
        id.to_string()
    }
}
