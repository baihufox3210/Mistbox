use std::{io::IsTerminal, net::SocketAddr, time::Duration};

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use mistbox::{AppState, api, auth, backup, config::Config, db, import, nodes, storage::Store};

#[derive(Parser)]
#[command(name = "mistbox", version, about = "Mistbox private cloud drive")]
struct Cli {
    #[command(flatten)]
    cfg: Config,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the web server (API + frontend).
    Serve,
    /// Exit 0 if the local server answers /api/v1/health (for container health checks).
    Healthcheck {
        #[arg(long, default_value = "http://127.0.0.1:8080/api/v1/health")]
        url: String,
    },
    /// Administrator account management.
    #[command(subcommand)]
    Admin(AdminCmd),
    /// Import a host folder (read-only) into the drive. Use --dry-run first.
    Import(import::ImportArgs),
    /// Write a consistent backup tar (database snapshot + files). "-" = stdout.
    Backup {
        #[arg(default_value = "-")]
        output: String,
    },
    /// Restore a backup tar into the data directory (server must be stopped). "-" = stdin.
    Restore {
        #[arg(default_value = "-")]
        input: String,
        /// Replace existing data (moved aside to pre-restore-<ts>/, not deleted).
        #[arg(long)]
        force: bool,
    },
    /// Re-hash every stored file and compare with recorded SHA-256.
    Verify,
    /// Remove blobs not referenced by any file (e.g. after a crash mid-upload).
    Gc,
    /// Semantic search (local embedding model) maintenance.
    #[command(subcommand)]
    Rag(RagCmd),
}

#[derive(Subcommand)]
enum RagCmd {
    /// Download an embedding model into MISTBOX_RAG_MODEL_DIR (used at image build time).
    DownloadModel {
        #[arg(default_value = mistbox::rag::DEFAULT_MODEL)]
        model: String,
    },
    /// Clear the index; the running server rebuilds it in the background.
    Reindex,
    /// Show indexing progress.
    Status,
    /// List supported models.
    Models,
    /// Load the configured models (text and code) from MISTBOX_RAG_MODEL_DIR and
    /// embed a sample: checks that this CPU can run them (no network access).
    Selftest,
}

#[derive(Subcommand)]
enum AdminCmd {
    /// Create the administrator account (only allowed when none exists).
    Create {
        #[arg(long)]
        username: String,
        /// Read the password from stdin instead of prompting.
        #[arg(long)]
        password_stdin: bool,
    },
    /// Set a new password (also logs out all sessions).
    ResetPassword {
        #[arg(long)]
        username: String,
        #[arg(long)]
        password_stdin: bool,
    },
    /// Log out every active session.
    RevokeSessions,
}

fn read_password(from_stdin: bool) -> anyhow::Result<String> {
    let pw = if from_stdin || !std::io::stdin().is_terminal() {
        let mut s = String::new();
        std::io::stdin().read_line(&mut s)?;
        s.trim_end_matches(['\r', '\n']).to_string()
    } else {
        let a = rpassword::prompt_password("新密碼: ")?;
        let b = rpassword::prompt_password("再次輸入: ")?;
        if a != b {
            bail!("兩次輸入的密碼不同");
        }
        a
    };
    auth::check_password_policy(&pw).map_err(|e| anyhow::anyhow!(e))?;
    Ok(pw)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Group-writable files so a one-off import container sharing the app's
    // group can write to the same volume (see README "來源匯入").
    unsafe { libc::umask(0o002) };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sqlx=warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let cfg = cli.cfg;
    match cli.cmd {
        Cmd::Serve => serve(cfg).await,
        Cmd::Healthcheck { url } => {
            let ok = reqwest::Client::builder()
                .timeout(Duration::from_secs(3))
                .no_proxy()
                .build()?
                .get(url)
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false);
            std::process::exit(if ok { 0 } else { 1 })
        }
        Cmd::Admin(cmd) => {
            let pool = db::open(&cfg.data_dir).await?;
            match cmd {
                AdminCmd::Create {
                    username,
                    password_stdin,
                } => {
                    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
                        .fetch_one(&pool)
                        .await?;
                    if n > 0 {
                        bail!("管理者已存在；請改用 `admin reset-password`");
                    }
                    let username = username.trim();
                    if username.is_empty() || username.len() > 64 {
                        bail!("帳號名稱需為 1–64 字元");
                    }
                    let pw = read_password(password_stdin)?;
                    auth::create_user(&pool, username, &pw).await?;
                    eprintln!("已建立管理者 {username}");
                }
                AdminCmd::ResetPassword {
                    username,
                    password_stdin,
                } => {
                    let id: Option<i64> =
                        sqlx::query_scalar("SELECT id FROM users WHERE username = ?")
                            .bind(&username)
                            .fetch_optional(&pool)
                            .await?;
                    let id = id.context("找不到此帳號")?;
                    let pw = read_password(password_stdin)?;
                    auth::set_password(&pool, id, &pw).await?;
                    eprintln!("已更新密碼並登出所有工作階段");
                }
                AdminCmd::RevokeSessions => {
                    let r = sqlx::query("DELETE FROM sessions").execute(&pool).await?;
                    eprintln!("已登出 {} 個工作階段", r.rows_affected());
                }
            }
            Ok(())
        }
        Cmd::Import(args) => {
            let pool = db::open(&cfg.data_dir).await?;
            let store = Store::new(&cfg.data_dir)?;
            let report = import::run(&pool, &store, &cfg.data_dir, &args).await?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                import::print_report(&report, args.show);
            }
            if !report.errors.is_empty() {
                std::process::exit(2);
            }
            Ok(())
        }
        Cmd::Backup { output } => {
            let pool = db::open(&cfg.data_dir).await?;
            backup::backup(&pool, &Store::new(&cfg.data_dir)?, &output).await
        }
        Cmd::Restore { input, force } => {
            tokio::task::spawn_blocking(move || backup::restore(&cfg.data_dir, &input, force))
                .await??;
            Ok(())
        }
        Cmd::Verify => {
            let pool = db::open(&cfg.data_dir).await?;
            let r = backup::verify(&pool, &Store::new(&cfg.data_dir)?).await?;
            println!("已檢查 {} 個檔案", r.checked);
            for p in &r.missing {
                println!("遺失: {p}");
            }
            for p in &r.corrupt {
                println!("損毀 (SHA-256 不符): {p}");
            }
            if !r.orphans.is_empty() {
                println!(
                    "未被引用的 blob: {} 個（可執行 `mistbox gc` 清除）",
                    r.orphans.len()
                );
            }
            if r.missing.is_empty() && r.corrupt.is_empty() {
                println!("完整性檢查通過");
                Ok(())
            } else {
                std::process::exit(3)
            }
        }
        Cmd::Rag(cmd) => match cmd {
            RagCmd::DownloadModel { model } => {
                let dir = cfg.rag_model_dir.clone();
                tokio::task::spawn_blocking(move || mistbox::rag::download_model(&model, &dir))
                    .await?
            }
            RagCmd::Selftest => {
                let (text, code, dir) = (cfg.rag_model.clone(), cfg.code_model.clone(), cfg.rag_model_dir.clone());
                tokio::task::spawn_blocking(move || mistbox::rag::selftest(&text, &code, &dir)).await?
            }
            RagCmd::Models => {
                for (name, desc, _, _) in mistbox::rag::MODELS {
                    println!("{name:34} {desc}");
                }
                Ok(())
            }
            RagCmd::Reindex | RagCmd::Status => {
                let pool = db::open(&cfg.data_dir).await?;
                if matches!(cmd, RagCmd::Reindex) {
                    sqlx::query("DELETE FROM rag_chunks").execute(&pool).await?;
                    sqlx::query("DELETE FROM rag_docs").execute(&pool).await?;
                    println!("已清除索引；執行中的伺服器會在 30 秒內開始重建");
                    return Ok(());
                }
                let rows: Vec<(String, String, i64, i64)> = sqlx::query_as(
                    "SELECT model, status, COUNT(*), COALESCE(SUM(chunks),0) FROM rag_docs GROUP BY model, status",
                )
                .fetch_all(&pool)
                .await?;
                for (m, st, n, c) in rows {
                    println!("{m:30} {st:8} {n:6} 個檔案  {c:7} 個片段");
                }
                Ok(())
            }
        },
        Cmd::Gc => {
            let pool = db::open(&cfg.data_dir).await?;
            let store = Store::new(&cfg.data_dir)?;
            let n = nodes::run_blob_gc(&pool, &store).await?;
            let r = backup::verify(&pool, &store).await?;
            for b in &r.orphans {
                store.remove_blob(b).await;
            }
            println!(
                "已清除 {} 個待刪除與 {} 個未被引用的 blob",
                n,
                r.orphans.len()
            );
            Ok(())
        }
    }
}

async fn serve(cfg: Config) -> anyhow::Result<()> {
    let bind: SocketAddr = cfg.bind.parse().context("MISTBOX_BIND")?;
    let state = AppState::new(cfg).await?;
    state.store.clean_tmp();
    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&state.db)
        .await?;
    if users == 0 {
        tracing::warn!(
            "尚未建立管理者。請執行：docker compose exec app mistbox admin create --username <name>"
        );
    }
    if state.latex.is_none() {
        tracing::warn!("MISTBOX_LATEX_URL 未設定：LaTeX PDF 編譯停用（仍可預覽原始碼）");
    }

    match &state.rag {
        Some(r) => r.spawn(state.db.clone(), state.store.clone()),
        None => tracing::warn!("語意搜尋停用（MISTBOX_RAG_MODEL=off，或未以 embed feature 編譯）"),
    }
    state.code.spawn(state.db.clone(), state.store.clone(), state.code_sem.clone());

    // Background maintenance: blob GC and expired sessions.
    {
        let st = state.clone();
        tokio::spawn(async move {
            loop {
                if let Err(e) = nodes::run_blob_gc(&st.db, &st.store).await {
                    tracing::warn!(error = %e, "blob gc failed");
                }
                if st.cfg.trash_days > 0 {
                    match nodes::purge_trash(&st.db, st.cfg.trash_days).await {
                        Ok(n) if n > 0 => {
                            tracing::info!(items = n, "recycle bin: expired items deleted")
                        }
                        Err(e) => tracing::warn!(error = %e.message, "recycle bin purge failed"),
                        _ => {}
                    }
                }
                let _ = sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
                    .bind(mistbox::util::now())
                    .execute(&st.db)
                    .await;
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        });
    }

    let app = api::router(state);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, "mistbox listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = term.recv() => {},
        }
    })
    .await?;
    Ok(())
}
