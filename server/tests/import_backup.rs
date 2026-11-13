//! Import (dry-run, sensitive files, symlinks, conflicts, idempotency,
//! read-only source) and backup / restore / verify.

use std::{
    collections::BTreeMap,
    os::unix::fs::{symlink, PermissionsExt},
    path::Path,
};

use mistbox::{
    backup, db,
    import::{self, ImportArgs, ImportConflict, SymlinkPolicy},
    nodes,
    storage::Store,
};
use sha2::{Digest, Sha256};

fn args(src: &Path) -> ImportArgs {
    ImportArgs {
        source: src.to_path_buf(),
        dest: "/".into(),
        dry_run: false,
        include_sensitive: false,
        symlinks: SymlinkPolicy::Skip,
        on_conflict: ImportConflict::Skip,
        excludes: vec![],
        json: false,
        show: 0,
    }
}

/// path -> (sha256, mode, mtime) for every entry, to prove the source is untouched.
fn snapshot(root: &Path) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for e in walkdir::WalkDir::new(root).follow_links(false) {
        let e = e.unwrap();
        let meta = std::fs::symlink_metadata(e.path()).unwrap();
        let content = if meta.is_file() { hex::encode(Sha256::digest(std::fs::read(e.path()).unwrap())) } else { String::new() };
        use std::os::unix::fs::MetadataExt;
        m.insert(e.path().display().to_string(), format!("{content}:{:o}:{}", meta.mode(), meta.mtime()));
    }
    m
}

fn make_source(root: &Path) {
    std::fs::create_dir_all(root.join("專案/深/層")).unwrap();
    std::fs::write(root.join("專案/深/層/筆記 一.md"), "# 標題\n").unwrap();
    std::fs::write(root.join(".hidden"), "h").unwrap();
    std::fs::write(root.join("noext"), "n").unwrap();
    std::fs::write(root.join("bin.dat"), [0u8, 1, 2, 255, 254]).unwrap();
    std::fs::write(root.join(".env"), "TOKEN=abc").unwrap();
    std::fs::write(root.join("deploy.key"), "x").unwrap();
    std::fs::write(root.join("innocent.txt"), "-----BEGIN RSA PRIVATE KEY-----\nabc\n").unwrap();
    std::fs::write(root.join(".env.example"), "TOKEN=").unwrap();
    symlink("noext", root.join("link-inside")).unwrap();
    symlink("/etc/hostname", root.join("link-outside")).unwrap();
    symlink("專案", root.join("link-dir")).unwrap();
    symlink("missing", root.join("link-dangling")).unwrap();
    nix_mkfifo(&root.join("pipe"));
    // A nested Mistbox data dir must be skipped (no recursion).
    std::fs::create_dir_all(root.join("app/data")).unwrap();
    std::fs::write(root.join("app/data").join(db::DATA_MARKER), "").unwrap();
    std::fs::write(root.join("app/data/mistbox.db"), "not really").unwrap();
    std::fs::create_dir_all(root.join("node_modules/x")).unwrap();
    std::fs::write(root.join("node_modules/x/i.js"), "").unwrap();
    let ro = root.join("readonly.txt");
    std::fs::write(&ro, "ro").unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o444)).unwrap();
}

fn nix_mkfifo(p: &Path) {
    let c = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
}

async fn file_count(pool: &sqlx::SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM nodes WHERE kind='file'").fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn import_dry_run_policies_and_idempotency() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    make_source(&src);
    let data = tmp.path().join("data");
    let pool = db::open(&data).await.unwrap();
    let store = Store::new(&data).unwrap();
    let before = snapshot(&src);

    // Dry-run: full report, nothing written.
    let mut a = args(&src);
    a.dry_run = true;
    a.excludes = vec!["node_modules".into()];
    let r = import::run(&pool, &store, &data, &a).await.unwrap();
    assert_eq!(file_count(&pool).await, 0);
    assert_eq!(store.list_blobs().unwrap().len(), 0);
    let paths = |v: &Vec<import::Entry>| v.iter().map(|e| e.path.clone()).collect::<Vec<_>>();
    let sens = paths(&r.sensitive);
    assert!(sens.contains(&".env".into()) && sens.contains(&"deploy.key".into()) && sens.contains(&"innocent.txt".into()));
    assert!(!sens.contains(&".env.example".into()));
    assert_eq!(r.symlinks.len(), 4, "every symlink listed: {:?}", r.symlinks);
    assert!(paths(&r.skipped).contains(&"pipe".into()));
    let ex = paths(&r.excluded);
    assert!(ex.contains(&"app/data".into()) && ex.contains(&"node_modules".into()));
    // .hidden, noext, bin.dat, .env.example, readonly.txt, 筆記 一.md
    assert_eq!(r.files, 6, "{r:?}");
    assert_eq!(r.new_files, 6);

    // Real import with follow-inside symlinks and sensitive files included.
    let mut a = args(&src);
    a.include_sensitive = true;
    a.symlinks = SymlinkPolicy::FollowInside;
    a.excludes = vec!["node_modules".into()];
    let r = import::run(&pool, &store, &data, &a).await.unwrap();
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(r.imported, 6 + 3 + 1, "6 normal + 3 sensitive + 1 in-source symlink");
    let sym_detail: Vec<_> = r.symlinks.iter().map(|e| format!("{} {}", e.path, e.detail)).collect();
    assert!(sym_detail.iter().any(|s| s.starts_with("link-inside") && s.contains("複本")));
    assert!(sym_detail.iter().any(|s| s.starts_with("link-outside") && s.contains("來源之外")));
    assert!(sym_detail.iter().any(|s| s.starts_with("link-dir") && s.contains("資料夾連結")));

    let root = nodes::get(&pool, "root").await.unwrap();
    let md = nodes::resolve_rel(&pool, "root", "專案/深/層/筆記 一.md", "root").await.unwrap();
    assert_eq!(md.sha256.unwrap(), hex::encode(Sha256::digest("# 標題\n".as_bytes())));
    let env = nodes::child_by_name(&pool, &root.id, ".env").await.unwrap().unwrap();
    assert!(env.sensitive, "sensitive files flagged");
    let bin = nodes::child_by_name(&pool, &root.id, "bin.dat").await.unwrap().unwrap();
    assert_eq!(std::fs::read(store.blob_path(bin.blob_id.as_deref().unwrap())).unwrap(), vec![0u8, 1, 2, 255, 254]);
    assert!(nodes::child_by_name(&pool, &root.id, "app").await.unwrap().unwrap().is_dir());
    assert!(nodes::resolve_rel(&pool, "root", "app/data", "root").await.is_err(), "data dir never imported");

    // Source untouched.
    assert_eq!(before, snapshot(&src));

    // Re-import: everything unchanged.
    let r = import::run(&pool, &store, &data, &a).await.unwrap();
    assert_eq!(r.imported, 0);
    assert_eq!(r.unchanged, 10);
    assert!(r.conflicts.is_empty());

    // Changed source file → conflict, skip keeps cloud version.
    std::fs::write(src.join("noext"), "changed").unwrap();
    let r = import::run(&pool, &store, &data, &a).await.unwrap();
    assert_eq!(r.conflicts.len(), 2, "noext + link-inside: {:?}", r.conflicts);
    assert_eq!(r.imported, 0);
    let n = nodes::child_by_name(&pool, "root", "noext").await.unwrap().unwrap();
    assert_eq!(std::fs::read(store.blob_path(n.blob_id.as_deref().unwrap())).unwrap(), b"n");
    // fail policy aborts before writing.
    a.on_conflict = ImportConflict::Fail;
    let count = file_count(&pool).await;
    assert!(import::run(&pool, &store, &data, &a).await.is_err());
    assert_eq!(file_count(&pool).await, count);
    // rename keeps both.
    a.on_conflict = ImportConflict::Rename;
    let r = import::run(&pool, &store, &data, &a).await.unwrap();
    assert_eq!(r.imported, 2);
    assert!(nodes::child_by_name(&pool, "root", "noext (1)").await.unwrap().is_some());

    // Importing the data dir itself is refused.
    assert!(import::run(&pool, &store, &data, &args(&data)).await.is_err());
}

#[tokio::test]
async fn backup_restore_verify() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(src.join("a")).unwrap();
    std::fs::write(src.join("a/一.txt"), "one").unwrap();
    let mut big = vec![0u8; 2_000_000];
    getrandom::fill(&mut big).unwrap();
    std::fs::write(src.join("big.bin"), &big).unwrap();
    let data = tmp.path().join("data");
    let pool = db::open(&data).await.unwrap();
    let store = Store::new(&data).unwrap();
    import::run(&pool, &store, &data, &args(&src)).await.unwrap();

    let tarfile = tmp.path().join("backup.tar");
    backup::backup(&pool, &store, tarfile.to_str().unwrap()).await.unwrap();
    let lock: i64 = sqlx::query_scalar("SELECT backup_until FROM maintenance").fetch_one(&pool).await.unwrap();
    assert_eq!(lock, 0, "backup lock released");
    pool.close().await;

    // Restore refuses to clobber without --force.
    let data2 = tmp.path().join("data2");
    backup::restore(&data2, tarfile.to_str().unwrap(), false).unwrap();
    assert!(backup::restore(&data2, tarfile.to_str().unwrap(), false).is_err());
    let pool2 = db::open(&data2).await.unwrap();
    let store2 = Store::new(&data2).unwrap();
    let v = backup::verify(&pool2, &store2).await.unwrap();
    assert_eq!(v.checked, 2);
    assert!(v.missing.is_empty() && v.corrupt.is_empty() && v.orphans.is_empty());
    let n = nodes::resolve_rel(&pool2, "root", "big.bin", "root").await.unwrap();
    assert_eq!(std::fs::read(store2.blob_path(n.blob_id.as_deref().unwrap())).unwrap(), big);

    // Corruption is detected.
    std::fs::write(store2.blob_path(n.blob_id.as_deref().unwrap()), b"tampered").unwrap();
    let v = backup::verify(&pool2, &store2).await.unwrap();
    assert_eq!(v.corrupt, vec!["/big.bin".to_string()]);
}

#[tokio::test]
async fn gc_paused_during_backup() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let pool = db::open(&data).await.unwrap();
    let store = Store::new(&data).unwrap();
    let staged = store.stage(&b"x"[..], 10).await.unwrap();
    let blob = store.commit(staged).await.unwrap();
    sqlx::query("INSERT INTO blob_gc (blob_id, queued_at) VALUES (?, 0)").bind(&blob).execute(&pool).await.unwrap();
    sqlx::query("UPDATE maintenance SET backup_until = ?").bind(mistbox::util::now() + 100).execute(&pool).await.unwrap();
    assert_eq!(nodes::run_blob_gc(&pool, &store).await.unwrap(), 0);
    assert!(store.blob_path(&blob).exists());
    sqlx::query("UPDATE maintenance SET backup_until = 0").execute(&pool).await.unwrap();
    assert_eq!(nodes::run_blob_gc(&pool, &store).await.unwrap(), 1);
    assert!(!store.blob_path(&blob).exists());
}
