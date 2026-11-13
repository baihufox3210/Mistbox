//! End-to-end API tests against the real router + SQLite + blob store.

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Method, Request, StatusCode},
};
use mistbox::{AppState, api, auth, config::Config};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

struct T {
    app: Router,
    state: AppState,
    _dir: tempfile::TempDir,
    cookie: String,
    csrf: String,
}

struct Resp {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
    fn code(&self) -> String {
        self.json()["error"]["code"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }
    fn h(&self, k: &str) -> String {
        self.headers
            .get(k)
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default()
    }
}

fn sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

fn enc(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

const PW: &str = "correct horse battery";

impl T {
    async fn new() -> T {
        Self::with_config(|_| {}).await
    }

    async fn with_config(f: impl FnOnce(&mut Config)) -> T {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::for_tests(dir.path().join("data"));
        f(&mut cfg);
        let state = AppState::new(cfg).await.unwrap();
        auth::create_user(&state.db, "admin", PW).await.unwrap();
        let app = api::router(state.clone());
        let mut t = T {
            app,
            state,
            _dir: dir,
            cookie: String::new(),
            csrf: String::new(),
        };
        let r = t
            .raw(
                Method::POST,
                "/api/v1/auth/login",
                &[("content-type", "application/json")],
                json!({"username":"admin","password":PW})
                    .to_string()
                    .into_bytes(),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK);
        t.cookie = r.h("set-cookie").split(';').next().unwrap().to_string();
        t.csrf = r.json()["csrf_token"].as_str().unwrap().to_string();
        t
    }

    async fn raw(&self, m: Method, uri: &str, headers: &[(&str, &str)], body: Vec<u8>) -> Resp {
        let mut b = Request::builder().method(m).uri(uri);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let res = self
            .app
            .clone()
            .oneshot(b.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec();
        Resp {
            status,
            headers,
            body,
        }
    }

    /// Request as the logged-in admin (cookie + CSRF).
    async fn admin(&self, m: Method, uri: &str, body: Option<Value>) -> Resp {
        let b = body.map(|v| v.to_string().into_bytes()).unwrap_or_default();
        self.raw(
            m,
            uri,
            &[
                ("cookie", &self.cookie),
                ("x-csrf-token", &self.csrf),
                ("content-type", "application/json"),
            ],
            b,
        )
        .await
    }

    async fn upload(&self, dir: &str, name: &str, data: &[u8]) -> Resp {
        self.raw(
            Method::POST,
            &format!("/api/v1/nodes/{dir}/files?name={}", enc(name)),
            &[("cookie", &self.cookie), ("x-csrf-token", &self.csrf)],
            data.to_vec(),
        )
        .await
    }

    async fn mkdir(&self, parent: &str, name: &str) -> String {
        let r = self
            .admin(
                Method::POST,
                &format!("/api/v1/nodes/{parent}/children"),
                Some(json!({"name": name})),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json());
        r.json()["id"].as_str().unwrap().to_string()
    }

    async fn key(&self, body: Value) -> String {
        let r = self.admin(Method::POST, "/api/v1/keys", Some(body)).await;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json());
        r.json()["key"].as_str().unwrap().to_string()
    }

    async fn bearer(&self, key: &str, m: Method, uri: &str, body: Vec<u8>) -> Resp {
        let h = format!("Bearer {key}");
        self.raw(
            m,
            uri,
            &[("authorization", &h), ("content-type", "application/json")],
            body,
        )
        .await
    }
}

#[tokio::test]
async fn unauthenticated_and_csrf() {
    let t = T::new().await;
    for uri in [
        "/api/v1/nodes/root/children",
        "/api/v1/nodes/root",
        "/api/v1/search?q=a",
        "/api/v1/keys",
        "/api/v1/auth/me",
    ] {
        let r = t.raw(Method::GET, uri, &[], vec![]).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{uri}");
        assert_eq!(r.code(), "unauthorized");
    }
    // Wrong password.
    let r = t
        .raw(
            Method::POST,
            "/api/v1/auth/login",
            &[("content-type", "application/json")],
            json!({"username":"admin","password":"nope"})
                .to_string()
                .into_bytes(),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // Session cookie flags.
    let r = t
        .raw(
            Method::POST,
            "/api/v1/auth/login",
            &[("content-type", "application/json")],
            json!({"username":"admin","password":PW})
                .to_string()
                .into_bytes(),
        )
        .await;
    let c = r.h("set-cookie");
    assert!(
        c.contains("HttpOnly") && c.contains("SameSite=Strict"),
        "{c}"
    );
    // State change without CSRF token is rejected.
    let r = t
        .raw(
            Method::POST,
            "/api/v1/nodes/root/children",
            &[("cookie", &t.cookie), ("content-type", "application/json")],
            br#"{"name":"x"}"#.to_vec(),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.code(), "csrf");
    // Logout invalidates the session.
    let r = t.admin(Method::POST, "/api/v1/auth/logout", None).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = t
        .admin(Method::GET, "/api/v1/nodes/root/children", None)
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_rate_limited() {
    let t = T::new().await;
    let mut last = StatusCode::OK;
    for _ in 0..12 {
        last = t
            .raw(
                Method::POST,
                "/api/v1/auth/login",
                &[("content-type", "application/json")],
                json!({"username":"admin","password":"bad"})
                    .to_string()
                    .into_bytes(),
            )
            .await
            .status;
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn roundtrip_names_and_integrity() {
    let t = T::new().await;
    let mut binary = vec![0u8; 300_000];
    getrandom::fill(&mut binary).unwrap();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("中文 檔名 報告.txt", "內容：你好\n".as_bytes().to_vec()),
        (".hidden", b"secret-ish but not sensitive".to_vec()),
        ("Makefile", b"all:\n\techo hi\n".to_vec()),
        ("data.bin", binary.clone()),
        ("empty", vec![]),
        ("  leading space.md", b"# x".to_vec()),
    ];
    for (name, data) in &cases {
        let r = t.upload("root", name, data).await;
        assert_eq!(r.status, StatusCode::CREATED, "{name}: {:?}", r.json());
        let v = r.json();
        assert_eq!(v["name"], *name);
        assert_eq!(v["sha256"], sha(data));
        let id = v["id"].as_str().unwrap();
        let d = t
            .admin(Method::GET, &format!("/api/v1/nodes/{id}/content"), None)
            .await;
        assert_eq!(d.status, StatusCode::OK);
        assert_eq!(sha(&d.body), sha(data), "{name}");
        assert_eq!(d.h("x-content-sha256"), sha(data));
        assert_eq!(d.h("content-type"), "application/octet-stream");
        assert!(d.h("content-disposition").starts_with("attachment;"));
        assert!(d.h("content-disposition").contains("filename*=UTF-8''"));
        assert_eq!(d.h("x-content-type-options"), "nosniff");
        assert!(d.h("content-security-policy").starts_with("sandbox"));
    }
    // Nested path upload creates folders.
    let r = t
        .raw(
            Method::PUT,
            &format!("/api/v1/nodes/root/files?path={}", enc("a/b/c/深層.txt")),
            &[("cookie", &t.cookie), ("x-csrf-token", &t.csrf)],
            b"deep".to_vec(),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(r.json()["path"], "/a/b/c/深層.txt");
    let r = t
        .admin(
            Method::GET,
            &format!("/api/v1/fs/download?path={}", enc("/a/b/c/深層.txt")),
            None,
        )
        .await;
    assert_eq!(r.body, b"deep");

    // Checksum header verified; mismatch stores nothing.
    let r = t
        .raw(
            Method::POST,
            "/api/v1/nodes/root/files?name=chk.txt",
            &[
                ("cookie", &t.cookie),
                ("x-csrf-token", &t.csrf),
                ("x-content-sha256", &sha(b"other")),
            ],
            b"data".to_vec(),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(r.code(), "checksum_mismatch");
    let r = t
        .admin(Method::GET, "/api/v1/fs/info?path=/chk.txt", None)
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(
        std::fs::read_dir(t.state.store.root().join("tmp"))
            .unwrap()
            .count(),
        0,
        "no temp leftovers"
    );

    // Range request.
    let id = t
        .admin(Method::GET, "/api/v1/fs/info?path=/data.bin", None)
        .await
        .json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = t
        .raw(
            Method::GET,
            &format!("/api/v1/nodes/{id}/content"),
            &[("cookie", &t.cookie), ("range", "bytes=10-19")],
            vec![],
        )
        .await;
    assert_eq!(r.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(r.body, binary[10..20]);
    assert_eq!(
        r.h("content-range"),
        format!("bytes 10-19/{}", binary.len())
    );
}

#[tokio::test]
async fn conflicts_never_overwrite() {
    let t = T::new().await;
    assert_eq!(
        t.upload("root", "a.txt", b"one").await.status,
        StatusCode::CREATED
    );
    let r = t.upload("root", "a.txt", b"two").await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let r = t
        .admin(Method::GET, "/api/v1/fs/download?path=/a.txt", None)
        .await;
    assert_eq!(r.body, b"one");
    let r = t
        .raw(
            Method::POST,
            "/api/v1/nodes/root/files?name=a.txt&on_conflict=rename",
            &[("cookie", &t.cookie), ("x-csrf-token", &t.csrf)],
            b"two".to_vec(),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(r.json()["name"], "a (1).txt");
    // mkdir / rename collisions
    t.mkdir("root", "d").await;
    let r = t
        .admin(
            Method::POST,
            "/api/v1/nodes/root/children",
            Some(json!({"name":"d"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let id = r.json();
    let _ = id;
    let a1 = t
        .admin(
            Method::GET,
            &format!("/api/v1/fs/info?path={}", enc("/a (1).txt")),
            None,
        )
        .await
        .json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = t
        .admin(
            Method::PATCH,
            &format!("/api/v1/nodes/{a1}"),
            Some(json!({"name":"a.txt"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn malicious_names_and_traversal() {
    let t = T::new().await;
    for bad in ["..", ".", "a/b", "..\\..\\x", "a\u{0}b", "x\ny", ""] {
        let r = t.upload("root", bad, b"x").await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad:?}");
    }
    let r = t
        .raw(
            Method::POST,
            &format!("/api/v1/nodes/root/files?path={}", enc("../../etc/passwd")),
            &[("cookie", &t.cookie), ("x-csrf-token", &t.csrf)],
            b"x".to_vec(),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = t
        .admin(
            Method::GET,
            &format!("/api/v1/fs/download?path={}", enc("/../../etc/passwd")),
            None,
        )
        .await;
    assert!(r.status == StatusCode::BAD_REQUEST || r.status == StatusCode::NOT_FOUND);
    let r = t
        .admin(
            Method::GET,
            "/api/v1/nodes/..%2F..%2Fetc%2Fpasswd/content",
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // Nothing escaped into the data dir under a user-supplied name.
    let names: Vec<String> = walk(t.state.store.root());
    assert!(names.iter().all(|n| !n.contains("passwd")));
}

fn walk(p: &std::path::Path) -> Vec<String> {
    let mut out = vec![];
    for e in walkdir::WalkDir::new(p) {
        out.push(e.unwrap().path().display().to_string());
    }
    out
}

#[tokio::test]
async fn untrusted_content_is_sandboxed() {
    let t = T::new().await;
    let html = t
        .upload("root", "evil.html", b"<script>alert(1)</script>")
        .await
        .json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = t
        .admin(
            Method::GET,
            &format!("/api/v1/nodes/{html}/content?inline=1"),
            None,
        )
        .await;
    assert_eq!(
        r.h("content-type"),
        "application/octet-stream",
        "HTML must never be served inline"
    );
    assert!(r.h("content-disposition").starts_with("attachment"));
    let svg = t
        .upload(
            "root",
            "x.svg",
            br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#,
        )
        .await
        .json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = t
        .admin(
            Method::GET,
            &format!("/api/v1/nodes/{svg}/content?inline=1"),
            None,
        )
        .await;
    assert_eq!(r.h("content-type"), "image/svg+xml");
    assert!(
        r.h("content-security-policy").starts_with("sandbox"),
        "SVG served inline only with CSP sandbox"
    );
    let png = t.upload("root", "x.png", b"\x89PNG\r\n\x1a\n").await.json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = t
        .admin(
            Method::GET,
            &format!("/api/v1/nodes/{png}/content?inline=1"),
            None,
        )
        .await;
    assert_eq!(r.h("content-type"), "image/png");
    assert!(r.h("content-disposition").starts_with("inline"));
    // App responses carry a strict CSP.
    let r = t
        .admin(Method::GET, "/api/v1/nodes/root/children", None)
        .await;
    assert!(r.h("content-security-policy").contains("script-src 'self'"));
    assert_eq!(r.h("cache-control"), "no-store");
}

#[tokio::test]
async fn upload_limit_and_preview_limit() {
    let t = T::new().await;
    let big = vec![b'a'; 4 * 1024 * 1024 + 1];
    let r = t.upload("root", "big.txt", &big).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(r.code(), "upload_too_large");
    assert_eq!(
        t.admin(Method::GET, "/api/v1/fs/info?path=/big.txt", None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        std::fs::read_dir(t.state.store.root().join("tmp"))
            .unwrap()
            .count(),
        0
    );

    // Preview limit is 64 KiB in tests.
    let mut text = "第一行\n".repeat(20_000).into_bytes();
    text.truncate(100_001);
    let id = t.upload("root", "long.log", &text).await.json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = t
        .admin(Method::GET, &format!("/api/v1/nodes/{id}/text"), None)
        .await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(r.code(), "preview_too_large");
    let r = t
        .admin(
            Method::GET,
            &format!("/api/v1/nodes/{id}/text?partial=1"),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let v = r.json();
    assert_eq!(v["truncated"], true);
    assert_eq!(v["had_errors"], false, "cut on a UTF-8 boundary");
    // Still downloadable in full.
    let r = t
        .admin(Method::GET, &format!("/api/v1/nodes/{id}/content"), None)
        .await;
    assert_eq!(r.body.len(), text.len());
}

#[tokio::test]
async fn text_preview_encodings() {
    let t = T::new().await;
    let (big5, _, _) =
        encoding_rs::BIG5.encode("這是 Big5 編碼的繁體中文檔案，用來測試編碼偵測與切換。\n");
    let id = t.upload("root", "big5.txt", &big5).await.json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let v = t
        .admin(Method::GET, &format!("/api/v1/nodes/{id}/text"), None)
        .await
        .json();
    assert_eq!(v["valid_utf8"], false);
    let v = t
        .admin(
            Method::GET,
            &format!("/api/v1/nodes/{id}/text?encoding=big5"),
            None,
        )
        .await
        .json();
    assert!(v["text"].as_str().unwrap().contains("繁體中文"));
    // Stored bytes untouched.
    let r = t
        .admin(Method::GET, &format!("/api/v1/nodes/{id}/content"), None)
        .await;
    assert_eq!(r.body, big5.to_vec());
    let id = t.upload("root", "x.bin", &[0, 1, 2, 3, 0, 0]).await.json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let v = t
        .admin(Method::GET, &format!("/api/v1/nodes/{id}/text"), None)
        .await
        .json();
    assert_eq!(v["binary"], true);
    let src = "int main() {\n\tprintf(\"你好 🌏\\n\");\r\n}\n";
    let id = t.upload("root", "m.c", src.as_bytes()).await.json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let v = t
        .admin(Method::GET, &format!("/api/v1/nodes/{id}/text"), None)
        .await
        .json();
    assert_eq!(v["text"], src);
}

#[tokio::test]
async fn rename_move_delete_search() {
    let t = T::new().await;
    let a = t.mkdir("root", "A").await;
    let b = t.mkdir(&a, "B").await;
    let f = t.upload(&b, "note.md", b"hi").await.json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    // Cycle
    let r = t
        .admin(
            Method::PATCH,
            &format!("/api/v1/nodes/{a}"),
            Some(json!({"parent_id": b})),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // Move + rename
    let r = t
        .admin(
            Method::PATCH,
            &format!("/api/v1/nodes/{f}"),
            Some(json!({"parent_id": "root", "name": "筆記.md"})),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["path"], "/筆記.md");
    let r = t
        .admin(
            Method::GET,
            &format!("/api/v1/search?q={}", enc("筆記")),
            None,
        )
        .await;
    assert_eq!(r.json()["results"].as_array().unwrap().len(), 1);
    let r = t.admin(Method::GET, "/api/v1/search?q=b", None).await;
    assert_eq!(r.json()["results"][0]["path"], "/A/B");
    // LIKE wildcards are literal
    assert_eq!(
        t.admin(Method::GET, "/api/v1/search?q=%25", None)
            .await
            .json()["results"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    // Delete folder with content; blob gets collected.
    let g = t.upload(&b, "g.txt", b"gone").await.json();
    let blob: String = sqlx::query_scalar("SELECT blob_id FROM nodes WHERE id = ?")
        .bind(g["id"].as_str().unwrap())
        .fetch_one(&t.state.db)
        .await
        .unwrap();
    let r = t
        .admin(
            Method::DELETE,
            &format!("/api/v1/nodes/{a}?permanent=1"),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["permanent"], true);
    assert_eq!(r.json()["stats"]["files"], 1);
    assert!(t.state.store.blob_path(&blob).exists());
    mistbox::nodes::run_blob_gc(&t.state.db, &t.state.store)
        .await
        .unwrap();
    assert!(!t.state.store.blob_path(&blob).exists());
    assert_eq!(
        t.admin(Method::DELETE, "/api/v1/nodes/root", None)
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn markdown_relative_resources() {
    let t = T::new().await;
    let docs = t.mkdir("root", "docs").await;
    let img_dir = t.mkdir(&docs, "img").await;
    let png = b"\x89PNG\r\n\x1a\nfake";
    t.upload(&img_dir, "圖 1.png", png).await;
    let md = t
        .upload(&docs, "讀我.md", "![](img/圖%201.png)".as_bytes())
        .await
        .json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = t
        .admin(
            Method::GET,
            &format!("/api/v1/nodes/{md}/relative?path={}", enc("img/圖 1.png")),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body, png);
    let r = t
        .admin(
            Method::GET,
            &format!(
                "/api/v1/nodes/{md}/resolve?path={}",
                enc("./img/../讀我.md#top")
            ),
            None,
        )
        .await;
    assert_eq!(r.json()["id"], md);
    // Unauthenticated relative access is refused.
    let r = t
        .raw(
            Method::GET,
            &format!("/api/v1/nodes/{md}/relative?path=img"),
            &[],
            vec![],
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_key_scope_permissions_expiry_revocation() {
    let t = T::new().await;
    let shared = t.mkdir("root", "shared").await;
    let private = t.mkdir("root", "private").await;
    let sub = t.mkdir(&shared, "sub").await;
    let inside = t.upload(&sub, "in.txt", b"inside").await.json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let outside = t.upload(&private, "out.txt", b"outside").await.json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    t.upload(&shared, ".env", b"SECRET=1").await;

    let full = t.key(json!({"name":"rw","permissions":{"list":true,"read":true,"write":true},"scope_node_id": shared})).await;
    assert!(full.starts_with("mbk_") && full.len() > 80);
    // Secret is not stored.
    let stored: String = sqlx::query_scalar("SELECT secret_hash FROM api_keys")
        .fetch_one(&t.state.db)
        .await
        .unwrap();
    assert!(!full.contains(&stored));

    // List scope root via alias and by ID; sensitive .env is hidden.
    let r = t
        .bearer(&full, Method::GET, "/api/v1/nodes/root/children", vec![])
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let names: Vec<String> = r.json()["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, vec!["sub"]);
    assert_eq!(r.json()["node"]["path"], "/", "paths are relative to scope");
    // Deep descendant readable.
    let r = t
        .bearer(
            &full,
            Method::GET,
            &format!("/api/v1/nodes/{inside}/content"),
            vec![],
        )
        .await;
    assert_eq!(r.body, b"inside");
    let r = t
        .bearer(
            &full,
            Method::GET,
            "/api/v1/fs/download?path=/sub/in.txt",
            vec![],
        )
        .await;
    assert_eq!(r.body, b"inside");
    // Guessing an ID outside the scope: 404, not 403.
    for uri in [
        format!("/api/v1/nodes/{outside}/content"),
        format!("/api/v1/nodes/{outside}"),
        format!("/api/v1/nodes/{private}/children"),
        "/api/v1/nodes/root/../".into(),
    ] {
        let r = t.bearer(&full, Method::GET, &uri, vec![]).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{uri}");
    }
    // Relative paths cannot climb out of the scope.
    let r = t
        .bearer(
            &full,
            Method::GET,
            &format!(
                "/api/v1/nodes/{inside}/relative?path={}",
                enc("../../private/out.txt")
            ),
            vec![],
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // Search limited to scope.
    let r = t
        .bearer(&full, Method::GET, "/api/v1/search?q=txt", vec![])
        .await;
    let res = r.json()["results"].as_array().unwrap().clone();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0]["path"], "/sub/in.txt");
    // Upload inside works, outside does not.
    let r = t
        .bearer(
            &full,
            Method::POST,
            &format!("/api/v1/nodes/{sub}/files?name=up.txt"),
            b"k".to_vec(),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let r = t
        .bearer(
            &full,
            Method::POST,
            &format!("/api/v1/nodes/{private}/files?name=up.txt"),
            b"k".to_vec(),
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // Cannot move a file out of the scope, nor delete/rename the scope root.
    let r = t
        .bearer(
            &full,
            Method::PATCH,
            &format!("/api/v1/nodes/{inside}"),
            json!({"parent_id": private}).to_string().into_bytes(),
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = t
        .bearer(
            &full,
            Method::PATCH,
            &format!("/api/v1/nodes/{inside}"),
            json!({"parent_id": "root"}).to_string().into_bytes(),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "'root' aliases to the scope");
    let r = t
        .bearer(
            &full,
            Method::DELETE,
            &format!("/api/v1/nodes/{shared}"),
            vec![],
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // Admin moves a file out: access disappears immediately.
    let r = t
        .admin(
            Method::PATCH,
            &format!("/api/v1/nodes/{inside}"),
            Some(json!({"parent_id": private})),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        t.bearer(
            &full,
            Method::GET,
            &format!("/api/v1/nodes/{inside}/content"),
            vec![]
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    // Keys cannot manage keys or the account.
    assert_eq!(
        t.bearer(&full, Method::GET, "/api/v1/keys", vec![])
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        t.bearer(
            &full,
            Method::POST,
            "/api/v1/keys",
            json!({"name":"x","permissions":{"read":true}})
                .to_string()
                .into_bytes()
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        t.bearer(&full, Method::POST, "/api/v1/auth/password", b"{}".to_vec())
            .await
            .status,
        StatusCode::FORBIDDEN
    );

    // Read-only key: no list, no write.
    let ro = t
        .key(json!({"name":"ro","permissions":{"read":true},"scope_node_id": shared}))
        .await;
    assert_eq!(
        t.bearer(&ro, Method::GET, "/api/v1/nodes/root/children", vec![])
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        t.bearer(
            &ro,
            Method::POST,
            &format!("/api/v1/nodes/{sub}/files?name=z"),
            b"z".to_vec()
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        t.bearer(
            &ro,
            Method::GET,
            "/api/v1/fs/download?path=/sub/up.txt",
            vec![]
        )
        .await
        .status,
        StatusCode::OK
    );

    // File-scoped key: exactly one file.
    let up: String = t
        .admin(Method::GET, "/api/v1/fs/info?path=/shared/sub/up.txt", None)
        .await
        .json()["id"]
        .as_str()
        .unwrap()
        .into();
    let fk = t
        .key(json!({"name":"file","permissions":{"list":true,"read":true},"scope_node_id": up}))
        .await;
    assert_eq!(
        t.bearer(
            &fk,
            Method::GET,
            &format!("/api/v1/nodes/{up}/content"),
            vec![]
        )
        .await
        .body,
        b"k"
    );
    assert_eq!(
        t.bearer(
            &fk,
            Method::GET,
            &format!("/api/v1/nodes/{sub}/children"),
            vec![]
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );

    // Sensitive-allowed key sees .env.
    let sk = t.key(json!({"name":"s","permissions":{"list":true,"read":true},"scope_node_id": shared, "allow_sensitive": true})).await;
    assert_eq!(
        t.bearer(&sk, Method::GET, "/api/v1/fs/download?path=/.env", vec![])
            .await
            .body,
        b"SECRET=1"
    );
    assert_eq!(
        t.bearer(&full, Method::GET, "/api/v1/fs/download?path=/.env", vec![])
            .await
            .status,
        StatusCode::NOT_FOUND
    );

    // Expiry.
    let past = t
        .admin(
            Method::POST,
            "/api/v1/keys",
            Some(
                json!({"name":"p","permissions":{"read":true},"expires_at":"2000-01-01T00:00:00Z"}),
            ),
        )
        .await;
    assert_eq!(past.status, StatusCode::BAD_REQUEST);
    let ek = t.key(json!({"name":"e","permissions":{"read":true,"list":true},"expires_at":"2999-01-01T00:00:00Z"})).await;
    assert_eq!(
        t.bearer(&ek, Method::GET, "/api/v1/whoami", vec![])
            .await
            .status,
        StatusCode::OK
    );
    let eid = &ek[4..20];
    sqlx::query("UPDATE api_keys SET expires_at = 1 WHERE id = ?")
        .bind(eid)
        .execute(&t.state.db)
        .await
        .unwrap();
    let r = t
        .bearer(&ek, Method::GET, "/api/v1/nodes/root/children", vec![])
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.code(), "invalid_api_key");

    // Revocation is immediate; last_used recorded.
    let kid = &full[4..20];
    let list = t.admin(Method::GET, "/api/v1/keys", None).await.json();
    let entry = list["keys"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["id"] == kid)
        .unwrap()
        .clone();
    assert!(entry["last_used_at"].is_string());
    assert!(
        entry.to_string().find(&full[20..]).is_none(),
        "secret never listed"
    );
    assert_eq!(
        t.admin(Method::DELETE, &format!("/api/v1/keys/{kid}"), None)
            .await
            .json()["status"],
        "revoked"
    );
    assert_eq!(
        t.bearer(&full, Method::GET, "/api/v1/whoami", vec![])
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );

    // Tampered / malformed keys.
    let mut bad = ro.clone();
    bad.pop();
    bad.push('0');
    assert_eq!(
        t.bearer(&bad, Method::GET, "/api/v1/whoami", vec![])
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        t.raw(
            Method::GET,
            &format!("/api/v1/whoami?api_key={ro}"),
            &[],
            vec![]
        )
        .await
        .status,
        StatusCode::UNAUTHORIZED,
        "keys are not accepted in URLs"
    );
    assert_eq!(
        t.raw(
            Method::GET,
            "/api/v1/whoami",
            &[("authorization", "Basic abc")],
            vec![]
        )
        .await
        .status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn latex_disabled_reports_clearly() {
    let t = T::new().await;
    let id = t
        .upload(
            "root",
            "a.tex",
            b"\\documentclass{article}\\begin{document}x\\end{document}",
        )
        .await
        .json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let r = t
        .admin(Method::POST, &format!("/api/v1/nodes/{id}/latex"), None)
        .await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(r.code(), "latex_disabled");
    // Source preview still works.
    assert_eq!(
        t.admin(Method::GET, &format!("/api/v1/nodes/{id}/text"), None)
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn settings_background_and_dynamic_csp() {
    let t = T::new().await;
    let r = t.admin(Method::GET, "/api/v1/settings", None).await;
    assert_eq!(r.json()["background_url"], "");
    let bad = t
        .admin(
            Method::PUT,
            "/api/v1/settings",
            Some(json!({"background_url": "javascript:alert(1)"})),
        )
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    let bad = t
        .admin(
            Method::PUT,
            "/api/v1/settings",
            Some(json!({"background_url": "https://x.com/a.jpg\");}"})),
        )
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    let ok = t
        .admin(
            Method::PUT,
            "/api/v1/settings",
            Some(json!({"background_url": "https://img.example.com/bg.jpg", "background_dim": 5})),
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK);
    assert_eq!(ok.json()["background_dim"], 0.95, "clamped");
    let r = t
        .admin(Method::GET, "/api/v1/nodes/root/children", None)
        .await;
    assert!(
        r.h("content-security-policy")
            .contains("img-src 'self' data: blob: https://img.example.com;"),
        "{}",
        r.h("content-security-policy")
    );
    assert!(
        !r.h("content-security-policy")
            .contains("script-src 'self' https"),
        "only img-src widened"
    );
    // API keys cannot read or change settings.
    let key = t
        .key(json!({"name":"k","permissions":{"list":true,"read":true,"write":true}}))
        .await;
    assert_eq!(
        t.bearer(&key, Method::GET, "/api/v1/settings", vec![])
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn overwrite_requires_matching_version() {
    let t = T::new().await;
    let v = t.upload("root", "note.md", b"v1").await.json();
    let id = v["id"].as_str().unwrap().to_string();
    let sha1 = v["sha256"].as_str().unwrap().to_string();
    let old_blob: String = sqlx::query_scalar("SELECT blob_id FROM nodes WHERE id = ?")
        .bind(&id)
        .fetch_one(&t.state.db)
        .await
        .unwrap();
    let put = |ifm: Option<String>, body: &'static [u8]| {
        let id = id.clone();
        let t = &t;
        async move {
            let mut h = vec![
                ("cookie", t.cookie.as_str()),
                ("x-csrf-token", t.csrf.as_str()),
            ];
            let tag;
            if let Some(m) = ifm {
                tag = format!("\"{m}\"");
                h.push(("if-match", tag.as_str()));
            }
            t.raw(
                Method::PUT,
                &format!("/api/v1/nodes/{id}/content"),
                &h,
                body.to_vec(),
            )
            .await
        }
    };
    assert_eq!(
        put(None, b"v2").await.status,
        StatusCode::PRECONDITION_REQUIRED
    );
    assert_eq!(
        put(Some(sha("x".as_bytes())), b"v2").await.status,
        StatusCode::PRECONDITION_FAILED
    );
    let r = put(Some(sha1.clone()), b"v2").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["sha256"], sha(b"v2"));
    // The old version hash is now stale.
    assert_eq!(
        put(Some(sha1), b"v3").await.status,
        StatusCode::PRECONDITION_FAILED
    );
    let d = t
        .admin(Method::GET, &format!("/api/v1/nodes/{id}/content"), None)
        .await;
    assert_eq!(d.body, b"v2");
    mistbox::nodes::run_blob_gc(&t.state.db, &t.state.store)
        .await
        .unwrap();
    assert!(
        !t.state.store.blob_path(&old_blob).exists(),
        "old version collected"
    );
    // Read-only key cannot overwrite.
    let ro = t
        .key(json!({"name":"ro","permissions":{"read":true}}))
        .await;
    let h = format!("Bearer {ro}");
    let tag = format!("\"{}\"", sha(b"v2"));
    let r = t
        .raw(
            Method::PUT,
            &format!("/api/v1/nodes/{id}/content"),
            &[("authorization", &h), ("if-match", &tag)],
            b"x".to_vec(),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn semantic_search_is_scoped_and_tracks_changes() {
    let t = T::new().await;
    let rag = t.state.rag.clone().expect("hash model enabled in tests");
    rag.load_model().await.unwrap();
    let docs = t.mkdir("root", "docs").await;
    let private = t.mkdir("root", "private").await;
    t.upload(
        &docs,
        "latex.md",
        "# LaTeX\n把 tex 文件編譯成 PDF 需要 xelatex 引擎。\n".as_bytes(),
    )
    .await;
    t.upload(
        &docs,
        "cats.md",
        "# 貓\n貓咪喜歡在陽光下睡覺。\n".as_bytes(),
    )
    .await;
    t.upload(
        &private,
        "secret-plan.md",
        "PDF 編譯 xelatex 機密計畫\n".as_bytes(),
    )
    .await;
    t.upload(&docs, ".env", "PDF_TOKEN=xelatex編譯\n".as_bytes())
        .await;
    t.upload(&docs, "img.png", b"\x89PNG\r\n\x1a\nxxxx").await;
    let n = rag
        .index_pending(&t.state.db, &t.state.store)
        .await
        .unwrap();
    assert_eq!(
        n, 4,
        "md×3 + png (skipped); .env is sensitive and never indexed"
    );

    let r = t
        .admin(
            Method::GET,
            &format!("/api/v1/rag/search?q={}", enc("如何用 xelatex 編譯 PDF")),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
    let res = r.json()["results"].as_array().unwrap().clone();
    let paths: Vec<&str> = res
        .iter()
        .map(|h| h["node"]["path"].as_str().unwrap())
        .collect();
    assert!(
        paths[..2].contains(&"/docs/latex.md") && paths[..2].contains(&"/private/secret-plan.md"),
        "{paths:?}"
    );
    assert!(!paths.contains(&"/docs/.env"));
    assert!(res[0]["line_start"].as_i64().unwrap() >= 1);

    // Scoped key: never sees /private, paths are relative to the scope.
    let key = t
        .key(json!({"name":"k","permissions":{"list":true,"read":true},"scope_node_id": docs}))
        .await;
    let r = t
        .bearer(
            &key,
            Method::GET,
            &format!("/api/v1/rag/search?q={}", enc("xelatex 編譯 PDF")),
            vec![],
        )
        .await;
    let res = r.json()["results"].as_array().unwrap().clone();
    assert!(!res.is_empty());
    assert!(
        res.iter()
            .all(|h| !h["text"].as_str().unwrap().contains("機密")),
        "out-of-scope content leaked"
    );
    assert_eq!(res[0]["node"]["path"], "/latex.md");
    let ctx = t
        .bearer(
            &key,
            Method::GET,
            &format!("/api/v1/rag/context?q={}&k=2", enc("xelatex")),
            vec![],
        )
        .await
        .json();
    assert!(
        ctx["context"]
            .as_str()
            .unwrap()
            .starts_with("[1] /latex.md:")
    );
    // list-only key may not retrieve content.
    let lk = t.key(json!({"name":"l","permissions":{"list":true}})).await;
    assert_eq!(
        t.bearer(&lk, Method::GET, "/api/v1/rag/search?q=x", vec![])
            .await
            .status,
        StatusCode::FORBIDDEN
    );

    // Editing re-indexes; deleting removes from results.
    let cats = t
        .admin(Method::GET, "/api/v1/fs/info?path=/docs/cats.md", None)
        .await
        .json();
    let tag = format!("\"{}\"", cats["sha256"].as_str().unwrap());
    let cid = cats["id"].as_str().unwrap().to_string();
    let r = t
        .raw(
            Method::PUT,
            &format!("/api/v1/nodes/{cid}/content"),
            &[
                ("cookie", &t.cookie),
                ("x-csrf-token", &t.csrf),
                ("if-match", &tag),
            ],
            "量子力學 薛丁格方程式".as_bytes().to_vec(),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    rag.index_pending(&t.state.db, &t.state.store)
        .await
        .unwrap();
    let r = t
        .admin(
            Method::GET,
            &format!("/api/v1/rag/search?q={}&k=1", enc("薛丁格方程式")),
            None,
        )
        .await
        .json();
    assert_eq!(r["results"][0]["node"]["path"], "/docs/cats.md");
    t.admin(Method::DELETE, &format!("/api/v1/nodes/{cid}"), None)
        .await;
    rag.index_pending(&t.state.db, &t.state.store)
        .await
        .unwrap();
    let r = t
        .admin(
            Method::GET,
            &format!("/api/v1/rag/search?q={}", enc("薛丁格方程式")),
            None,
        )
        .await
        .json();
    assert!(
        r["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|h| h["node"]["path"] != "/docs/cats.md")
    );
    let st = t
        .admin(Method::GET, "/api/v1/rag/status", None)
        .await
        .json();
    assert_eq!(st["indexed_files"], 2);
    assert_eq!(st["pending"], 0);
    assert_eq!(st["skipped"], 1);
}

#[tokio::test]
async fn recycle_bin_hides_restores_and_purges() {
    let t = T::new().await;
    let docs = t.mkdir("root", "docs").await;
    let sub = t.mkdir(&docs, "sub").await;
    let f = t.upload(&sub, "plan.md", b"secret plan").await.json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let key = t
        .key(json!({"name":"k","permissions":{"list":true,"read":true,"write":true}}))
        .await;

    // Trash a folder: gone from listings, search, path access and API keys.
    let r = t
        .admin(Method::DELETE, &format!("/api/v1/nodes/{sub}"), None)
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["permanent"], false);
    let names: Vec<String> = t
        .admin(Method::GET, &format!("/api/v1/nodes/{docs}/children"), None)
        .await
        .json()["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.is_empty());
    let root: Vec<String> = t
        .admin(Method::GET, "/api/v1/nodes/root/children", None)
        .await
        .json()["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect();
    assert!(
        !root.contains(&"trash".to_string()),
        "bin folder never listed"
    );
    assert_eq!(
        t.admin(Method::GET, "/api/v1/search?q=plan", None)
            .await
            .json()["results"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        t.admin(Method::GET, "/api/v1/fs/info?path=/docs/sub/plan.md", None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        t.bearer(
            &key,
            Method::GET,
            &format!("/api/v1/nodes/{f}/content"),
            vec![]
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        t.bearer(&key, Method::GET, "/api/v1/nodes/trash/children", vec![])
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        t.bearer(&key, Method::GET, "/api/v1/trash", vec![])
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    // Bytes are kept; nothing can be uploaded into the bin.
    assert_eq!(
        t.admin(Method::GET, &format!("/api/v1/nodes/{f}/content"), None)
            .await
            .body,
        b"secret plan"
    );
    assert_eq!(
        t.upload("trash", "x.txt", b"x").await.status,
        StatusCode::BAD_REQUEST
    );
    // A new item may take the old name while the original is in the bin.
    let again = t.mkdir(&docs, "sub").await;

    let list = t.admin(Method::GET, "/api/v1/trash", None).await.json();
    let item = &list["items"][0];
    assert_eq!(item["name"], "sub");
    assert_eq!(item["path"], "/docs/sub");
    assert_eq!(item["stats"]["files"], 1);
    assert!(item["purge_at"].is_string());

    // Restore: name clash → "sub (1)", never overwrites.
    let r = t
        .admin(Method::POST, &format!("/api/v1/trash/{sub}/restore"), None)
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["node"]["path"], "/docs/sub (1)");
    assert_eq!(
        t.admin(
            Method::GET,
            "/api/v1/fs/download?path=/docs/sub%20(1)/plan.md",
            None
        )
        .await
        .body,
        b"secret plan"
    );
    assert_eq!(
        t.bearer(
            &key,
            Method::GET,
            &format!("/api/v1/nodes/{f}/content"),
            vec![]
        )
        .await
        .status,
        StatusCode::OK
    );

    // Original parent gone → restored to the root.
    t.admin(Method::DELETE, &format!("/api/v1/nodes/{f}"), None)
        .await;
    t.admin(
        Method::DELETE,
        &format!("/api/v1/nodes/{docs}?permanent=1"),
        None,
    )
    .await;
    let r = t
        .admin(Method::POST, &format!("/api/v1/trash/{f}/restore"), None)
        .await
        .json();
    assert_eq!(r["node"]["path"], "/plan.md");
    assert_eq!(r["restored_to_root"], true);
    let _ = again;

    // Keys may trash but not permanently delete.
    assert_eq!(
        t.bearer(
            &key,
            Method::DELETE,
            &format!("/api/v1/nodes/{f}?permanent=1"),
            vec![]
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        t.bearer(&key, Method::DELETE, &format!("/api/v1/nodes/{f}"), vec![])
            .await
            .status,
        StatusCode::OK
    );
    // Deleting again from the bin is permanent; blobs are then collected.
    let blob: String = sqlx::query_scalar("SELECT blob_id FROM nodes WHERE id = ?")
        .bind(&f)
        .fetch_one(&t.state.db)
        .await
        .unwrap();
    let r = t.admin(Method::DELETE, "/api/v1/trash", None).await;
    assert_eq!(r.json()["deleted"], 1);
    mistbox::nodes::run_blob_gc(&t.state.db, &t.state.store)
        .await
        .unwrap();
    assert!(!t.state.store.blob_path(&blob).exists());
    assert_eq!(
        t.admin(Method::GET, "/api/v1/trash", None).await.json()["items"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    // Expiry purge.
    let g = t.upload("root", "old.txt", b"o").await.json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    t.admin(Method::DELETE, &format!("/api/v1/nodes/{g}"), None)
        .await;
    sqlx::query("UPDATE trash SET trashed_at = 1")
        .execute(&t.state.db)
        .await
        .unwrap();
    assert_eq!(
        mistbox::nodes::purge_trash(&t.state.db, 30).await.unwrap(),
        1
    );
    assert!(
        mistbox::nodes::get_opt(&t.state.db, &g)
            .await
            .unwrap()
            .is_none()
    );
}

/// Stand-in for the sandboxed worker's `/pdftotext` endpoint.
async fn fake_pdf_worker() -> (String, std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>>) {
    use axum::{body::Bytes, response::IntoResponse, routing::post};
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Vec<u8>>::new()));
    let s2 = seen.clone();
    let app = Router::new().route(
        "/pdftotext",
        post(move |body: Bytes| {
            let seen = s2.clone();
            async move {
                seen.lock().unwrap().push(body.to_vec());
                if body.windows(9).any(|w| w == b"ENCRYPTED") {
                    return (
                        StatusCode::UNPROCESSABLE_ENTITY,
                        axum::Json(json!({"code":"pdf_failed","message":"PDF 已加密，無法擷取文字","log":""})),
                    )
                        .into_response();
                }
                if body.windows(7).any(|w| w == b"SCANNED") {
                    return "\x0c \n\x0c".into_response();
                }
                "第一頁：貓咪喜歡在陽光下睡覺。\n\x0c第二頁：用 xelatex 把 tex 編譯成 PDF。\n\x0c".into_response()
            }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (url, seen)
}

#[tokio::test]
async fn pdf_text_is_indexed_per_page_via_worker() {
    let (url, seen) = fake_pdf_worker().await;
    let t = T::with_config(|c| c.latex_url = Some(url)).await;
    let rag = t.state.rag.clone().unwrap();
    rag.load_model().await.unwrap();
    let pdf = b"%PDF-1.7\n...binary...\n%%EOF\n".to_vec();
    t.upload("root", "paper.pdf", &pdf).await;
    t.upload("root", "locked.pdf", b"%PDF-1.7 ENCRYPTED").await;
    t.upload("root", "scan.pdf", b"%PDF-1.7 SCANNED").await;
    rag.index_pending(&t.state.db, &t.state.store)
        .await
        .unwrap();
    assert!(
        seen.lock().unwrap().contains(&pdf),
        "worker received the exact PDF bytes"
    );

    let r = t
        .admin(
            Method::GET,
            &format!("/api/v1/rag/search?q={}", enc("xelatex 編譯 PDF")),
            None,
        )
        .await;
    let res = r.json()["results"].as_array().unwrap().clone();
    assert_eq!(res[0]["node"]["path"], "/paper.pdf", "{res:?}");
    assert_eq!(res[0]["unit"], "page");
    assert_eq!(res[0]["line_start"], 2);
    assert!(res[0]["text"].as_str().unwrap().contains("第二頁"));

    let st = t
        .admin(Method::GET, "/api/v1/rag/status", None)
        .await
        .json();
    assert_eq!(st["indexed_files"], 1);
    assert_eq!(
        st["skipped"], 2,
        "encrypted + text-less PDFs are skipped: {st}"
    );
    assert_eq!(st["pdf"], true);
    let detail: Vec<String> =
        sqlx::query_scalar("SELECT detail FROM rag_docs WHERE status = 'skipped' ORDER BY detail")
            .fetch_all(&t.state.db)
            .await
            .unwrap();
    assert!(
        detail.iter().any(|d| d.contains("加密")) && detail.iter().any(|d| d.contains("掃描")),
        "{detail:?}"
    );
}

#[tokio::test]
async fn pdf_skipped_without_worker() {
    let t = T::new().await;
    let rag = t.state.rag.clone().unwrap();
    rag.load_model().await.unwrap();
    t.upload("root", "paper.pdf", b"%PDF-1.7 xelatex").await;
    rag.index_pending(&t.state.db, &t.state.store)
        .await
        .unwrap();
    let st = t
        .admin(Method::GET, "/api/v1/rag/status", None)
        .await
        .json();
    assert_eq!(
        (st["skipped"].as_i64(), st["pdf"].as_bool()),
        (Some(1), Some(false))
    );
}

/// Request as a share-link holder (`X-Share-Token`).
async fn as_share(t: &T, token: &str, m: Method, uri: &str, body: Vec<u8>) -> Resp {
    t.raw(
        m,
        uri,
        &[
            ("x-share-token", token),
            ("content-type", "application/json"),
        ],
        body,
    )
    .await
}

/// Create a share; returns (id, header token, link).
async fn make_share(t: &T, body: Value) -> (String, String, String) {
    let r = t.admin(Method::POST, "/api/v1/shares", Some(body)).await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json());
    let link = r.json()["link"].as_str().unwrap().to_string();
    let (path, secret) = link.split_once('#').unwrap();
    let id = path.strip_prefix("/s/").unwrap().to_string();
    assert_eq!(r.json()["share"]["id"], id.as_str());
    (id.clone(), format!("mbs_{id}_{secret}"), link)
}

#[tokio::test]
async fn share_links_view_edit_expiry_and_isolation() {
    let t = T::new().await;
    let proj = t.mkdir("root", "proj").await;
    let sub = t.mkdir(&proj, "sub").await;
    let other = t.mkdir("root", "other").await;
    let a = t.upload(&proj, "a.md", b"# hello").await.json()["id"]
        .as_str()
        .unwrap()
        .to_string();
    t.upload(&proj, ".env", b"SECRET=1").await;
    let secret_file = t.upload(&other, "secret.txt", b"nope").await.json()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Sensitive items and the bin cannot be shared; bad permission rejected.
    let env_id = sqlx::query_scalar::<_, String>("SELECT id FROM nodes WHERE name = '.env'")
        .fetch_one(&t.state.db)
        .await
        .unwrap();
    for body in [
        json!({"node_id": env_id}),
        json!({"node_id": "trash"}),
        json!({"node_id": proj, "permission": "owner"}),
    ] {
        assert_eq!(
            t.admin(Method::POST, "/api/v1/shares", Some(body))
                .await
                .status,
            StatusCode::BAD_REQUEST
        );
    }
    let past = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    assert_eq!(
        t.admin(
            Method::POST,
            "/api/v1/shares",
            Some(json!({"node_id": proj, "expires_at": past}))
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );

    let soon = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    let (id, tok, link) = make_share(
        &t,
        json!({"node_id": proj, "expires_at": soon, "label": "給同事"}),
    )
    .await;
    assert!(link.starts_with("/s/") && link.contains('#'));

    // Open: describes the share and sets a per-share HttpOnly cookie.
    let r = as_share(&t, &tok, Method::POST, "/api/v1/share/open", vec![]).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
    assert_eq!(r.json()["permission"], "view");
    assert_eq!(r.json()["root"]["name"], "proj");
    assert_eq!(r.json()["root"]["parent_id"], Value::Null);
    let cookie = r.h("set-cookie");
    assert!(
        cookie.starts_with(&format!("mbs_{id}="))
            && cookie.contains("HttpOnly")
            && cookie.contains("SameSite=Strict"),
        "{cookie}"
    );
    let cookie = cookie.split(';').next().unwrap().to_string();

    // View: list the shared folder (root alias = share root), sensitive hidden.
    let r = as_share(&t, &tok, Method::GET, "/api/v1/nodes/root/children", vec![]).await;
    assert_eq!(r.status, StatusCode::OK);
    let names: Vec<String> = r.json()["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, vec!["sub".to_string(), "a.md".to_string()]);
    assert_eq!(
        as_share(
            &t,
            &tok,
            Method::GET,
            &format!("/api/v1/nodes/{a}/content"),
            vec![]
        )
        .await
        .body,
        b"# hello"
    );
    // Outside the share → 404; writes → 403 with a clear message.
    assert_eq!(
        as_share(
            &t,
            &tok,
            Method::GET,
            &format!("/api/v1/nodes/{secret_file}/content"),
            vec![]
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        as_share(
            &t,
            &tok,
            Method::GET,
            &format!("/api/v1/nodes/{other}/children"),
            vec![]
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    let r = as_share(
        &t,
        &tok,
        Method::POST,
        &format!("/api/v1/nodes/{sub}/files?name=x.txt"),
        b"x".to_vec(),
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(
        r.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("只能檢視")
    );
    // Search stays inside the share.
    let r = as_share(&t, &tok, Method::GET, "/api/v1/search?q=secret", vec![]).await;
    assert_eq!(r.json()["results"].as_array().unwrap().len(), 0);

    // Cookie: only for GETs that name the share.
    let with_cookie = |uri: String, m: Method| {
        let c = cookie.clone();
        let t = &t;
        async move { t.raw(m, &uri, &[("cookie", &c)], vec![]).await }
    };
    assert_eq!(
        with_cookie(format!("/api/v1/nodes/{a}/content?share={id}"), Method::GET)
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        with_cookie(format!("/api/v1/nodes/{a}/content"), Method::GET)
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        with_cookie(
            format!("/api/v1/nodes/{secret_file}/content?share={id}"),
            Method::GET
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        with_cookie(format!("/api/v1/nodes/{a}?share={id}"), Method::DELETE)
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );

    // Admin-only surfaces are closed to shares.
    for (m, uri) in [
        (Method::GET, "/api/v1/keys"),
        (Method::GET, "/api/v1/shares"),
        (Method::GET, "/api/v1/trash"),
        (Method::GET, "/api/v1/settings"),
        (Method::GET, "/api/v1/rag/search?q=x"),
    ] {
        assert_eq!(
            as_share(&t, &tok, m, uri, vec![]).await.status,
            StatusCode::FORBIDDEN,
            "{uri}"
        );
    }

    // Upgrade to edit: upload, mkdir, rename, trash — but not the share root,
    // not permanent deletion, not the sensitive flag.
    let r = t
        .admin(
            Method::PATCH,
            &format!("/api/v1/shares/{id}"),
            Some(json!({"permission": "edit"})),
        )
        .await;
    assert_eq!(r.json()["permission"], "edit");
    assert!(
        r.json()["expires_at"].is_string(),
        "expiry unchanged when omitted"
    );
    let r = as_share(
        &t,
        &tok,
        Method::POST,
        &format!("/api/v1/nodes/{sub}/files?name=up.txt"),
        b"from share".to_vec(),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json());
    let up = r.json()["id"].as_str().unwrap().to_string();
    let r = as_share(
        &t,
        &tok,
        Method::POST,
        "/api/v1/nodes/root/children",
        json!({"name":"new"}).to_string().into_bytes(),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let r = as_share(
        &t,
        &tok,
        Method::PATCH,
        &format!("/api/v1/nodes/{up}"),
        json!({"name":"renamed.txt"}).to_string().into_bytes(),
    )
    .await;
    assert_eq!(r.json()["name"], "renamed.txt");
    let r = as_share(
        &t,
        &tok,
        Method::PATCH,
        &format!("/api/v1/nodes/{up}"),
        json!({"sensitive":true}).to_string().into_bytes(),
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = as_share(
        &t,
        &tok,
        Method::PATCH,
        &format!("/api/v1/nodes/{up}"),
        json!({"parent_id": other}).to_string().into_bytes(),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::NOT_FOUND,
        "cannot move out of the share"
    );
    let r = as_share(
        &t,
        &tok,
        Method::PATCH,
        "/api/v1/nodes/root",
        json!({"name":"hijack"}).to_string().into_bytes(),
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "share root is immutable");
    assert_eq!(
        as_share(
            &t,
            &tok,
            Method::DELETE,
            &format!("/api/v1/nodes/{up}?permanent=1"),
            vec![]
        )
        .await
        .status,
        StatusCode::FORBIDDEN
    );
    let r = as_share(
        &t,
        &tok,
        Method::DELETE,
        &format!("/api/v1/nodes/{up}"),
        vec![],
    )
    .await;
    assert_eq!(
        (r.status, r.json()["permanent"].as_bool()),
        (StatusCode::OK, Some(false))
    );
    // The trashed item is gone for the share, restorable by the admin.
    assert_eq!(
        as_share(
            &t,
            &tok,
            Method::GET,
            &format!("/api/v1/nodes/{up}"),
            vec![]
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        t.admin(Method::POST, &format!("/api/v1/trash/{up}/restore"), None)
            .await
            .status,
        StatusCode::OK
    );

    // Listing for the admin shows usage.
    let r = t
        .admin(Method::GET, &format!("/api/v1/shares?node_id={proj}"), None)
        .await;
    let s = &r.json()["shares"][0];
    assert_eq!(
        (
            s["status"].as_str(),
            s["label"].as_str(),
            s["use_count"].as_i64()
        ),
        (Some("active"), Some("給同事"), Some(1))
    );
    assert_eq!(s["node"]["path"], "/proj");

    // A wrong secret or a malformed token is rejected the same way.
    let bad = format!("{}0", &tok[..tok.len() - 1]);
    for token in [bad.as_str(), "mbs_zz_zz", "garbage"] {
        let r = as_share(
            &t,
            token,
            Method::GET,
            "/api/v1/nodes/root/children",
            vec![],
        )
        .await;
        assert_eq!(
            (r.status, r.code()),
            (StatusCode::NOT_FOUND, "share_invalid".to_string()),
            "{token}"
        );
    }

    // Expired → invalid; clearing the expiry (null) revives it.
    sqlx::query("UPDATE shares SET expires_at = 1 WHERE id = ?")
        .bind(&id)
        .execute(&t.state.db)
        .await
        .unwrap();
    assert_eq!(
        as_share(&t, &tok, Method::GET, "/api/v1/nodes/root/children", vec![])
            .await
            .code(),
        "share_invalid"
    );
    let r = t
        .admin(
            Method::PATCH,
            &format!("/api/v1/shares/{id}"),
            Some(json!({"expires_at": null})),
        )
        .await;
    assert_eq!(r.json()["expires_at"], Value::Null);
    assert_eq!(
        as_share(&t, &tok, Method::GET, "/api/v1/nodes/root/children", vec![])
            .await
            .status,
        StatusCode::OK
    );

    // Shared folder moved to the bin → link stops working.
    t.admin(Method::DELETE, &format!("/api/v1/nodes/{proj}"), None)
        .await;
    assert_eq!(
        as_share(&t, &tok, Method::POST, "/api/v1/share/open", vec![])
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    t.admin(Method::POST, &format!("/api/v1/trash/{proj}/restore"), None)
        .await;

    // Deleted → gone.
    assert_eq!(
        t.admin(Method::DELETE, &format!("/api/v1/shares/{id}"), None)
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        as_share(&t, &tok, Method::GET, "/api/v1/nodes/root/children", vec![])
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn file_share_edit_allows_saving_content_only() {
    let t = T::new().await;
    let f = t.upload("root", "note.txt", b"v1").await.json();
    let (id, sha) = (
        f["id"].as_str().unwrap().to_string(),
        f["sha256"].as_str().unwrap().to_string(),
    );
    let (_, tok, _) = make_share(&t, json!({"node_id": id, "permission": "edit"})).await;
    let r = as_share(&t, &tok, Method::GET, "/api/v1/nodes/root", vec![]).await;
    assert_eq!(r.json()["name"], "note.txt");
    let etag = format!("\"{sha}\"");
    let r = t
        .raw(
            Method::PUT,
            &format!("/api/v1/nodes/{id}/content"),
            &[("x-share-token", &tok), ("if-match", &etag)],
            b"v2".to_vec(),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
    assert_eq!(
        t.admin(Method::GET, &format!("/api/v1/nodes/{id}/content"), None)
            .await
            .body,
        b"v2"
    );
    // …but cannot rename or delete the shared file itself.
    let r = as_share(
        &t,
        &tok,
        Method::PATCH,
        "/api/v1/nodes/root",
        json!({"name":"x"}).to_string().into_bytes(),
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(
        as_share(&t, &tok, Method::DELETE, "/api/v1/nodes/root", vec![])
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

const LED_C: &str = r#"#include "pico/stdlib.h"
#include "hardware/pio.h"
static void fill_buffer(uint32_t *buf, int n, uint32_t color) {
    for (int i = 0; i < n; i++) {
        uint32_t r = (color >> 16) & 0xff;
        uint32_t g = (color >> 8) & 0xff;
        uint32_t b = color & 0xff;
        buf[i] = (g << 24) | (r << 16) | (b << 8);
    }
}
int check(uint32_t x) {
    return x & 0x4 == 0;
}
"#;

#[tokio::test]
async fn code_index_findings_duplicates_symbols_and_scope() {
    let t = T::new().await;
    let fw = t.mkdir("root", "fw").await;
    let other = t.mkdir("root", "other").await;
    let a = t.upload(&fw, "led.c", LED_C.as_bytes()).await.json()["id"].as_str().unwrap().to_string();
    let copy = LED_C.replace("fill_buffer", "paint").replace("color", "rgb").replace("check", "chk");
    let b = t.upload(&other, "copy.c", copy.as_bytes()).await.json()["id"].as_str().unwrap().to_string();
    t.upload(&fw, "ws.pio", b".program p\n    set pins, 40\n").await;
    t.upload(&fw, "notes.md", b"# not code\n").await;
    // Sensitive: never analysed.
    let s = t.upload(&fw, "secret.c", b"int f(int x){ return x & 1 == 0; }\n").await.json()["id"].as_str().unwrap().to_string();
    t.admin(Method::PATCH, &format!("/api/v1/nodes/{s}"), Some(json!({"sensitive": true}))).await;

    let n = t.state.code.index_pending(&t.state.db, &t.state.store).await.unwrap();
    assert_eq!(n, 4, "sensitive file is not even looked at");
    assert_eq!(t.state.code.index_pending(&t.state.db, &t.state.store).await.unwrap(), 0, "nothing changed");

    let r = t.admin(Method::GET, "/api/v1/code/status", None).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
    assert_eq!(r.json()["files"], 3, "{:?}", r.json());
    assert_eq!(r.json()["by_lang"]["c"], 2);
    assert_eq!(r.json()["by_lang"]["pio"], 1);
    assert_eq!(r.json()["pending"], 0);

    // Findings in one file, and in a folder.
    let r = t.admin(Method::GET, &format!("/api/v1/code/findings?node_id={a}"), None).await;
    let f = r.json()["findings"].as_array().unwrap().clone();
    assert!(f.iter().any(|x| x["rule"] == "bit-precedence" && x["line"] == 12 && x["symbol"] == "check" && x["path"] == "/fw/led.c"), "{f:?}");
    let r = t.admin(Method::GET, &format!("/api/v1/code/findings?node_id={fw}&severity=error"), None).await;
    let rules: Vec<_> = r.json()["findings"].as_array().unwrap().iter().map(|x| x["rule"].as_str().unwrap().to_string()).collect();
    assert!(rules.contains(&"pio-set-range".to_string()) && rules.contains(&"bit-precedence".to_string()), "{rules:?}");
    assert!(rules.iter().all(|r| r != "irq-blocking-call"));
    let r = t.admin(Method::GET, "/api/v1/code/findings?severity=bogus", None).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = t.admin(Method::GET, &format!("/api/v1/code/findings?node_id={s}"), None).await;
    assert_eq!(r.json()["findings"].as_array().unwrap().len(), 0, "sensitive file not analysed");

    // Duplicates: the renamed copy in another folder.
    let r = t.admin(Method::GET, &format!("/api/v1/code/duplicates?node_id={a}"), None).await;
    let d = r.json()["duplicates"].as_array().unwrap().clone();
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0]["b"]["path"], "/other/copy.c");
    assert!(d[0]["a"]["line_start"].as_i64().unwrap() <= 4 && d[0]["a"]["line_end"].as_i64().unwrap() >= 8, "{d:?}");
    let r = t.admin(Method::GET, "/api/v1/code/duplicates", None).await;
    assert_eq!(r.json()["duplicates"].as_array().unwrap().len(), 1, "pair reported once");

    // Symbols.
    let r = t.admin(Method::GET, "/api/v1/code/symbols?q=fill_buffer", None).await;
    let sy = r.json()["symbols"].as_array().unwrap().clone();
    assert_eq!(sy.len(), 1);
    assert_eq!((sy[0]["path"].as_str(), sy[0]["line_start"].as_i64()), (Some("/fw/led.c"), Some(3)));
    assert_eq!(t.admin(Method::GET, "/api/v1/code/symbols?q=p", None).await.json()["symbols"][0]["kind"], "pio_program");

    // Editing the file re-analyses it; the bug is gone.
    let r = t
        .raw(
            Method::PUT,
            &format!("/api/v1/nodes/{a}/content"),
            &[("cookie", &t.cookie), ("x-csrf-token", &t.csrf), ("if-match", &format!("\"{}\"", sha(LED_C.as_bytes())))],
            LED_C.replace("x & 0x4 == 0", "(x & 0x4) == 0").into_bytes(),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
    assert_eq!(t.state.code.index_pending(&t.state.db, &t.state.store).await.unwrap(), 1);
    let r = t.admin(Method::GET, &format!("/api/v1/code/findings?node_id={a}&severity=error"), None).await;
    assert_eq!(r.json()["findings"].as_array().unwrap().len(), 0, "{:?}", r.json());

    // API key scoped to /fw: sees only /fw; the copy in /other is out of scope.
    let k = t.key(json!({"name":"k","permissions":{"list":true,"read":true},"scope_node_id": fw})).await;
    let r = t.bearer(&k, Method::GET, &format!("/api/v1/code/duplicates?node_id={a}"), vec![]).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
    assert_eq!(r.json()["duplicates"].as_array().unwrap().len(), 0);
    let r = t.bearer(&k, Method::GET, &format!("/api/v1/code/findings?node_id={b}"), vec![]).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = t.bearer(&k, Method::GET, "/api/v1/code/symbols?q=paint", vec![]).await;
    assert_eq!(r.json()["symbols"].as_array().unwrap().len(), 0);
    // Without read: forbidden.
    let k2 = t.key(json!({"name":"k2","permissions":{"list":true},"scope_node_id": fw})).await;
    let r = t.bearer(&k2, Method::GET, "/api/v1/code/findings", vec![]).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // Share links: no code analysis.
    let (_, tok, _) = make_share(&t, json!({"node_id": fw})).await;
    let r = as_share(&t, &tok, Method::GET, "/api/v1/code/findings", vec![]).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // Rules catalogue; reindex is admin-only.
    let r = t.admin(Method::GET, "/api/v1/code/rules", None).await;
    assert!(r.json()["rules"].as_array().unwrap().len() > 30);
    let r = t.bearer(&k, Method::POST, "/api/v1/code/reindex", vec![]).await;
    assert!(r.status == StatusCode::FORBIDDEN || r.status == StatusCode::UNAUTHORIZED);
    let r = t.admin(Method::POST, "/api/v1/code/reindex", None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(t.state.code.index_pending(&t.state.db, &t.state.store).await.unwrap(), 4);

    // Trashing a file removes its results.
    t.admin(Method::DELETE, &format!("/api/v1/nodes/{b}"), None).await;
    t.state.code.index_pending(&t.state.db, &t.state.store).await.unwrap();
    let r = t.admin(Method::GET, "/api/v1/code/duplicates", None).await;
    assert_eq!(r.json()["duplicates"].as_array().unwrap().len(), 0);
}
