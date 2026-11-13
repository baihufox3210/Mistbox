//! Isolated LaTeX compile worker. Runs in its own container (see Dockerfile
//! target `latex`): read-only root fs, tmpfs /tmp, no data volume, internal
//! network only, non-root, all capabilities dropped, pids/memory/CPU limits.
//!
//! Per job: fresh temp dir containing only the files the app sent; engine run
//! with `-no-shell-escape`, `shell_escape=f`, `openin_any=p`, `openout_any=p`,
//! a cleared environment, rlimits (CPU, address space, file size, processes,
//! open files, no core dumps) and a wall-clock timeout that kills the whole
//! process group. The temp dir is removed afterwards.
//!
//! It also extracts text from PDFs (`POST /pdftotext`, poppler) for semantic
//! search, so untrusted PDFs are never parsed inside the app container.

use std::{
    io::Read,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::json;
use tokio::sync::Semaphore;

#[derive(Clone)]
struct Limits {
    timeout: Duration,
    cpu_secs: u64,
    mem_bytes: u64,
    max_output: u64,
    max_files: usize,
    work_dir: PathBuf,
}

#[derive(Clone)]
struct St {
    lim: Arc<Limits>,
    sem: Arc<Semaphore>,
    pdf_sem: Arc<Semaphore>,
}

fn env_u64(k: &str, d: u64) -> u64 {
    std::env::var(k)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(d)
}

#[tokio::main]
async fn main() {
    let lim = Limits {
        timeout: Duration::from_secs(env_u64("LATEX_TIMEOUT_SECS", 60)),
        cpu_secs: env_u64("LATEX_CPU_SECS", 45),
        mem_bytes: env_u64("LATEX_MEMORY_MB", 1536) * 1024 * 1024,
        max_output: env_u64("LATEX_MAX_OUTPUT_MB", 50) * 1024 * 1024,
        max_files: env_u64("LATEX_MAX_FILES", 300) as usize,
        work_dir: PathBuf::from(std::env::var("LATEX_WORK_DIR").unwrap_or_else(|_| "/tmp".into())),
    };
    let st = St {
        lim: Arc::new(lim),
        sem: Arc::new(Semaphore::new(env_u64("LATEX_CONCURRENCY", 2) as usize)),
        pdf_sem: Arc::new(Semaphore::new(1)),
    };
    let bind = std::env::var("LATEX_BIND").unwrap_or_else(|_| "0.0.0.0:8090".into());
    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/compile", post(compile))
        .route("/pdftotext", post(pdftotext))
        .layer(DefaultBodyLimit::max(
            env_u64("LATEX_MAX_INPUT_MB", 64) as usize * 1024 * 1024,
        ))
        .with_state(st);
    let listener = tokio::net::TcpListener::bind(&bind).await.expect("bind");
    eprintln!("latex-worker listening on {bind}");
    axum::serve(listener, app).await.unwrap();
}

fn err(status: StatusCode, code: &str, message: impl Into<String>, log: &str) -> Response {
    (
        status,
        Json(json!({ "code": code, "message": message.into(), "log": log })),
    )
        .into_response()
}

struct JobDir(PathBuf);
impl Drop for JobDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn safe_rel(p: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => out.push(s),
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

fn extract(body: &[u8], dir: &Path, max_files: usize) -> Result<usize, String> {
    let mut ar = tar::Archive::new(body);
    let mut n = 0;
    for e in ar.entries().map_err(|e| e.to_string())? {
        let mut e = e.map_err(|e| e.to_string())?;
        if e.header().entry_type() != tar::EntryType::Regular {
            return Err("只接受一般檔案".into());
        }
        let rel = safe_rel(&e.path().map_err(|e| e.to_string())?).ok_or("不安全的路徑")?;
        n += 1;
        if n > max_files {
            return Err("檔案數量超過限制".into());
        }
        let dest = dir.join(&rel);
        std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
        let mut buf = Vec::new();
        e.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        std::fs::write(&dest, buf).map_err(|e| e.to_string())?;
    }
    Ok(n)
}

enum RunEnd {
    Exit(i32),
    Signal(i32),
    Timeout,
}

async fn run(
    prog: &str,
    args: &[&str],
    job: &Path,
    var: &Path,
    lim: &Limits,
    deadline: Instant,
) -> std::io::Result<(RunEnd, String)> {
    // Console output goes to a file (bounded by RLIMIT_FSIZE) so a chatty
    // document can never block on a full pipe.
    let out_path = var.join("console.txt");
    let out_file = std::fs::File::create(&out_path)?;
    let mut cmd = tokio::process::Command::new(prog);
    cmd.args(args)
        .current_dir(job)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", var)
        .env("TEXMFVAR", var.join("texmf-var"))
        .env("TEXMFCONFIG", var.join("texmf-config"))
        .env("TEXMFHOME", var.join("texmf-home"))
        .env("TEXMFOUTPUT", job)
        .env("shell_escape", "f")
        .env("openout_any", "p")
        .env("openin_any", "p")
        .env("max_print_line", "1000")
        .env("LANG", "C.UTF-8")
        .stdin(std::process::Stdio::null())
        .stdout(out_file.try_clone()?)
        .stderr(out_file)
        .kill_on_drop(true);
    let (cpu, mem, fsize) = (lim.cpu_secs, lim.mem_bytes, lim.max_output);
    unsafe {
        cmd.pre_exec(move || {
            // New session/process group so a timeout can kill every child.
            libc::setsid();
            let set = |res, v: u64| {
                let r = libc::rlimit {
                    rlim_cur: v,
                    rlim_max: v,
                };
                libc::setrlimit(res, &r)
            };
            set(libc::RLIMIT_CPU, cpu);
            set(libc::RLIMIT_AS, mem);
            set(libc::RLIMIT_FSIZE, fsize);
            set(libc::RLIMIT_NOFILE, 256);
            set(libc::RLIMIT_CORE, 0);
            set(libc::RLIMIT_NPROC, 512);
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    let pid = child.id().unwrap_or(0) as i32;
    let remaining = deadline.saturating_duration_since(Instant::now());
    let end = match tokio::time::timeout(remaining, child.wait()).await {
        Ok(Ok(st)) => {
            use std::os::unix::process::ExitStatusExt;
            match (st.code(), st.signal()) {
                (Some(c), _) => RunEnd::Exit(c),
                (_, Some(s)) => RunEnd::Signal(s),
                _ => RunEnd::Exit(-1),
            }
        }
        Ok(Err(e)) => return Err(e),
        Err(_) => {
            if pid > 0 {
                unsafe { libc::kill(-pid, libc::SIGKILL) };
            }
            let _ = child.kill().await;
            RunEnd::Timeout
        }
    };
    if pid > 0 {
        // Reap anything left in the group.
        unsafe { libc::kill(-pid, libc::SIGKILL) };
    }
    let out = std::fs::read(&out_path)
        .map(|b| String::from_utf8_lossy(&b[b.len().saturating_sub(256 * 1024)..]).into_owned())
        .unwrap_or_default();
    Ok((end, out))
}

fn tail(s: &str, lines: usize) -> String {
    let v: Vec<&str> = s.lines().collect();
    v[v.len().saturating_sub(lines)..].join("\n")
}

/// First error message from a TeX log (file-line-error or "!" style).
fn first_error(log: &str) -> Option<String> {
    let lines: Vec<&str> = log.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        let is_fle = l.contains(".tex:")
            && l.split(':')
                .nth(1)
                .is_some_and(|n| n.trim().parse::<u32>().is_ok());
        if l.starts_with('!') || is_fle {
            let mut msg = l.to_string();
            if let Some(next) = lines.get(i + 1).filter(|n| n.starts_with("l.")) {
                msg.push('\n');
                msg.push_str(next);
            }
            return Some(msg);
        }
    }
    None
}

fn missing_file(log: &str) -> Option<String> {
    let re = regex::Regex::new(r"(?:File|file) `([^']+)' not found").unwrap();
    re.captures(log).map(|c| c[1].to_string())
}

async fn compile(State(st): State<St>, headers: HeaderMap, body: Bytes) -> Response {
    let lim = st.lim.clone();
    let Ok(Ok(_permit)) =
        tokio::time::timeout(Duration::from_secs(60), st.sem.clone().acquire_owned()).await
    else {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "latex_busy",
            "編譯佇列忙碌",
            "",
        );
    };
    let engine = headers
        .get("x-engine")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("xelatex");
    if !matches!(engine, "xelatex" | "pdflatex") {
        return err(StatusCode::BAD_REQUEST, "bad_engine", "不支援的引擎", "");
    }
    let main = headers
        .get("x-main-file")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            percent_encoding::percent_decode_str(v)
                .decode_utf8()
                .ok()
                .map(|s| s.into_owned())
        })
        .unwrap_or_default();
    if main.is_empty()
        || main.contains('/')
        || main.starts_with('.')
        || !main.to_ascii_lowercase().ends_with(".tex")
    {
        return err(StatusCode::BAD_REQUEST, "bad_main", "主檔名無效", "");
    }

    let id = {
        let mut b = [0u8; 12];
        getrandom::fill(&mut b).unwrap();
        hex::encode(b)
    };
    let job = JobDir(lim.work_dir.join(format!("job-{id}")));
    let var = JobDir(lim.work_dir.join(format!("var-{id}")));
    for d in [&job.0, &var.0] {
        if let Err(e) = std::fs::create_dir_all(d) {
            return err(StatusCode::INTERNAL_SERVER_ERROR, "io", e.to_string(), "");
        }
    }
    if let Err(e) = extract(&body, &job.0, lim.max_files) {
        return err(StatusCode::BAD_REQUEST, "bad_archive", e, "");
    }
    if !job.0.join(&main).is_file() {
        return err(StatusCode::BAD_REQUEST, "bad_main", "主檔不存在", "");
    }

    let stem = main[..main.len() - 4].to_string();
    let deadline = Instant::now() + lim.timeout;
    let args = [
        "-no-shell-escape",
        "-interaction=nonstopmode",
        "-halt-on-error",
        "-file-line-error",
        main.as_str(),
    ];
    let log_path = job.0.join(format!("{stem}.log"));
    let read_log = || {
        std::fs::read_to_string(&log_path)
            .or_else(|_| std::fs::read(&log_path).map(|b| String::from_utf8_lossy(&b).into_owned()))
            .unwrap_or_default()
    };

    let mut passes = 0;
    let mut did_bib = false;
    loop {
        passes += 1;
        let (end, out) = match run(engine, &args, &job.0, &var.0, &lim, deadline).await {
            Ok(r) => r,
            Err(e) => {
                return err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "latex_failed",
                    format!("無法啟動 {engine}：{e}"),
                    "",
                );
            }
        };
        let log = read_log();
        let shown = if log.is_empty() {
            out.clone()
        } else {
            log.clone()
        };
        match end {
            RunEnd::Timeout => {
                return err(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "latex_timeout",
                    format!("編譯超過 {} 秒時間限制，已終止", lim.timeout.as_secs()),
                    &tail(&shown, 80),
                );
            }
            RunEnd::Signal(s) if s == libc::SIGXCPU || s == libc::SIGKILL => {
                return err(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "latex_resource_limit",
                    format!("超過 CPU 時間限制（{} 秒）", lim.cpu_secs),
                    &tail(&shown, 80),
                );
            }
            RunEnd::Signal(s) if s == libc::SIGXFSZ => {
                return err(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "latex_output_too_large",
                    "輸出檔案超過大小限制",
                    &tail(&shown, 80),
                );
            }
            RunEnd::Signal(s) => {
                return err(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "latex_resource_limit",
                    format!("編譯程序被訊號 {s} 終止（可能超過記憶體限制）"),
                    &tail(&shown, 80),
                );
            }
            RunEnd::Exit(0) => {}
            RunEnd::Exit(_) => {
                if let Some(f) = missing_file(&shown) {
                    return err(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "latex_missing_file",
                        format!("找不到檔案或套件：{f}（未上傳、未授權，或不在支援的套件清單內）"),
                        &tail(&shown, 120),
                    );
                }
                if shown.contains("memory")
                    && (shown.contains("out of memory") || shown.contains("Cannot allocate"))
                {
                    return err(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "latex_resource_limit",
                        "超過記憶體限制",
                        &tail(&shown, 80),
                    );
                }
                let first = first_error(&shown).unwrap_or_else(|| "LaTeX 編譯失敗".into());
                return err(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "latex_failed",
                    first,
                    &tail(&shown, 120),
                );
            }
        }
        let aux = std::fs::read_to_string(job.0.join(format!("{stem}.aux"))).unwrap_or_default();
        if !did_bib && aux.contains("\\bibdata") {
            did_bib = true;
            let _ = run("bibtex", &[stem.as_str()], &job.0, &var.0, &lim, deadline).await;
            continue;
        }
        let needs_rerun = log.contains("Rerun to get")
            || log.contains("Label(s) may have changed")
            || (passes == 1
                && (aux.contains("\\newlabel")
                    || aux.contains("\\@writefile")
                    || aux.contains("\\bibcite")));
        if !needs_rerun || passes >= 4 {
            break;
        }
    }

    let pdf_path = job.0.join(format!("{stem}.pdf"));
    match std::fs::metadata(&pdf_path) {
        Ok(m) if m.len() > lim.max_output => err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "latex_output_too_large",
            "PDF 超過大小限制",
            "",
        ),
        Ok(_) => match std::fs::read(&pdf_path) {
            Ok(b) => ([(header::CONTENT_TYPE, "application/pdf")], b).into_response(),
            Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "io", e.to_string(), ""),
        },
        Err(_) => err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "latex_failed",
            "沒有產生 PDF（文件可能沒有內容）",
            &tail(&read_log(), 80),
        ),
    }
}

/// Extract UTF-8 text from one PDF (body). Pages are separated by form feeds.
async fn pdftotext(State(st): State<St>, body: Bytes) -> Response {
    let lim = st.lim.clone();
    let Ok(Ok(_permit)) =
        tokio::time::timeout(Duration::from_secs(120), st.pdf_sem.clone().acquire_owned()).await
    else {
        return err(
            StatusCode::SERVICE_UNAVAILABLE,
            "pdf_busy",
            "PDF 擷取佇列忙碌",
            "",
        );
    };
    let head = &body[..body.len().min(1024)];
    if !head.windows(5).any(|w| w == b"%PDF-") {
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "pdf_invalid",
            "不是有效的 PDF 檔",
            "",
        );
    }
    let id = {
        let mut b = [0u8; 12];
        getrandom::fill(&mut b).unwrap();
        hex::encode(b)
    };
    let job = JobDir(lim.work_dir.join(format!("pdf-{id}")));
    let var = JobDir(lim.work_dir.join(format!("pdfvar-{id}")));
    for d in [&job.0, &var.0] {
        if let Err(e) = std::fs::create_dir_all(d) {
            return err(StatusCode::INTERNAL_SERVER_ERROR, "io", e.to_string(), "");
        }
    }
    if let Err(e) = std::fs::write(job.0.join("in.pdf"), &body) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, "io", e.to_string(), "");
    }
    let max_pages = env_u64("PDF_MAX_PAGES", 2000).to_string();
    let deadline = Instant::now() + Duration::from_secs(env_u64("PDF_TIMEOUT_SECS", 60));
    let args = [
        "-enc",
        "UTF-8",
        "-eol",
        "unix",
        "-l",
        max_pages.as_str(),
        "in.pdf",
        "out.txt",
    ];
    let (end, out) = match run("pdftotext", &args, &job.0, &var.0, &lim, deadline).await {
        Ok(r) => r,
        Err(e) => {
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "pdf_failed",
                format!("無法啟動 pdftotext：{e}"),
                "",
            );
        }
    };
    match end {
        RunEnd::Exit(0) => {}
        RunEnd::Timeout => {
            return err(
                StatusCode::UNPROCESSABLE_ENTITY,
                "pdf_timeout",
                "PDF 文字擷取逾時",
                "",
            );
        }
        RunEnd::Signal(_) => {
            return err(
                StatusCode::UNPROCESSABLE_ENTITY,
                "pdf_resource_limit",
                "PDF 文字擷取超過資源限制",
                "",
            );
        }
        RunEnd::Exit(_) => {
            let msg = if out.contains("password") || out.contains("Incorrect") {
                "PDF 已加密，無法擷取文字"
            } else {
                "PDF 無法解析"
            };
            return err(
                StatusCode::UNPROCESSABLE_ENTITY,
                "pdf_failed",
                msg,
                &tail(&out, 20),
            );
        }
    }
    match std::fs::read(job.0.join("out.txt")) {
        Ok(b) => (
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            String::from_utf8_lossy(&b).into_owned(),
        )
            .into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, "io", e.to_string(), ""),
    }
}
