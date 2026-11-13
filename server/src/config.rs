use std::path::PathBuf;

/// Runtime configuration. Every option can be set via environment variable.
#[derive(clap::Args, Clone, Debug)]
pub struct Config {
    /// Directory holding the SQLite database and blob store.
    #[arg(long, env = "MISTBOX_DATA_DIR", default_value = "./data", global = true)]
    pub data_dir: PathBuf,

    /// Address to listen on.
    #[arg(long, env = "MISTBOX_BIND", default_value = "127.0.0.1:8080", global = true)]
    pub bind: String,

    /// Directory with the built frontend (index.html + assets). Empty = API only.
    #[arg(long, env = "MISTBOX_STATIC_DIR", default_value = "", global = true)]
    pub static_dir: String,

    /// Maximum size of a single uploaded file in bytes.
    #[arg(long, env = "MISTBOX_MAX_UPLOAD_BYTES", default_value_t = 10 * 1024 * 1024 * 1024, global = true)]
    pub max_upload_bytes: u64,

    /// Maximum bytes decoded for a text/code/Markdown preview.
    #[arg(long, env = "MISTBOX_PREVIEW_MAX_BYTES", default_value_t = 2 * 1024 * 1024, global = true)]
    pub preview_max_bytes: u64,

    /// Mark the session cookie `Secure` (required behind HTTPS; browsers also accept it on http://localhost).
    #[arg(long, env = "MISTBOX_COOKIE_SECURE", default_value_t = true, action = clap::ArgAction::Set, global = true)]
    pub cookie_secure: bool,

    /// Session lifetime in hours.
    #[arg(long, env = "MISTBOX_SESSION_TTL_HOURS", default_value_t = 12, global = true)]
    pub session_ttl_hours: i64,

    /// Base URL of the isolated LaTeX worker (e.g. http://latex:8090). Empty disables PDF compilation.
    #[arg(long, env = "MISTBOX_LATEX_URL", global = true)]
    pub latex_url: Option<String>,

    /// Maximum total bytes of a LaTeX job's inputs.
    #[arg(long, env = "MISTBOX_LATEX_MAX_INPUT_BYTES", default_value_t = 50 * 1024 * 1024, global = true)]
    pub latex_max_input_bytes: u64,

    /// Use the right-most X-Forwarded-For entry as client IP (only behind a trusted reverse proxy).
    #[arg(long, env = "MISTBOX_TRUST_PROXY", default_value_t = false, action = clap::ArgAction::Set, global = true)]
    pub trust_proxy: bool,

    /// Login attempts allowed per client IP per 15 minutes.
    #[arg(long, env = "MISTBOX_LOGIN_ATTEMPTS", default_value_t = 10, global = true)]
    pub login_attempts: u32,

    /// Directory with admin-provided theme files (custom.css, background images), served at /theme/.
    #[arg(long, env = "MISTBOX_THEME_DIR", default_value = "", global = true)]
    pub theme_dir: String,

    /// Embedding model for semantic search: multilingual-e5-small (default when built with
    /// the `embed` feature), multilingual-e5-base, bge-small-zh, paraphrase-multilingual-minilm,
    /// `hash` (lexical test embedder) or `off`.
    #[arg(long, env = "MISTBOX_RAG_MODEL", default_value = "auto", global = true)]
    pub rag_model: String,

    /// Directory holding downloaded embedding models (baked into the Docker image).
    #[arg(long, env = "MISTBOX_RAG_MODEL_DIR", default_value = "./models", global = true)]
    pub rag_model_dir: std::path::PathBuf,

    /// Files larger than this are not indexed for semantic search.
    #[arg(long, env = "MISTBOX_RAG_MAX_FILE_BYTES", default_value_t = 1024 * 1024, global = true)]
    pub rag_max_file_bytes: u64,

    /// CPU threads used by the embedding model.
    #[arg(long, env = "MISTBOX_RAG_THREADS", default_value_t = 4, global = true)]
    pub rag_threads: usize,

    /// Days before items in the recycle bin are permanently deleted (0 = never).
    #[arg(long, env = "MISTBOX_TRASH_DAYS", default_value_t = 30, global = true)]
    pub trash_days: i64,

    /// API requests allowed per minute per API key / session / IP.
    #[arg(long, env = "MISTBOX_API_RATE_PER_MIN", default_value_t = 1200, global = true)]
    pub api_rate_per_min: u32,
}

impl Config {
    /// Configuration for tests: everything local, small limits.
    pub fn for_tests(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            bind: "127.0.0.1:0".into(),
            static_dir: String::new(),
            max_upload_bytes: 4 * 1024 * 1024,
            preview_max_bytes: 64 * 1024,
            cookie_secure: false,
            session_ttl_hours: 12,
            latex_url: None,
            latex_max_input_bytes: 10 * 1024 * 1024,
            trust_proxy: false,
            login_attempts: 10,
            api_rate_per_min: 100_000,
            theme_dir: String::new(),
            trash_days: 30,
            rag_model: "hash".into(),
            rag_model_dir: "./models".into(),
            rag_max_file_bytes: 256 * 1024,
            rag_threads: 1,
        }
    }
}
