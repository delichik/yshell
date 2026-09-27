//! Behaviour tests for recursive tree transfers and batch operations, driven
//! through the public API and the fake backend.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use tempfile::TempDir;
use yshell_sftp::{
    FakeSftpBackend, FsEntry, FsEntryKind, OverwritePolicy, RealSftpBackend, SftpBackend,
    SftpClient, SftpErrorKind, TransferDirection, TransferItemKind, TransferProgress,
    TreeTransferOptions,
};

fn fake_client() -> SftpClient {
    SftpClient::with_backend(FakeSftpBackend::default())
}

/// Build a fake-backed client after seeding the raw backend (needed for
/// `insert`, which is a fake-only helper).
fn fake_client_with(setup: impl FnOnce(&mut FakeSftpBackend)) -> SftpClient {
    let mut backend = FakeSftpBackend::default();
    setup(&mut backend);
    SftpClient::with_backend(backend)
}

fn options(overwrite: OverwritePolicy, follow_symlinks: bool) -> TreeTransferOptions {
    TreeTransferOptions {
        overwrite,
        follow_symlinks,
    }
}

fn write_file(root: &Path, relative: &str, contents: &str) -> PathBuf {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create local parent");
    }
    fs::write(&path, contents).expect("write local file");
    path
}

fn upload_text(client: &mut SftpClient, remote: &str, contents: &str) {
    let temp = TempDir::new().expect("tempdir");
    let local = temp.path().join("upload.txt");
    fs::write(&local, contents).expect("write upload source");
    client
        .upload_file(&local, remote)
        .expect("upload fake file");
}

fn download_text(client: &SftpClient, remote: &str) -> String {
    let temp = TempDir::new().expect("tempdir");
    let local = temp.path().join("download.txt");
    client.download_file(remote, &local).expect("download fake file");
    fs::read_to_string(&local).expect("read downloaded text")
}

/// Remote seeding shared by the upload overwrite-policy tests: `/dest` already
/// contains `a.txt` (file), `sub/keep.txt` (dir) and `blocked` (file).
fn seeded_upload_client() -> SftpClient {
    let mut client = fake_client();
    client.mkdir("/dest").expect("mkdir /dest");
    upload_text(&mut client, "/dest/a.txt", "old-a");
    client.mkdir("/dest/sub").expect("mkdir /dest/sub");
    upload_text(&mut client, "/dest/sub/keep.txt", "keep");
    upload_text(&mut client, "/dest/blocked", "old-file");
    client
}

/// Local tree shared by the upload tests: `a.txt`, `sub/b.txt`, `blocked/y.txt`
/// (a directory where the remote already has a file).
fn seeded_upload_local(temp: &TempDir) -> PathBuf {
    let local = temp.path().join("src");
    write_file(&local, "a.txt", "new-a");
    write_file(&local, "sub/b.txt", "new-b");
    write_file(&local, "blocked/y.txt", "new-y");
    local
}

#[test]
fn upload_tree_transfers_nested_tree_and_reports_progress() {
    let temp = TempDir::new().expect("tempdir");
    let local = temp.path().join("src");
    write_file(&local, "a.txt", "alpha"); // 5 bytes
    write_file(&local, "sub/b.txt", "beta!"); // 5 bytes
    write_file(&local, "sub/deep/c.txt", "c"); // 1 byte

    let mut client = fake_client();
    let cancel = AtomicBool::new(false);
    let mut events: Vec<TransferProgress> = Vec::new();
    let mut progress = |event: TransferProgress| events.push(event);
    let report = client
        .upload_tree(
            &local,
            "/srv/data",
            options(OverwritePolicy::Overwrite, false),
            &mut progress,
            &cancel,
        )
        .expect("upload tree");
    assert!(!report.cancelled);
    assert!(report.failed.is_empty());
    assert_eq!(report.skipped, 0);
    assert!(report.conflicts.is_empty());
    assert_eq!(report.bytes_transferred, 11);
    assert_eq!(report.completed, 6); // root + a.txt + sub + b.txt + deep + c.txt
    assert!(report.is_success());

    // Missing intermediate remote directories are created (`mkdir -p` root).
    assert!(client
        .list_dir("/srv")
        .expect("list /srv")
        .entries
        .iter()
        .any(|entry| entry.name == "data"));
    assert_eq!(download_text(&client, "/srv/data/a.txt"), "alpha");
    assert_eq!(download_text(&client, "/srv/data/sub/b.txt"), "beta!");
    assert_eq!(download_text(&client, "/srv/data/sub/deep/c.txt"), "c");

    assert!(events
        .iter()
        .all(|event| event.direction == TransferDirection::Upload));
    assert!(events.iter().any(|event| event.kind == TransferItemKind::Dir));
    let last = events.last().expect("progress events");
    assert_eq!(last.path, "/srv/data/sub/deep/c.txt");
    assert_eq!(last.bytes_done, 11);
    assert_eq!(last.bytes_total, 11);
    assert!(events.windows(2).all(|pair| pair[0].bytes_done <= pair[1].bytes_done));
}

#[test]
fn upload_tree_overwrite_skip_ask_and_rename_policies() {
    // Overwrite: replace the file, merge `sub`, replace the file blocking the
    // `blocked` directory.
    let temp = TempDir::new().expect("tempdir");
    let local = seeded_upload_local(&temp);
    let mut client = seeded_upload_client();
    let cancel = AtomicBool::new(false);
    let mut progress = |_event: TransferProgress| {};
    let report = client
        .upload_tree(
            &local,
            "/dest",
            options(OverwritePolicy::Overwrite, false),
            &mut progress,
            &cancel,
        )
        .expect("upload tree");
    assert_eq!(report.completed, 6); // /dest + a.txt + blocked + y.txt + sub + b.txt
    assert_eq!(report.skipped, 0);
    assert_eq!(report.bytes_transferred, 15);
    assert!(report.failed.is_empty());
    assert_eq!(download_text(&client, "/dest/a.txt"), "new-a");
    assert_eq!(download_text(&client, "/dest/sub/keep.txt"), "keep");
    assert_eq!(download_text(&client, "/dest/sub/b.txt"), "new-b");
    assert_eq!(download_text(&client, "/dest/blocked/y.txt"), "new-y");

    // Skip: existing files are untouched, the conflicting directory subtree is
    // skipped instead of failing.
    let mut client = seeded_upload_client();
    let report = client
        .upload_tree(
            &local,
            "/dest",
            options(OverwritePolicy::Skip, false),
            &mut progress,
            &cancel,
        )
        .expect("upload tree");
    assert_eq!(report.completed, 3); // /dest + sub + b.txt
    assert_eq!(report.skipped, 3); // a.txt + blocked + blocked/y.txt
    assert!(report.conflicts.is_empty());
    assert_eq!(report.bytes_transferred, 5);
    // Every scanned item is accounted for: root + a.txt + blocked + y.txt + sub + b.txt.
    assert_eq!(report.completed + report.skipped + report.failed.len() as u64, 6);
    assert_eq!(download_text(&client, "/dest/a.txt"), "old-a");
    assert_eq!(download_text(&client, "/dest/blocked"), "old-file");
    assert_eq!(download_text(&client, "/dest/sub/b.txt"), "new-b");

    // Ask: the same items are skipped, but conflicts are reported so a caller
    // can prompt and re-run with a concrete policy.
    let mut client = seeded_upload_client();
    let report = client
        .upload_tree(
            &local,
            "/dest",
            options(OverwritePolicy::Ask, false),
            &mut progress,
            &cancel,
        )
        .expect("upload tree");
    assert_eq!(report.skipped, 3);
    assert_eq!(
        report.conflicts,
        vec!["/dest/a.txt".to_owned(), "/dest/blocked".to_owned()]
    );
    assert_eq!(download_text(&client, "/dest/a.txt"), "old-a");
    assert_eq!(download_text(&client, "/dest/blocked"), "old-file");

    // Rename: keep both sides using numbered siblings.
    let mut client = seeded_upload_client();
    let report = client
        .upload_tree(
            &local,
            "/dest",
            options(OverwritePolicy::Rename, false),
            &mut progress,
            &cancel,
        )
        .expect("upload tree");
    assert_eq!(report.completed, 6);
    assert_eq!(report.skipped, 0);
    assert!(report.conflicts.is_empty());
    assert_eq!(download_text(&client, "/dest/a.txt"), "old-a");
    assert_eq!(download_text(&client, "/dest/a (1).txt"), "new-a");
    assert_eq!(download_text(&client, "/dest/blocked"), "old-file");
    assert_eq!(download_text(&client, "/dest/blocked (1)/y.txt"), "new-y");
    assert_eq!(download_text(&client, "/dest/sub/b.txt"), "new-b");
}

#[test]
fn upload_tree_cancel_stops_remaining_items() {
    let temp = TempDir::new().expect("tempdir");
    let local = temp.path().join("src");
    write_file(&local, "a.txt", "aaaaa");
    write_file(&local, "b.txt", "bbbbb");
    write_file(&local, "c.txt", "ccccc");

    let mut client = fake_client();
    let cancel = AtomicBool::new(false);
    let mut events: Vec<TransferProgress> = Vec::new();
    let mut progress = |event: TransferProgress| {
        if event.kind == TransferItemKind::File && event.bytes_done > 0 {
            cancel.store(true, Ordering::Relaxed);
        }
        events.push(event);
    };
    let report = client
        .upload_tree(
            &local,
            "/dest",
            options(OverwritePolicy::Overwrite, false),
            &mut progress,
            &cancel,
        )
        .expect("upload tree");
    assert!(report.cancelled);
    assert!(!report.is_success());
    assert_eq!(report.completed, 2); // /dest + a.txt
    assert_eq!(report.bytes_transferred, 5);
    assert_eq!(download_text(&client, "/dest/a.txt"), "aaaaa");
    assert!(client
        .list_dir("/dest")
        .expect("list /dest")
        .entries
        .iter()
        .all(|entry| entry.name == "a.txt"));
    let last = events.last().expect("progress events");
    assert_eq!(last.bytes_done, 5);
    assert_eq!(last.bytes_total, 15);
}

#[test]
fn upload_tree_cancelled_before_start_transfers_nothing() {
    let temp = TempDir::new().expect("tempdir");
    let local = temp.path().join("src");
    write_file(&local, "a.txt", "aaaaa");

    let mut client = fake_client();
    let cancel = AtomicBool::new(true);
    let mut progress = |_event: TransferProgress| {};
    let report = client
        .upload_tree(
            &local,
            "/dest",
            options(OverwritePolicy::Overwrite, false),
            &mut progress,
            &cancel,
        )
        .expect("upload tree");

    assert!(report.cancelled);
    assert_eq!(report.completed, 0);
    assert_eq!(report.bytes_transferred, 0);
    assert!(client
        .list_dir("/")
        .expect("list /")
        .entries
        .iter()
        .all(|entry| entry.name != "dest"));
}

#[cfg(unix)]
#[test]
fn upload_tree_records_broken_symlink_and_continues() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().expect("tempdir");
    let local = temp.path().join("src");
    write_file(&local, "a.txt", "aaaaa");
    symlink("missing-target", local.join("m_broken.txt")).expect("create dangling symlink");
    write_file(&local, "z.txt", "zzzzz");

    let mut client = fake_client();
    let cancel = AtomicBool::new(false);
    let mut progress = |_event: TransferProgress| {};
    let report = client
        .upload_tree(
            &local,
            "/dest",
            options(OverwritePolicy::Overwrite, true),
            &mut progress,
            &cancel,
        )
        .expect("upload tree");

    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].0, "/dest/m_broken.txt");
    assert!(report.failed[0].1.contains("broken symlink"));
    assert_eq!(report.completed, 3); // /dest + a.txt + z.txt
    assert_eq!(report.skipped, 0);
    assert_eq!(download_text(&client, "/dest/a.txt"), "aaaaa");
    assert_eq!(download_text(&client, "/dest/z.txt"), "zzzzz");

    // Without following symlinks the same tree succeeds and reports a skip.
    let mut client = fake_client();
    let report = client
        .upload_tree(
            &local,
            "/dest",
            options(OverwritePolicy::Overwrite, false),
            &mut progress,
            &cancel,
        )
        .expect("upload tree");
    assert!(report.failed.is_empty());
    assert_eq!(report.skipped, 1);
    assert_eq!(report.completed, 3);
    assert_eq!(report.skipped_items.len(), 1);
    assert_eq!(report.skipped_items[0].0, "/dest/m_broken.txt");
    assert!(report.skipped_items[0].1.contains("follow_symlinks"));
}

#[cfg(unix)]
#[test]
fn upload_tree_follows_symlinks_and_detects_cycles() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().expect("tempdir");
    let local = temp.path().join("src");
    write_file(&local, "real/file.txt", "content");
    symlink("real/file.txt", local.join("link.txt")).expect("symlink file");
    symlink("real", local.join("linkdir")).expect("symlink dir");
    symlink(".", local.join("loop")).expect("symlink cycle");

    let mut client = fake_client();
    let cancel = AtomicBool::new(false);
    let mut progress = |_event: TransferProgress| {};
    let report = client
        .upload_tree(
            &local,
            "/dest",
            options(OverwritePolicy::Overwrite, true),
            &mut progress,
            &cancel,
        )
        .expect("upload tree");

    assert!(report.failed.is_empty());
    assert_eq!(report.skipped, 1); // `loop` re-enters the upload root
    assert_eq!(report.skipped_items.len(), 1);
    assert_eq!(report.skipped_items[0].0, "/dest/loop");
    assert!(report.skipped_items[0].1.contains("cycle"));
    assert_eq!(report.completed, 6); // root + link.txt + linkdir + file + real + file
    assert_eq!(report.bytes_transferred, 21);
    assert_eq!(download_text(&client, "/dest/link.txt"), "content");
    assert_eq!(download_text(&client, "/dest/linkdir/file.txt"), "content");
    assert_eq!(download_text(&client, "/dest/real/file.txt"), "content");
}

/// Remote seeding shared by the download tests: `/src/a.txt` plus
/// `/src/blocked/y.txt`.
fn seeded_download_client() -> SftpClient {
    let mut client = fake_client();
    client.mkdir("/src").expect("mkdir /src");
    upload_text(&mut client, "/src/a.txt", "A-new");
    client.mkdir("/src/blocked").expect("mkdir /src/blocked");
    upload_text(&mut client, "/src/blocked/y.txt", "Y");
    client
}

/// Local output with a conflicting file at `a.txt` and a file where the remote
/// wants a `blocked/` directory.
fn seeded_download_output(temp: &TempDir) -> PathBuf {
    let out = temp.path().join("out");
    fs::create_dir_all(&out).expect("create output dir");
    fs::write(out.join("a.txt"), "A-old").expect("write conflicting file");
    fs::write(out.join("blocked"), "local-file").expect("write blocking file");
    out
}

#[test]
fn download_tree_transfers_nested_tree_and_reports_progress() {
    let temp = TempDir::new().expect("tempdir");
    let out = temp.path().join("out");
    let mut client = fake_client();
    client.mkdir("/src").expect("mkdir /src");
    upload_text(&mut client, "/src/a.txt", "A");
    client.mkdir("/src/sub").expect("mkdir /src/sub");
    upload_text(&mut client, "/src/sub/b.txt", "BB");

    let cancel = AtomicBool::new(false);
    let mut events: Vec<TransferProgress> = Vec::new();
    let mut progress = |event: TransferProgress| events.push(event);
    let report = client
        .download_tree(
            "/src",
            &out,
            options(OverwritePolicy::Overwrite, false),
            &mut progress,
            &cancel,
        )
        .expect("download tree");
    assert!(report.is_success());
    assert_eq!(report.completed, 4); // /src + a.txt + sub + b.txt
    assert_eq!(report.bytes_transferred, 3);
    assert_eq!(fs::read_to_string(out.join("a.txt")).expect("read a"), "A");
    assert_eq!(
        fs::read_to_string(out.join("sub/b.txt")).expect("read b"),
        "BB"
    );
    assert!(events
        .iter()
        .all(|event| event.direction == TransferDirection::Download));
    let last = events.last().expect("progress events");
    assert_eq!(last.bytes_done, 3);
    assert_eq!(last.bytes_total, 3);
}

#[test]
fn download_tree_overwrite_skip_ask_and_rename_policies() {
    let cancel = AtomicBool::new(false);
    let mut progress = |_event: TransferProgress| {};

    // Overwrite: the blocking file is replaced by the remote directory.
    let temp = TempDir::new().expect("tempdir");
    let out = seeded_download_output(&temp);
    let mut client = seeded_download_client();
    let report = client
        .download_tree(
            "/src",
            &out,
            options(OverwritePolicy::Overwrite, false),
            &mut progress,
            &cancel,
        )
        .expect("download tree");
    assert_eq!(report.completed, 4);
    assert_eq!(report.skipped, 0);
    assert!(report.failed.is_empty());
    assert_eq!(fs::read_to_string(out.join("a.txt")).expect("read a"), "A-new");
    assert_eq!(
        fs::read_to_string(out.join("blocked/y.txt")).expect("read y"),
        "Y"
    );

    // Skip: local files win; the whole conflicting subtree is skipped.
    let temp = TempDir::new().expect("tempdir");
    let out = seeded_download_output(&temp);
    let mut client = seeded_download_client();
    let report = client
        .download_tree(
            "/src",
            &out,
            options(OverwritePolicy::Skip, false),
            &mut progress,
            &cancel,
        )
        .expect("download tree");
    assert_eq!(report.completed, 1); // only the existing output root
    assert_eq!(report.skipped, 3); // a.txt + blocked + blocked/y.txt
    assert!(report.conflicts.is_empty());
    assert_eq!(report.bytes_transferred, 0);
    assert_eq!(fs::read_to_string(out.join("a.txt")).expect("read a"), "A-old");
    assert_eq!(
        fs::read_to_string(out.join("blocked")).expect("read blocked"),
        "local-file"
    );

    // Ask: same skips, conflicts listed for the caller.
    let temp = TempDir::new().expect("tempdir");
    let out = seeded_download_output(&temp);
    let mut client = seeded_download_client();
    let report = client
        .download_tree(
            "/src",
            &out,
            options(OverwritePolicy::Ask, false),
            &mut progress,
            &cancel,
        )
        .expect("download tree");
    assert_eq!(report.skipped, 3);
    assert_eq!(
        report.conflicts,
        vec!["/src/a.txt".to_owned(), "/src/blocked".to_owned()]
    );
    assert_eq!(report.completed + report.skipped + report.failed.len() as u64, 4);

    // Rename: keep both sides.
    let temp = TempDir::new().expect("tempdir");
    let out = seeded_download_output(&temp);
    let mut client = seeded_download_client();
    let report = client
        .download_tree(
            "/src",
            &out,
            options(OverwritePolicy::Rename, false),
            &mut progress,
            &cancel,
        )
        .expect("download tree");
    assert_eq!(report.completed, 4);
    assert_eq!(report.skipped, 0);
    assert_eq!(fs::read_to_string(out.join("a.txt")).expect("read a"), "A-old");
    assert_eq!(
        fs::read_to_string(out.join("a (1).txt")).expect("read renamed a"),
        "A-new"
    );
    assert_eq!(
        fs::read_to_string(out.join("blocked")).expect("read blocked"),
        "local-file"
    );
    assert_eq!(
        fs::read_to_string(out.join("blocked (1)/y.txt")).expect("read renamed y"),
        "Y"
    );
}

#[test]
fn download_tree_records_missing_remote_content_and_continues() {
    let temp = TempDir::new().expect("tempdir");
    let out = temp.path().join("out");
    let mut client = fake_client_with(|backend| {
        backend.mkdir("/src").expect("mkdir /src");
        // An entry without stored fake content fails on download.
        backend.insert(FsEntry::file("/src/bad.txt", 5));
    });
    upload_text(&mut client, "/src/ok.txt", "ok");

    let cancel = AtomicBool::new(false);
    let mut progress = |_event: TransferProgress| {};
    let report = client
        .download_tree(
            "/src",
            &out,
            options(OverwritePolicy::Overwrite, false),
            &mut progress,
            &cancel,
        )
        .expect("download tree");

    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].0, "/src/bad.txt");
    assert!(report.failed[0].1.contains("no stored fake content"));
    assert_eq!(report.completed, 2); // output root + ok.txt
    assert_eq!(fs::read_to_string(out.join("ok.txt")).expect("read ok"), "ok");
    assert!(!out.join("bad.txt").exists());
}

#[test]
fn download_tree_cancel_stops_remaining_items() {
    let temp = TempDir::new().expect("tempdir");
    let out = temp.path().join("out");
    let mut client = fake_client();
    client.mkdir("/src").expect("mkdir /src");
    upload_text(&mut client, "/src/a.txt", "a");
    upload_text(&mut client, "/src/b.txt", "b");
    upload_text(&mut client, "/src/c.txt", "c");

    let cancel = AtomicBool::new(false);
    let mut progress = |event: TransferProgress| {
        if event.kind == TransferItemKind::File && event.bytes_done > 0 {
            cancel.store(true, Ordering::Relaxed);
        }
    };
    let report = client
        .download_tree(
            "/src",
            &out,
            options(OverwritePolicy::Overwrite, false),
            &mut progress,
            &cancel,
        )
        .expect("download tree");
    assert!(report.cancelled);
    assert_eq!(report.bytes_transferred, 1);
    assert!(out.join("a.txt").exists());
    assert!(!out.join("b.txt").exists());
    assert!(!out.join("c.txt").exists());
}

#[test]
fn download_tree_follow_symlinks_policy() {
    let temp = TempDir::new().expect("tempdir");
    let upload_source = temp.path().join("upload.txt");
    fs::write(&upload_source, "linked").expect("write upload source");
    let mut client = fake_client_with(|backend| {
        backend.mkdir("/src").expect("mkdir /src");
        backend
            .upload_file(&upload_source, "/src/real.txt")
            .expect("upload real");
        backend
            .upload_file(&upload_source, "/src/link.txt")
            .expect("upload link target");
        // Re-insert the entry as a symlink; the fake keeps its stored content so
        // the follow-up download can resolve it.
        backend.insert(FsEntry {
            kind: FsEntryKind::Symlink,
            ..FsEntry::file("/src/link.txt", 6)
        });
    });

    let cancel = AtomicBool::new(false);
    let mut progress = |_event: TransferProgress| {};

    // follow_symlinks = false: the symlink entry is skipped.
    let out = temp.path().join("out-skip");
    let report = client
        .download_tree(
            "/src",
            &out,
            options(OverwritePolicy::Overwrite, false),
            &mut progress,
            &cancel,
        )
        .expect("download tree");
    assert_eq!(report.skipped, 1);
    assert_eq!(report.skipped_items[0].0, "/src/link.txt");
    assert!(!out.join("link.txt").exists());
    assert_eq!(
        fs::read_to_string(out.join("real.txt")).expect("read real"),
        "linked"
    );

    // follow_symlinks = true: the symlink behaves like the file it points to.
    let out = temp.path().join("out-follow");
    let report = client
        .download_tree(
            "/src",
            &out,
            options(OverwritePolicy::Overwrite, true),
            &mut progress,
            &cancel,
        )
        .expect("download tree");
    assert_eq!(report.skipped, 0);
    assert_eq!(
        fs::read_to_string(out.join("link.txt")).expect("read link"),
        "linked"
    );
}

#[test]
fn remove_dir_only_removes_empty_directories() {
    let mut client = fake_client();
    client.mkdir("/srv").expect("mkdir /srv");
    client.mkdir("/srv/sub").expect("mkdir /srv/sub");
    upload_text(&mut client, "/srv/sub/a.txt", "a");

    let non_empty = client.remove_dir("/srv/sub").expect_err("non-empty");
    assert_eq!(non_empty.kind, SftpErrorKind::Backend);

    let not_a_dir = client
        .remove_dir("/srv/sub/a.txt")
        .expect_err("not a directory");
    assert_eq!(not_a_dir.kind, SftpErrorKind::InvalidPath);

    let missing = client.remove_dir("/srv/missing").expect_err("missing");
    assert_eq!(missing.kind, SftpErrorKind::NotFound);

    client.delete("/srv/sub/a.txt").expect("delete file");
    client.remove_dir("/srv/sub").expect("remove empty dir");
    assert!(client.list_dir("/srv").expect("list /srv").entries.is_empty());
}

#[test]
fn delete_many_and_chmod_many_report_each_item() {
    let mut client = fake_client();
    client.mkdir("/srv").expect("mkdir /srv");
    upload_text(&mut client, "/srv/a.txt", "a");
    upload_text(&mut client, "/srv/b.txt", "b");

    let results = client.delete_many(&[
        "/srv/a.txt".to_owned(),
        "/srv/missing".to_owned(),
        "/srv/b.txt".to_owned(),
    ]);
    assert_eq!(results.len(), 3);
    assert!(results[0].is_ok());
    assert_eq!(
        results[1].result.as_ref().expect_err("missing").kind,
        SftpErrorKind::NotFound
    );
    assert!(results[2].is_ok());
    assert!(client.list_dir("/srv").expect("list /srv").entries.is_empty());

    upload_text(&mut client, "/srv/a.txt", "a");
    let results = client.chmod_many(&["/srv/a.txt".to_owned(), "/srv/missing".to_owned()], 0o600);
    assert_eq!(results.len(), 2);
    assert!(results[0].is_ok());
    assert!(results[1].result.is_err());
    let entry = client
        .list_dir("/srv")
        .expect("list /srv")
        .entries
        .into_iter()
        .find(|entry| entry.name == "a.txt")
        .expect("a.txt entry");
    assert_eq!(entry.permissions, 0o600);
}

#[test]
fn tree_transfer_rejects_renaming_the_transfer_root() {
    let cancel = AtomicBool::new(false);
    let mut progress = |_event: TransferProgress| {};

    // Upload root conflicts with an existing file.
    let temp = TempDir::new().expect("tempdir");
    let local = temp.path().join("src");
    write_file(&local, "a.txt", "a");
    let mut client = fake_client();
    upload_text(&mut client, "/dest", "file");
    let error = client
        .upload_tree(
            &local,
            "/dest",
            options(OverwritePolicy::Rename, false),
            &mut progress,
            &cancel,
        )
        .expect_err("upload root rename");
    assert_eq!(error.kind, SftpErrorKind::InvalidPath);
    assert!(error.message.contains("transfer root"));
    assert_eq!(download_text(&client, "/dest"), "file");

    // Download root conflicts with an existing file.
    let temp = TempDir::new().expect("tempdir");
    let out = temp.path().join("out");
    fs::write(&out, "file").expect("write blocking root");
    let mut client = fake_client();
    client.mkdir("/src").expect("mkdir /src");
    upload_text(&mut client, "/src/a.txt", "a");
    let error = client
        .download_tree(
            "/src",
            &out,
            options(OverwritePolicy::Rename, false),
            &mut progress,
            &cancel,
        )
        .expect_err("download root rename");
    assert_eq!(error.kind, SftpErrorKind::InvalidPath);
    assert!(error.message.contains("transfer root"));
    assert_eq!(fs::read_to_string(&out).expect("read blocking root"), "file");
}

#[test]
fn fake_and_real_backends_expose_the_same_trait_surface() {
    fn assert_backend<B: SftpBackend>() {}
    assert_backend::<FakeSftpBackend>();
    assert_backend::<RealSftpBackend>();
}
