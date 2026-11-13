//! LaTeX: the app collects exactly the files a document references (and the
//! caller may read), packs them into a tar, and sends them to the isolated
//! worker container (`latex-worker`), which has no network route outside the
//! internal compose network and no access to the data volume.

use std::{collections::HashSet, sync::OnceLock, time::Duration};

use axum::http::StatusCode;
use regex::Regex;
use serde::Serialize;
use tokio::sync::Semaphore;

use crate::{
    auth::Principal,
    authz::{self, Perm},
    config::Config,
    error::{ApiError, ApiResult},
    nodes::{self, Node},
    util::sha256_hex,
    AppState,
};

pub const ENGINES: &[&str] = &["xelatex", "pdflatex"];
const MAX_FILES: usize = 300;
const MAX_PARSE_BYTES: u64 = 4 * 1024 * 1024;

pub struct LatexClient {
    base: String,
    http: reqwest::Client,
    sem: Semaphore,
    max_input: u64,
}

#[derive(Serialize, Debug)]
pub struct Collected {
    pub engine: String,
    /// Paths relative to the main file's directory.
    pub files: Vec<String>,
    /// References that were not found or are not authorized.
    pub missing: Vec<String>,
    #[serde(skip)]
    nodes: Vec<(String, Node)>,
    #[serde(skip)]
    pub cache_key: String,
}

fn re(pat: &'static str, cell: &'static OnceLock<Regex>) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).unwrap())
}

fn strip_comments(src: &str) -> String {
    src.lines()
        .map(|line| {
            let b = line.as_bytes();
            let mut i = 0;
            while i < b.len() {
                if b[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if b[i] == b'%' {
                    return &line[..i];
                }
                i += 1;
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Detect `% !TEX program = <engine>` in the first lines.
pub fn detect_engine(src: &str) -> String {
    static R: OnceLock<Regex> = OnceLock::new();
    let r = re(r"(?im)^\s*%\s*!\s*TEX\s+(?:TS-)?program\s*=\s*([a-z]+)", &R);
    let head: String = src.lines().take(30).collect::<Vec<_>>().join("\n");
    r.captures(&head)
        .map(|c| c[1].to_ascii_lowercase())
        .filter(|e| ENGINES.contains(&e.as_str()))
        .unwrap_or_else(|| "xelatex".into())
}

#[derive(Clone, Copy)]
enum RefKind {
    Tex,
    Graphic,
    Bib,
    Package,
    Class,
    Plain,
}

/// Extract (kind, raw reference) pairs from TeX source.
fn references(src: &str) -> (Vec<(RefKind, String)>, Vec<String>) {
    static INPUT: OnceLock<Regex> = OnceLock::new();
    static GFX: OnceLock<Regex> = OnceLock::new();
    static BIB: OnceLock<Regex> = OnceLock::new();
    static PKG: OnceLock<Regex> = OnceLock::new();
    static CLS: OnceLock<Regex> = OnceLock::new();
    static PLAIN: OnceLock<Regex> = OnceLock::new();
    static GPATH: OnceLock<Regex> = OnceLock::new();
    let src = strip_comments(src);
    let mut out = Vec::new();
    let mut push_list = |kind, s: &str| {
        for part in s.split(',') {
            let p = part.trim();
            if !p.is_empty() {
                out.push((kind, p.to_string()));
            }
        }
    };
    for c in re(r"\\(?:input|include|subfile|subfileinclude)\s*\{([^}]+)\}", &INPUT).captures_iter(&src) {
        push_list(RefKind::Tex, &c[1]);
    }
    for c in re(r"\\includegraphics\s*\*?\s*(?:\[[^\]]*\])?\s*\{([^}]+)\}", &GFX).captures_iter(&src) {
        push_list(RefKind::Graphic, &c[1]);
    }
    for c in re(r"\\(?:bibliography|addbibresource)\s*(?:\[[^\]]*\])?\s*\{([^}]+)\}", &BIB).captures_iter(&src) {
        push_list(RefKind::Bib, &c[1]);
    }
    for c in re(r"\\(?:usepackage|RequirePackage)\s*(?:\[[^\]]*\])?\s*\{([^}]+)\}", &PKG).captures_iter(&src) {
        push_list(RefKind::Package, &c[1]);
    }
    for c in re(r"\\documentclass\s*(?:\[[^\]]*\])?\s*\{([^}]+)\}", &CLS).captures_iter(&src) {
        push_list(RefKind::Class, &c[1]);
    }
    for c in re(r"\\(?:lstinputlisting|verbatiminput|inputminted)\s*(?:\[[^\]]*\])?\s*(?:\{[^}]*\}\s*)?\{([^}]+)\}", &PLAIN)
        .captures_iter(&src)
    {
        out.push((RefKind::Plain, c[1].trim().to_string()));
    }
    let mut gpaths = Vec::new();
    if let Some(c) = re(r"\\graphicspath\s*\{((?:\s*\{[^}]*\})+)\s*\}", &GPATH).captures(&src) {
        for p in c[1].split('}') {
            let p = p.trim().trim_start_matches('{').trim();
            if !p.is_empty() {
                gpaths.push(p.trim_end_matches('/').to_string());
            }
        }
    }
    (out, gpaths)
}

/// Normalise a reference to a relative path inside the main directory.
/// Absolute paths and anything climbing above the main directory are refused.
fn normalise(raw: &str) -> Option<String> {
    let raw = raw.trim().trim_matches('"');
    if raw.is_empty() || raw.starts_with('/') || raw.starts_with('~') || raw.contains('\\') || raw.contains(':') {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for seg in raw.split('/') {
        match seg {
            "" | "." => {}
            ".." => return None,
            s => parts.push(s),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn candidates(kind: RefKind, raw: &str, gpaths: &[String]) -> Vec<String> {
    let Some(base) = normalise(raw) else { return vec![] };
    let has_ext = base.rsplit('/').next().is_some_and(|f| f.contains('.'));
    let exts: &[&str] = match kind {
        RefKind::Tex => &[".tex"],
        RefKind::Graphic => &[".pdf", ".png", ".jpg", ".jpeg", ".eps"],
        RefKind::Bib => &[".bib"],
        RefKind::Package => &[".sty"],
        RefKind::Class => &[".cls"],
        RefKind::Plain => &[],
    };
    let mut prefixes = vec![String::new()];
    if matches!(kind, RefKind::Graphic) {
        prefixes.extend(gpaths.iter().filter_map(|g| normalise(g)).map(|g| format!("{g}/")));
    }
    let mut out = Vec::new();
    for p in &prefixes {
        let b = format!("{p}{base}");
        if has_ext || exts.is_empty() {
            out.push(b.clone());
        }
        // `\bibliography{refs}` and `\input{chap}` may also have dotted names.
        if !has_ext || matches!(kind, RefKind::Tex | RefKind::Bib) {
            for e in exts {
                if !b.ends_with(e) {
                    out.push(format!("{b}{e}"));
                }
            }
        }
    }
    out
}

async fn read_blob_limited(state: &AppState, node: &Node, limit: u64) -> ApiResult<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let f = tokio::fs::File::open(state.store.blob_path(node.blob_id.as_deref().unwrap_or_default())).await?;
    let mut buf = Vec::new();
    f.take(limit).read_to_end(&mut buf).await?;
    Ok(buf)
}

/// Gather the main file plus every referenced, existing, authorized file
/// below the main file's directory.
pub async fn collect(state: &AppState, p: &Principal, main: &Node, max_input: u64) -> ApiResult<Collected> {
    let base_dir = main.parent_id.clone().ok_or_else(ApiError::not_found)?;
    let main_src = String::from_utf8_lossy(&read_blob_limited(state, main, MAX_PARSE_BYTES).await?).into_owned();
    let engine = detect_engine(&main_src);

    let mut nodes_out: Vec<(String, Node)> = vec![(main.name.clone(), main.clone())];
    let mut seen: HashSet<String> = HashSet::from([main.name.clone()]);
    let mut missing = Vec::new();
    let mut queue: Vec<(String, String)> = vec![(main.name.clone(), main_src)];
    let mut total = main.size as u64;

    while let Some((_, src)) = queue.pop() {
        let (refs, gpaths) = references(&src);
        for (kind, raw) in refs {
            let mut found = false;
            for cand in candidates(kind, &raw, &gpaths) {
                if seen.contains(&cand) {
                    found = true;
                    break;
                }
                let Ok(node) = nodes::resolve_rel(&state.db, &base_dir, &cand, nodes::ROOT_ID).await else { continue };
                if !node.is_file() || authz::require(state, p, &node, Perm::Read).await.is_err() {
                    continue;
                }
                found = true;
                seen.insert(cand.clone());
                total += node.size as u64;
                if nodes_out.len() >= MAX_FILES || total > max_input {
                    return Err(ApiError::too_large("latex_input_too_large", "LaTeX 輸入檔案過多或總大小超過限制"));
                }
                let lower = cand.to_ascii_lowercase();
                if lower.ends_with(".tex") || lower.ends_with(".sty") || lower.ends_with(".cls") {
                    let s = String::from_utf8_lossy(&read_blob_limited(state, &node, MAX_PARSE_BYTES).await?).into_owned();
                    queue.push((cand.clone(), s));
                }
                nodes_out.push((cand, node));
                break;
            }
            // Packages/classes normally come from TeX Live; only report our own misses.
            if !found && !matches!(kind, RefKind::Package | RefKind::Class) {
                missing.push(raw);
            }
        }
    }

    let mut entries: Vec<String> =
        nodes_out.iter().map(|(rel, n)| format!("{rel}\t{}", n.sha256.as_deref().unwrap_or(""))).collect();
    entries.sort();
    let cache_key = sha256_hex(format!("v1\n{engine}\n{}\n{}", main.name, entries.join("\n")));
    missing.sort();
    missing.dedup();
    Ok(Collected {
        engine,
        files: nodes_out.iter().map(|(r, _)| r.clone()).collect(),
        missing,
        nodes: nodes_out,
        cache_key,
    })
}

pub enum Outcome {
    Pdf(std::path::PathBuf),
}

impl LatexClient {
    pub fn new(base: String, cfg: &Config) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(150))
            .no_proxy()
            .build()
            .expect("http client");
        Self { base: base.trim_end_matches('/').to_string(), http, sem: Semaphore::new(2), max_input: cfg.latex_max_input_bytes }
    }

    pub fn max_input(&self) -> u64 {
        self.max_input
    }

    pub fn cache_path(state: &AppState, key: &str) -> std::path::PathBuf {
        state.store.latex_cache_dir().join(format!("{key}.pdf"))
    }

    pub async fn compile(&self, state: &AppState, main: &Node, c: &Collected) -> ApiResult<Outcome> {
        let cached = Self::cache_path(state, &c.cache_key);
        if tokio::fs::metadata(&cached).await.is_ok() {
            return Ok(Outcome::Pdf(cached));
        }
        // Build the job archive in memory (bounded by max_input).
        let mut builder = tar::Builder::new(Vec::new());
        for (rel, node) in &c.nodes {
            let data = read_blob_limited(state, node, self.max_input).await?;
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_entry_type(tar::EntryType::Regular);
            builder.append_data(&mut h, rel, data.as_slice()).map_err(|e| ApiError::from(anyhow::anyhow!(e)))?;
        }
        let body = builder.into_inner().map_err(|e| ApiError::from(anyhow::anyhow!(e)))?;

        let _permit = tokio::time::timeout(Duration::from_secs(120), self.sem.acquire())
            .await
            .map_err(|_| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "latex_busy", "LaTeX 編譯佇列忙碌，請稍後再試"))?
            .map_err(|_| ApiError::internal())?;

        let main_enc = percent_encoding::utf8_percent_encode(&main.name, percent_encoding::NON_ALPHANUMERIC).to_string();
        let resp = self
            .http
            .post(format!("{}/compile", self.base))
            .header("x-main-file", main_enc)
            .header("x-engine", &c.engine)
            .header("content-type", "application/x-tar")
            .body(body)
            .send()
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "latex worker unreachable");
                ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "latex_unavailable", "LaTeX 編譯服務目前無法使用")
            })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|_| ApiError::new(StatusCode::BAD_GATEWAY, "latex_unavailable", "LaTeX 編譯服務回應中斷"))?;
        if status.is_success() {
            let tmp = state.store.root().join("tmp").join(format!("{}.pdf", crate::util::random_hex(8)));
            tokio::fs::write(&tmp, &bytes).await?;
            tokio::fs::rename(&tmp, &cached).await?;
            return Ok(Outcome::Pdf(cached));
        }
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
        let code = match v["code"].as_str() {
            Some("latex_timeout") => "latex_timeout",
            Some("latex_missing_file") => "latex_missing_file",
            Some("latex_resource_limit") => "latex_resource_limit",
            Some("latex_output_too_large") => "latex_output_too_large",
            _ => "latex_failed",
        };
        let msg = v["message"].as_str().unwrap_or("LaTeX 編譯失敗").to_string();
        let _ = main;
        Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, code, msg).with_details(serde_json::json!({
            "log": v["log"].as_str().unwrap_or(""),
            "missing": c.missing,
            "engine": c.engine,
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_and_comments() {
        let src = r"
% \input{commented}
\documentclass{article}
\usepackage{amsmath,mystyle}
\graphicspath{{figs/}{img/}}
\begin{document}
\input{chapters/one}
\includegraphics[width=3cm]{cat}
\bibliography{refs,more}
50\% done \include{two}
\input{../../etc/passwd}
\input{/etc/passwd}
\end{document}";
        let (refs, gp) = references(src);
        let raws: Vec<_> = refs.iter().map(|(_, r)| r.as_str()).collect();
        assert!(!raws.contains(&"commented"));
        assert!(raws.contains(&"chapters/one") && raws.contains(&"cat") && raws.contains(&"two"));
        assert!(raws.contains(&"refs") && raws.contains(&"more") && raws.contains(&"mystyle"));
        assert_eq!(gp, vec!["figs", "img"]);
        assert!(candidates(RefKind::Tex, "../../etc/passwd", &gp).is_empty());
        assert!(candidates(RefKind::Tex, "/etc/passwd", &gp).is_empty());
        let c = candidates(RefKind::Graphic, "cat", &gp);
        assert!(c.contains(&"figs/cat.png".to_string()) && c.contains(&"cat.pdf".to_string()));
    }

    #[test]
    fn engine_magic() {
        assert_eq!(detect_engine("% !TEX program = pdflatex\n\\documentclass{article}"), "pdflatex");
        assert_eq!(detect_engine("% !TEX program = evil\n"), "xelatex");
        assert_eq!(detect_engine("\\documentclass{article}"), "xelatex");
    }
}
