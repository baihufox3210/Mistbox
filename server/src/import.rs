//! `mistbox import <SOURCE>` — copy a host folder into the drive.
//!
//! Guarantees:
//! - The source is only ever opened read-only (O_RDONLY | O_NOFOLLOW); nothing
//!   in it is moved, modified or deleted. Mount it `:ro` as well.
//! - Directories containing the Mistbox data marker are never descended into
//!   (no self-import / recursion).
//! - Nothing is skipped silently: every skipped entry is listed with a reason.
//! - Re-running is idempotent: identical files (same SHA-256) are "unchanged";
//!   differing files with the same path are conflicts handled by `--on-conflict`
//!   and are never overwritten.

use std::{
    collections::HashMap,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

use anyhow::{bail, Context};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::{
    db::DATA_MARKER,
    nodes::{self, ConflictPolicy, Node},
    sensitive,
    storage::Store,
    util::guess_mime,
};

#[derive(clap::ValueEnum, Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SymlinkPolicy {
    /// Never follow symbolic links; list and skip them.
    Skip,
    /// Follow links to regular files that resolve *inside* the source and store
    /// a copy of the target's content. Directory links and links leaving the
    /// source are skipped.
    FollowInside,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ImportConflict {
    /// Keep the existing drive file, skip the source file (reported).
    Skip,
    /// Store the source file next to it as "name (1).ext".
    Rename,
    /// Abort before writing anything if any conflict exists.
    Fail,
}

#[derive(clap::Args, Debug, Clone)]
pub struct ImportArgs {
    /// Source directory on the host / mounted read-only into the container.
    pub source: PathBuf,
    /// Destination folder inside the drive (created if missing).
    #[arg(long, default_value = "/")]
    pub dest: String,
    /// Only analyse and print what would happen; write nothing.
    #[arg(long)]
    pub dry_run: bool,
    /// Import files that look like secrets (.env, private keys, certificates…).
    /// They are flagged `sensitive` and hidden from API keys without `allow_sensitive`.
    #[arg(long)]
    pub include_sensitive: bool,
    #[arg(long, value_enum, default_value = "skip")]
    pub symlinks: SymlinkPolicy,
    #[arg(long, value_enum, default_value = "skip")]
    pub on_conflict: ImportConflict,
    /// Exclude paths (relative to source) or names matching this glob (`*`, `?`). Repeatable.
    #[arg(long = "exclude", value_name = "GLOB")]
    pub excludes: Vec<String>,
    /// Print the report as JSON.
    #[arg(long)]
    pub json: bool,
    /// Max entries listed per category in the text report (0 = all).
    #[arg(long, default_value_t = 100)]
    pub show: usize,
}

#[derive(Serialize, Debug, Clone)]
pub struct Entry {
    pub path: String,
    pub detail: String,
}

#[derive(Serialize, Default, Debug)]
pub struct Report {
    pub dry_run: bool,
    pub source: String,
    pub dest: String,
    pub policies: HashMap<&'static str, String>,
    pub dirs: u64,
    pub files: u64,
    pub total_bytes: u64,
    pub new_files: u64,
    pub new_bytes: u64,
    pub unchanged: u64,
    pub imported: u64,
    pub imported_bytes: u64,
    pub conflicts: Vec<Entry>,
    pub sensitive: Vec<Entry>,
    pub symlinks: Vec<Entry>,
    pub skipped: Vec<Entry>,
    pub excluded: Vec<Entry>,
    pub errors: Vec<Entry>,
}

#[derive(Debug)]
enum Item {
    Dir { rel: String },
    File { rel: String, read_from: PathBuf, size: u64, mtime: i64, sensitive: bool },
}

fn glob_match(pat: &[u8], s: &[u8]) -> bool {
    match (pat.first(), s.first()) {
        (None, None) => true,
        (Some(b'*'), _) => glob_match(&pat[1..], s) || (!s.is_empty() && glob_match(pat, &s[1..])),
        (Some(b'?'), Some(_)) => glob_match(&pat[1..], &s[1..]),
        (Some(a), Some(b)) if a == b => glob_match(&pat[1..], &s[1..]),
        _ => false,
    }
}

fn excluded(rel: &str, name: &str, pats: &[String]) -> Option<String> {
    pats.iter()
        .find(|p| {
            let p = p.trim_matches('/');
            glob_match(p.as_bytes(), rel.as_bytes()) || glob_match(p.as_bytes(), name.as_bytes())
        })
        .cloned()
}

fn open_ro(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NOCTTY).open(path)
}

fn hash_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = open_ro(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

fn sniff_sensitive(path: &Path, name: &str) -> Option<&'static str> {
    if let Some(r) = sensitive::classify_name(name) {
        return Some(r);
    }
    use std::io::Read;
    let mut head = [0u8; 4096];
    let n = open_ro(path).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
    sensitive::classify_content(&head[..n])
}

struct Walker<'a> {
    args: &'a ImportArgs,
    root: PathBuf,
    data_dir: Option<PathBuf>,
    items: Vec<Item>,
    report: Report,
}

impl Walker<'_> {
    fn walk(&mut self, abs: &Path, rel: &str) {
        let mut entries: Vec<_> = match std::fs::read_dir(abs) {
            Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
            Err(e) => {
                self.report.errors.push(Entry { path: rel_or_root(rel), detail: format!("無法讀取資料夾：{e}") });
                return;
            }
        };
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let os_name = e.file_name();
            let path = e.path();
            let Some(name) = os_name.to_str().map(str::to_string) else {
                let shown = os_name.to_string_lossy().into_owned();
                self.report.skipped.push(Entry { path: join(rel, &shown), detail: "檔名不是有效的 UTF-8，無法保存".into() });
                continue;
            };
            let child_rel = join(rel, &name);
            if let Err(err) = nodes::validate_name(&name) {
                self.report.skipped.push(Entry { path: child_rel, detail: format!("不允許的檔名：{}", err.message) });
                continue;
            }
            if let Some(p) = excluded(&child_rel, &name, &self.args.excludes) {
                self.report.excluded.push(Entry { path: child_rel, detail: format!("符合 --exclude {p}") });
                continue;
            }
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(err) => {
                    self.report.errors.push(Entry { path: child_rel, detail: format!("無法讀取：{err}") });
                    continue;
                }
            };
            let ft = meta.file_type();
            if ft.is_symlink() {
                self.symlink(&path, child_rel, &name);
            } else if ft.is_dir() {
                if path.join(DATA_MARKER).exists() || self.data_dir.as_deref() == std::fs::canonicalize(&path).ok().as_deref() {
                    self.report.excluded.push(Entry { path: child_rel, detail: "Mistbox 執行期資料目錄，避免遞迴匯入".into() });
                    continue;
                }
                self.report.dirs += 1;
                self.items.push(Item::Dir { rel: child_rel.clone() });
                self.walk(&path, &child_rel);
            } else if ft.is_file() {
                self.file(&path, child_rel, &name, &meta, false);
            } else {
                self.report.skipped.push(Entry { path: child_rel, detail: "特殊檔案（FIFO/socket/裝置），不保存".into() });
            }
        }
    }

    fn symlink(&mut self, path: &Path, rel: String, name: &str) {
        let target = std::fs::read_link(path).map(|t| t.display().to_string()).unwrap_or_default();
        if self.args.symlinks == SymlinkPolicy::Skip {
            self.report.symlinks.push(Entry { path: rel, detail: format!("→ {target}：略過（--symlinks skip）") });
            return;
        }
        let resolved = match std::fs::canonicalize(path) {
            Ok(r) => r,
            Err(_) => {
                self.report.symlinks.push(Entry { path: rel, detail: format!("→ {target}：目標不存在，略過") });
                return;
            }
        };
        if !resolved.starts_with(&self.root) {
            self.report.symlinks.push(Entry { path: rel, detail: format!("→ {target}：指向來源之外，略過") });
            return;
        }
        match std::fs::metadata(&resolved) {
            Ok(m) if m.is_file() => {
                self.report.symlinks.push(Entry { path: rel.clone(), detail: format!("→ {target}：以檔案複本保存") });
                self.file(&resolved, rel, name, &m, true);
            }
            Ok(m) if m.is_dir() => {
                self.report.symlinks.push(Entry { path: rel, detail: format!("→ {target}：資料夾連結不跟隨，略過") });
            }
            _ => self.report.symlinks.push(Entry { path: rel, detail: format!("→ {target}：非一般檔案，略過") }),
        }
    }

    fn file(&mut self, read_from: &Path, rel: String, name: &str, meta: &std::fs::Metadata, _via_link: bool) {
        let sens = sniff_sensitive(read_from, name);
        if let Some(reason) = sens {
            let action = if self.args.include_sensitive { "納入（標記為敏感）" } else { "略過（需 --include-sensitive）" };
            self.report.sensitive.push(Entry { path: rel.clone(), detail: format!("{reason}：{action}") });
            if !self.args.include_sensitive {
                return;
            }
        }
        self.report.files += 1;
        self.report.total_bytes += meta.len();
        self.items.push(Item::File {
            rel,
            read_from: read_from.to_path_buf(),
            size: meta.len(),
            mtime: meta.mtime(),
            sensitive: sens.is_some(),
        });
    }
}

fn join(rel: &str, name: &str) -> String {
    if rel.is_empty() { name.to_string() } else { format!("{rel}/{name}") }
}
fn rel_or_root(rel: &str) -> String {
    if rel.is_empty() { ".".into() } else { rel.to_string() }
}

enum Plan {
    New,
    Unchanged,
    Conflict(String),
}

/// Look up an existing node at `dest/rel` without creating anything.
async fn existing(db: &SqlitePool, dest: Option<&Node>, rel: &str, cache: &mut HashMap<String, Option<String>>) -> anyhow::Result<Option<Node>> {
    let Some(dest) = dest else { return Ok(None) };
    let (dir_rel, name) = rel.rsplit_once('/').unwrap_or(("", rel));
    let dir_id = if let Some(c) = cache.get(dir_rel) {
        c.clone()
    } else {
        let mut cur = Some(dest.id.clone());
        for seg in dir_rel.split('/').filter(|s| !s.is_empty()) {
            cur = match cur {
                Some(id) => nodes::child_by_name(db, &id, seg).await.map_err(|e| anyhow::anyhow!(e.message))?.filter(|n| n.is_dir()).map(|n| n.id),
                None => None,
            };
        }
        cache.insert(dir_rel.to_string(), cur.clone());
        cur
    };
    match dir_id {
        Some(id) => Ok(nodes::child_by_name(db, &id, name).await.map_err(|e| anyhow::anyhow!(e.message))?),
        None => Ok(None),
    }
}

pub async fn run(db: &SqlitePool, store: &Store, data_dir: &Path, args: &ImportArgs) -> anyhow::Result<Report> {
    let root = std::fs::canonicalize(&args.source).with_context(|| format!("來源不存在：{}", args.source.display()))?;
    if !root.is_dir() {
        bail!("來源不是資料夾：{}", root.display());
    }
    if root.join(DATA_MARKER).exists() {
        bail!("來源本身是 Mistbox 資料目錄，拒絕匯入");
    }
    let data_canon = std::fs::canonicalize(data_dir).ok();
    if let Some(d) = &data_canon
        && (d == &root || root.starts_with(d)) {
            bail!("來源位於 Mistbox 資料目錄內，拒絕匯入");
        }
    let dest_segs: Vec<String> = args.dest.split('/').filter(|s| !s.is_empty()).map(str::to_string).collect();
    for s in &dest_segs {
        nodes::validate_name(s).map_err(|e| anyhow::anyhow!("--dest 無效：{}", e.message))?;
    }

    let mut w = Walker { args, root: root.clone(), data_dir: data_canon, items: Vec::new(), report: Report::default() };
    w.report.dry_run = args.dry_run;
    w.report.source = root.display().to_string();
    w.report.dest = format!("/{}", dest_segs.join("/"));
    w.report.policies = HashMap::from([
        ("symlinks", match args.symlinks {
            SymlinkPolicy::Skip => "skip：不跟隨任何符號連結，逐一列出後略過".to_string(),
            SymlinkPolicy::FollowInside => "follow-inside：僅跟隨指向來源內一般檔案的連結並保存目標內容複本；資料夾連結與指向來源外者略過".to_string(),
        }),
        ("sensitive", if args.include_sensitive {
            "納入並標記為敏感（API Key 預設看不到）".to_string()
        } else {
            "略過並列出（使用 --include-sensitive 納入）".to_string()
        }),
        ("on_conflict", match args.on_conflict {
            ImportConflict::Skip => "skip：同路徑已有不同內容的檔案時保留雲端版本、略過來源檔".to_string(),
            ImportConflict::Rename => "rename：另存為「名稱 (1).副檔名」，不覆寫".to_string(),
            ImportConflict::Fail => "fail：只要有衝突就在寫入前中止".to_string(),
        }),
        ("repeat", "重複匯入時，路徑與 SHA-256 相同的檔案視為未變更並略過".to_string()),
    ]);
    w.walk(&root, "");
    let Walker { items, mut report, .. } = w;

    // Destination folder (only looked up during dry-run).
    let root_node = nodes::get(db, nodes::ROOT_ID).await.map_err(|e| anyhow::anyhow!(e.message))?;
    let mut dest_node = Some(root_node.clone());
    for s in &dest_segs {
        dest_node = match dest_node {
            Some(d) => nodes::child_by_name(db, &d.id, s).await.map_err(|e| anyhow::anyhow!(e.message))?,
            None => None,
        };
        if dest_node.as_ref().is_some_and(|n| !n.is_dir()) {
            bail!("--dest 路徑上的「{s}」是檔案，不是資料夾");
        }
    }

    // Classify each file against what's already in the drive.
    let mut cache = HashMap::new();
    let mut plans = Vec::with_capacity(items.len());
    for item in &items {
        let plan = match item {
            Item::Dir { rel } => match existing(db, dest_node.as_ref(), rel, &mut cache).await? {
                Some(n) if !n.is_dir() => Plan::Conflict("雲端已有同名檔案，無法建立資料夾".into()),
                _ => Plan::New,
            },
            Item::File { rel, read_from, size, .. } => match existing(db, dest_node.as_ref(), rel, &mut cache).await? {
                None => Plan::New,
                Some(n) if n.is_dir() => Plan::Conflict("雲端已有同名資料夾".into()),
                Some(n) => {
                    let same = n.size as u64 == *size
                        && hash_file(read_from).ok().as_deref() == n.sha256.as_deref();
                    if same { Plan::Unchanged } else { Plan::Conflict(format!("內容不同（雲端 {} bytes / 來源 {} bytes）", n.size, size)) }
                }
            },
        };
        match (&plan, item) {
            (Plan::New, Item::File { size, .. }) => {
                report.new_files += 1;
                report.new_bytes += size;
            }
            (Plan::Unchanged, _) => report.unchanged += 1,
            (Plan::Conflict(why), Item::File { rel, .. } | Item::Dir { rel }) => {
                let action = match (args.on_conflict, item) {
                    (_, Item::Dir { .. }) => "略過（含其內容）",
                    (ImportConflict::Skip, _) => "略過，保留雲端版本",
                    (ImportConflict::Rename, _) => "另存新檔名",
                    (ImportConflict::Fail, _) => "中止匯入",
                };
                report.conflicts.push(Entry { path: rel.clone(), detail: format!("{why}：{action}") });
            }
            _ => {}
        }
        plans.push(plan);
    }

    if args.dry_run {
        return Ok(report);
    }
    if args.on_conflict == ImportConflict::Fail && !report.conflicts.is_empty() {
        bail!("發現 {} 個衝突，依 --on-conflict fail 中止；未寫入任何內容", report.conflicts.len());
    }

    let dest = nodes::ensure_dirs(db, &root_node, &dest_segs).await.map_err(|e| anyhow::anyhow!(e.message))?;
    let mut dirs: HashMap<String, Option<Node>> = HashMap::from([(String::new(), Some(dest.clone()))]);
    for (item, plan) in items.iter().zip(plans) {
        match item {
            Item::Dir { rel } => {
                let (parent_rel, name) = rel.rsplit_once('/').unwrap_or(("", rel));
                let parent = dirs.get(parent_rel).cloned().flatten();
                let made = match (parent, &plan) {
                    (Some(p), Plan::New) => nodes::ensure_dirs(db, &p, &[name.to_string()]).await.ok(),
                    _ => None,
                };
                dirs.insert(rel.clone(), made);
            }
            Item::File { rel, read_from, size, mtime, sensitive } => {
                let policy = match plan {
                    Plan::Unchanged => continue,
                    Plan::Conflict(_) if args.on_conflict != ImportConflict::Rename => continue,
                    Plan::Conflict(_) => ConflictPolicy::Rename,
                    Plan::New => ConflictPolicy::Fail,
                };
                let (parent_rel, name) = rel.rsplit_once('/').unwrap_or(("", rel));
                let Some(parent) = dirs.get(parent_rel).cloned().flatten() else {
                    report.errors.push(Entry { path: rel.clone(), detail: "上層資料夾未建立，略過".into() });
                    continue;
                };
                match import_one(db, store, &parent, name, read_from, *size, *mtime, *sensitive, policy).await {
                    Ok(n) => {
                        report.imported += 1;
                        report.imported_bytes += n.size as u64;
                    }
                    Err(e) => report.errors.push(Entry { path: rel.clone(), detail: e.to_string() }),
                }
            }
        }
    }
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
async fn import_one(
    db: &SqlitePool,
    store: &Store,
    parent: &Node,
    name: &str,
    path: &Path,
    expected_size: u64,
    mtime: i64,
    sensitive: bool,
    policy: ConflictPolicy,
) -> anyhow::Result<Node> {
    let f = tokio::fs::File::from_std(open_ro(path).with_context(|| "無法開啟（權限不足？）")?);
    let staged = store.stage(f, u64::MAX).await?;
    if staged.size != expected_size {
        bail!("讀取期間檔案大小改變（{} → {}），略過", expected_size, staged.size);
    }
    let (size, sha) = (staged.size, staged.sha256.clone());
    let blob = store.commit(staged).await?;
    let res = nodes::insert_file(
        db,
        parent,
        nodes::NewFile { name, size, sha256: &sha, blob_id: &blob, mime: &guess_mime(name), sensitive, modified_at: Some(mtime) },
        policy,
    )
    .await;
    match res {
        Ok(n) => Ok(n),
        Err(e) => {
            store.remove_blob(&blob).await;
            bail!("{}", e.message)
        }
    }
}

pub fn print_report(r: &Report, show: usize) {
    let mb = |b: u64| format!("{:.2} MiB", b as f64 / 1048576.0);
    println!("{}", if r.dry_run { "=== 匯入預演（dry-run，未寫入任何內容）===" } else { "=== 匯入結果 ===" });
    println!("來源：{}（唯讀）", r.source);
    println!("目的：{}", r.dest);
    let mut pol: Vec<_> = r.policies.iter().collect();
    pol.sort();
    for (k, v) in pol {
        println!("政策 {k}: {v}");
    }
    println!("資料夾：{}　檔案：{}　總容量：{}", r.dirs, r.files, mb(r.total_bytes));
    println!("新檔案：{}（{}）　未變更：{}　衝突：{}", r.new_files, mb(r.new_bytes), r.unchanged, r.conflicts.len());
    if !r.dry_run {
        println!("已匯入：{} 個檔案（{}）", r.imported, mb(r.imported_bytes));
    }
    for (title, list) in [
        ("衝突", &r.conflicts),
        ("敏感檔", &r.sensitive),
        ("符號連結", &r.symlinks),
        ("略過", &r.skipped),
        ("排除", &r.excluded),
        ("錯誤", &r.errors),
    ] {
        if list.is_empty() {
            continue;
        }
        println!("\n[{title}] {} 項", list.len());
        let n = if show == 0 { list.len() } else { show.min(list.len()) };
        for e in &list[..n] {
            println!("  {}  —  {}", e.path, e.detail);
        }
        if n < list.len() {
            println!("  …另有 {} 項（使用 --show 0 或 --json 顯示全部）", list.len() - n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::glob_match;
    #[test]
    fn globs() {
        assert!(glob_match(b"*.log", b"a.log"));
        assert!(glob_match(b"node_modules", b"node_modules"));
        assert!(glob_match(b"a/*/c", b"a/b/c"));
        assert!(!glob_match(b"*.log", b"a.txt"));
    }
}
