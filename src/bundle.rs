//! Deterministic, required session bundling.
//!
//! The archive contains the final mutable workspace, explicit artifacts, the
//! original prompt control record, and every output sidecar. Symbolic links
//! below the mutable workspace/artifact roots are recorded and archived as
//! links without dereferencing their targets. Missing entries, unreadable
//! files, special files, tar failures, or rename failures are hard session
//! errors. There is no `--ignore-failed-read` path.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
};

use tokio::process::Command;

use crate::error::{io_msg, ServiceError, ServiceResult};

#[derive(Debug, Clone)]
pub struct BundleStats {
    /// SHA-256 of the exact published archive bytes, computed from the
    /// synced partial file that the no-clobber publication links into place.
    pub sha256: String,
    pub compressed_bytes: u64,
    pub uncompressed_bytes: u64,
    pub file_count: u64,
    pub artifacts_file_count: u64,
}

pub async fn create_bundle(session_dir: &Path, archive_path: &Path) -> ServiceResult<BundleStats> {
    for required in ["staged", "artifacts", "control", "output"] {
        let path = session_dir.join(required);
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            ServiceError::Internal(io_msg("bundle required entry", &path, &error))
        })?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(ServiceError::Internal(format!(
                "bundle required entry {} is not a real directory",
                path.display()
            )));
        }
    }
    for required in [
        "control/prompt.txt",
        "control/turn-budget.json",
        "output/ready.json",
        "output/events.jsonl",
        "output/qwen.stderr",
        "output/qwen-exit-code",
        "output/response.txt",
    ] {
        let path = session_dir.join(required);
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
            ServiceError::Internal(io_msg("bundle required file", &path, &error))
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(ServiceError::Internal(format!(
                "bundle required file {} is not a regular non-symlink file",
                path.display()
            )));
        }
    }

    let before = snapshot_selected(session_dir)?;
    let parent = archive_path.parent().ok_or_else(|| {
        ServiceError::Internal(format!(
            "bundle path has no parent: {}",
            archive_path.display()
        ))
    })?;
    ensure_service_owned_result_directory(parent)?;
    match std::fs::symlink_metadata(archive_path) {
        Ok(_) => {
            return Err(ServiceError::Internal(format!(
                "refusing to overwrite existing bundle {}",
                archive_path.display()
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(ServiceError::Internal(io_msg(
                "stat bundle destination",
                archive_path,
                &error,
            )));
        }
    }
    let archive_name = archive_path.file_name().ok_or_else(|| {
        ServiceError::Internal(format!(
            "bundle path has no file name: {}",
            archive_path.display()
        ))
    })?;
    let mut partial_name = archive_name.to_os_string();
    partial_name.push(".partial");
    let partial = archive_path.with_file_name(partial_name);
    let partial_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&partial)
        .map_err(|error| {
            ServiceError::Internal(format!(
                "create new partial bundle {} without overwrite: {error}",
                partial.display()
            ))
        })?;

    let session = utf8_path(session_dir, "session directory")?;
    let mut published = false;
    let result = async {
        let mut command = Command::new("tar");
        command.args([
            "--zstd",
            "--sort=name",
            "--mtime=@0",
            "--owner=0",
            "--group=0",
            "--numeric-owner",
            "--format=posix",
            "--pax-option=delete=atime,delete=ctime",
            "-cf",
            "-",
            "-C",
            session,
            "staged",
            "artifacts",
            "control",
            "output",
        ]);
        command.env_remove("TAR_OPTIONS");
        let output = run_with_stdout_file(&mut command, partial_file).await?;
        if !output.status.success() {
            return Err(ServiceError::Internal(format!(
                "bundle: tar exited {:?}; stderr: {}; stdout: {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim(),
                "<archive bytes were directed to the exclusive partial file>"
            )));
        }
        let after = snapshot_selected(session_dir)?;
        if before.entries != after.entries
            || before.uncompressed_bytes != after.uncompressed_bytes
            || before.file_count != after.file_count
            || before.artifacts_file_count != after.artifacts_file_count
        {
            return Err(ServiceError::Internal(format!(
                "bundle: selected session tree changed while tar was reading it; no archive was accepted; first difference: {}",
                first_snapshot_difference(&before.entries, &after.entries)
            )));
        }
        let compressed_bytes = std::fs::metadata(&partial)
            .map_err(|error| {
                ServiceError::Internal(io_msg("stat partial bundle", &partial, &error))
            })?
            .len();
        if compressed_bytes == 0 {
            return Err(ServiceError::Internal(format!(
                "bundle: pinned tar produced an empty archive at {}",
                partial.display()
            )));
        }
        std::fs::File::open(&partial)
            .and_then(|file| file.sync_all())
            .map_err(|error| {
                ServiceError::Internal(io_msg("sync partial bundle", &partial, &error))
            })?;
        let sha256 = {
            let partial_for_hash = partial.clone();
            tokio::task::spawn_blocking(move || hash_file_sha256(&partial_for_hash))
                .await
                .map_err(|error| {
                    ServiceError::Internal(format!(
                        "bundle: blocking hash task terminated unexpectedly: {error}"
                    ))
                })??
        };
        std::fs::hard_link(&partial, archive_path).map_err(|error| {
            ServiceError::Internal(format!(
                "bundle: no-clobber publication {} -> {} failed: {error}",
                partial.display(),
                archive_path.display()
            ))
        })?;
        published = true;
        std::fs::remove_file(&partial).map_err(|error| {
            ServiceError::Internal(io_msg(
                "remove published partial bundle link",
                &partial,
                &error,
            ))
        })?;
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                ServiceError::Internal(io_msg("sync bundle directory", parent, &error))
            })?;

        Ok(BundleStats {
            sha256,
            compressed_bytes,
            uncompressed_bytes: before.uncompressed_bytes,
            file_count: before.file_count,
            artifacts_file_count: before.artifacts_file_count,
        })
    }
    .await;

    match result {
        Ok(stats) => Ok(stats),
        Err(error) => {
            let mut cleanup_errors = Vec::new();
            if published {
                if let Err(remove_error) = std::fs::remove_file(archive_path) {
                    cleanup_errors.push(io_msg(
                        "roll back failed bundle publication",
                        archive_path,
                        &remove_error,
                    ));
                }
            }
            if let Err(remove_error) = std::fs::remove_file(&partial) {
                if remove_error.kind() != std::io::ErrorKind::NotFound {
                    cleanup_errors.push(io_msg(
                        "remove failed partial bundle",
                        &partial,
                        &remove_error,
                    ));
                }
            }
            if cleanup_errors.is_empty() {
                return Err(error);
            }
            Err(ServiceError::Internal(format!(
                "{error}; bundle rollback also failed: {}",
                cleanup_errors.join("; ")
            )))
        }
    }
}

/// Create or validate one per-session result directory. The service process
/// owns this namespace; following a pre-created symlink or accepting ambient
/// umask drift would undermine every no-clobber publication check inside it.
pub(crate) fn ensure_service_owned_result_directory(path: &Path) -> ServiceResult<()> {
    let mut created = false;
    match std::fs::create_dir(path) {
        Ok(()) => {
            created = true;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(
                |error| ServiceError::Internal(io_msg("chmod result directory", path, &error)),
            )?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(ServiceError::Internal(io_msg(
                "create result directory",
                path,
                &error,
            )));
        }
    }
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| ServiceError::Internal(io_msg("stat result directory", path, &error)))?;
    let expected_uid = unsafe { libc::geteuid() };
    let expected_gid = unsafe { libc::getegid() };
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o777 != 0o755
        || metadata.uid() != expected_uid
        || metadata.gid() != expected_gid
    {
        return Err(ServiceError::Internal(format!(
            "result directory {} has unsafe type/mode/owner: type={:?} mode={:o} uid={} gid={} expected={}:{}",
            path.display(),
            metadata.file_type(),
            metadata.permissions().mode() & 0o777,
            metadata.uid(),
            metadata.gid(),
            expected_uid,
            expected_gid,
        )));
    }
    if created {
        let parent = path.parent().ok_or_else(|| {
            ServiceError::Internal(format!(
                "result directory {} has no parent to sync",
                path.display()
            ))
        })?;
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                ServiceError::Internal(io_msg(
                    "sync result root after directory creation",
                    parent,
                    &error,
                ))
            })?;
    }
    Ok(())
}

async fn run_with_stdout_file(
    command: &mut Command,
    stdout_file: std::fs::File,
) -> ServiceResult<Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = command.spawn().map_err(|error| {
        ServiceError::Internal(format!("bundle: cannot execute pinned tar: {error}"))
    })?;
    child.wait_with_output().await.map_err(|error| {
        ServiceError::Internal(format!("bundle: cannot wait for pinned tar: {error}"))
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EntryFingerprint {
    kind: u8,
    device: u64,
    inode: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    links: u64,
    size: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
    symlink_target: Option<OsString>,
}

#[derive(Default)]
struct TreeSnapshot {
    uncompressed_bytes: u64,
    file_count: u64,
    artifacts_file_count: u64,
    entries: BTreeMap<PathBuf, EntryFingerprint>,
}

fn snapshot_selected(session_dir: &Path) -> ServiceResult<TreeSnapshot> {
    let mut snapshot = TreeSnapshot::default();
    record_entry(session_dir, session_dir, &mut snapshot)?;
    for name in ["staged", "artifacts", "control", "output"] {
        let before = snapshot.file_count;
        walk(session_dir, &session_dir.join(name), &mut snapshot)?;
        if name == "artifacts" {
            snapshot.artifacts_file_count =
                snapshot.file_count.checked_sub(before).ok_or_else(|| {
                    ServiceError::Internal(format!(
                        "bundle artifact counter moved backwards: before={before}, after={}",
                        snapshot.file_count
                    ))
                })?;
        }
    }
    Ok(snapshot)
}

fn walk(base: &Path, path: &Path, snapshot: &mut TreeSnapshot) -> ServiceResult<()> {
    record_entry(base, path, snapshot)?;
    let mut children = std::fs::read_dir(path)
        .map_err(|error| ServiceError::Internal(io_msg("bundle read directory", path, &error)))?
        .map(|entry| {
            entry
                .map(|value| value.path())
                .map_err(|error| ServiceError::Internal(io_msg("bundle read entry", path, &error)))
        })
        .collect::<ServiceResult<Vec<_>>>()?;
    children.sort();
    for child in children {
        let metadata = std::fs::symlink_metadata(&child)
            .map_err(|error| ServiceError::Internal(io_msg("bundle stat entry", &child, &error)))?;
        if metadata.file_type().is_symlink() {
            record_metadata(base, &child, &metadata, snapshot)?;
            add_archive_entry_stats(snapshot, &child, metadata.len())?;
        } else if metadata.is_dir() {
            walk(base, &child, snapshot)?;
        } else if metadata.is_file() {
            record_metadata(base, &child, &metadata, snapshot)?;
            add_archive_entry_stats(snapshot, &child, metadata.len())?;
        } else {
            return Err(ServiceError::Internal(format!(
                "bundle refuses non-file/non-directory {}",
                child.display()
            )));
        }
    }
    Ok(())
}

fn add_archive_entry_stats(
    snapshot: &mut TreeSnapshot,
    path: &Path,
    entry_bytes: u64,
) -> ServiceResult<()> {
    let file_count = snapshot.file_count.checked_add(1).ok_or_else(|| {
        ServiceError::Internal(format!(
            "bundle archive-entry counter overflowed u64 while recording {}",
            path.display()
        ))
    })?;
    let uncompressed_bytes = snapshot
        .uncompressed_bytes
        .checked_add(entry_bytes)
        .ok_or_else(|| {
            ServiceError::Internal(format!(
                "bundle uncompressed-byte counter overflowed u64 while recording {}: current={} entry={entry_bytes}",
                path.display(),
                snapshot.uncompressed_bytes
            ))
        })?;
    snapshot.file_count = file_count;
    snapshot.uncompressed_bytes = uncompressed_bytes;
    Ok(())
}

fn record_entry(base: &Path, path: &Path, snapshot: &mut TreeSnapshot) -> ServiceResult<()> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| ServiceError::Internal(io_msg("bundle stat entry", path, &error)))?;
    if metadata.file_type().is_symlink() {
        return Err(ServiceError::Internal(format!(
            "bundle refuses symlink {}",
            path.display()
        )));
    }
    if !metadata.is_dir() && !metadata.is_file() {
        return Err(ServiceError::Internal(format!(
            "bundle refuses non-file/non-directory {}",
            path.display()
        )));
    }
    record_metadata(base, path, &metadata, snapshot)
}

fn record_metadata(
    base: &Path,
    path: &Path,
    metadata: &std::fs::Metadata,
    snapshot: &mut TreeSnapshot,
) -> ServiceResult<()> {
    let relative = path.strip_prefix(base).map_err(|error| {
        ServiceError::Internal(format!(
            "bundle path {} is outside selected root {}: {error}",
            path.display(),
            base.display()
        ))
    })?;
    let kind = if metadata.is_dir() {
        1
    } else if metadata.is_file() {
        2
    } else if metadata.file_type().is_symlink() {
        3
    } else {
        return Err(ServiceError::Internal(format!(
            "bundle refuses unsupported entry type {}",
            path.display()
        )));
    };
    let symlink_target = if metadata.file_type().is_symlink() {
        Some(
            std::fs::read_link(path)
                .map_err(|error| {
                    ServiceError::Internal(io_msg("bundle read symbolic link", path, &error))
                })?
                .into_os_string(),
        )
    } else {
        None
    };
    let fingerprint = EntryFingerprint {
        kind,
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.mode(),
        uid: metadata.uid(),
        gid: metadata.gid(),
        links: metadata.nlink(),
        size: metadata.size(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        changed_seconds: metadata.ctime(),
        changed_nanoseconds: metadata.ctime_nsec(),
        symlink_target,
    };
    if snapshot
        .entries
        .insert(relative.to_path_buf(), fingerprint)
        .is_some()
    {
        return Err(ServiceError::Internal(format!(
            "bundle encountered duplicate selected path {}",
            relative.display()
        )));
    }
    Ok(())
}

fn first_snapshot_difference(
    before: &BTreeMap<PathBuf, EntryFingerprint>,
    after: &BTreeMap<PathBuf, EntryFingerprint>,
) -> String {
    for path in before.keys().chain(after.keys()) {
        match (before.get(path), after.get(path)) {
            (Some(left), Some(right)) if left == right => {}
            (Some(left), Some(right)) => {
                return format!("{} metadata {:?} -> {:?}", path.display(), left, right);
            }
            (Some(_), None) => return format!("{} was removed", path.display()),
            (None, Some(_)) => return format!("{} was created", path.display()),
            (None, None) => {}
        }
    }
    "aggregate counters changed without a differing entry (internal invariant)".to_string()
}

fn utf8_path<'a>(path: &'a Path, role: &str) -> ServiceResult<&'a str> {
    path.to_str().ok_or_else(|| {
        ServiceError::Internal(format!("{role} path is not UTF-8: {}", path.display()))
    })
}

/// Streamed SHA-256 of one regular file's exact bytes.
pub(crate) fn hash_file_sha256(path: &Path) -> ServiceResult<String> {
    use sha2::{Digest, Sha256};

    let mut file = std::fs::File::open(path)
        .map_err(|error| ServiceError::Internal(io_msg("open file for hashing", path, &error)))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)
            .map_err(|error| ServiceError::Internal(io_msg("read file for hashing", path, &error)))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let mut rendered = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        write!(rendered, "{byte:02x}").expect("writing hex to a String cannot fail");
    }
    Ok(rendered)
}

pub async fn check_host_dependencies() -> ServiceResult<()> {
    for binary in ["tar", "zstd"] {
        let output = Command::new(binary)
            .arg("--version")
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|error| {
                ServiceError::Internal(format!("pinned service image lacks {binary}: {error}"))
            })?;
        if !output.status.success() {
            return Err(ServiceError::Internal(format!(
                "{binary} --version failed with {:?}",
                output.status.code()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::{symlink, PermissionsExt};

    use super::*;

    #[tokio::test]
    async fn configured_stdout_file_is_not_replaced_by_output_capture() {
        let path = std::env::temp_dir().join(format!(
            "agent-service-stdout-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .expect("test creates exclusive stdout file");
        let mut command = Command::new("sh");
        command.args(["-c", "printf archive-bytes"]);
        let output = run_with_stdout_file(&mut command, file)
            .await
            .expect("test command runs");
        assert!(output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            std::fs::read(&path).expect("test reads stdout file"),
            b"archive-bytes"
        );
        std::fs::remove_file(path).expect("test removes stdout file");
    }

    #[test]
    fn result_directory_is_exact_owned_mode_and_never_follows_symlink() {
        let root = std::env::temp_dir().join(format!(
            "qwen38-result-dir-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let results = root.join("results");
        std::fs::create_dir_all(&results).expect("create results fixture root");
        let owned = results.join("s-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        ensure_service_owned_result_directory(&owned).expect("create exact result directory");
        assert_eq!(
            std::fs::metadata(&owned)
                .expect("stat result directory")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );

        std::fs::set_permissions(&owned, std::fs::Permissions::from_mode(0o777))
            .expect("drift result mode");
        let error = ensure_service_owned_result_directory(&owned)
            .expect_err("unsafe existing mode must not be silently fixed");
        assert!(error.to_string().contains("unsafe type/mode/owner"));

        let outside = root.join("outside");
        std::fs::create_dir(&outside).expect("create outside fixture");
        let link = results.join("s-cccccccccccccccccccccccccccccccc");
        symlink(&outside, &link).expect("create hostile result symlink");
        let error = ensure_service_owned_result_directory(&link)
            .expect_err("result directory symlink must fail closed");
        assert!(error.to_string().contains("unsafe type/mode/owner"));
        assert!(outside.is_dir());
        std::fs::remove_dir_all(&root).expect("remove result directory fixture");
    }

    #[test]
    fn selected_tree_snapshot_detects_content_replacement_and_namespace_changes() {
        let root = std::env::temp_dir().join(format!(
            "qwen38-bundle-snapshot-{}",
            uuid::Uuid::new_v4().simple()
        ));
        for directory in ["staged", "artifacts", "control", "output"] {
            std::fs::create_dir_all(root.join(directory)).expect("create snapshot fixture");
        }
        let selected = root.join("staged/value.bin");
        std::fs::write(&selected, b"AAAA").expect("write original selected content");
        let before = snapshot_selected(&root).expect("snapshot original selected tree");

        // Atomic same-length replacement cannot hide behind an unchanged byte
        // count or pathname: inode and directory metadata are both frozen.
        let replacement = root.join("staged/replacement.bin");
        std::fs::write(&replacement, b"BBBB").expect("write replacement content");
        std::fs::rename(&replacement, &selected).expect("atomically replace selected file");
        let replaced = snapshot_selected(&root).expect("snapshot replaced selected tree");
        assert_ne!(before.entries, replaced.entries);
        let replacement_difference = first_snapshot_difference(&before.entries, &replaced.entries);
        assert!(replacement_difference.contains("staged"));

        let created = root.join("artifacts/new.txt");
        std::fs::write(&created, b"new").expect("create selected artifact");
        let with_created = snapshot_selected(&root).expect("snapshot created entry");
        assert_ne!(replaced.entries, with_created.entries);
        assert_eq!(with_created.artifacts_file_count, 1);
        assert!(
            first_snapshot_difference(&replaced.entries, &with_created.entries)
                .contains("artifacts")
        );

        std::fs::remove_file(&created).expect("remove selected artifact");
        let after_removal = snapshot_selected(&root).expect("snapshot removed entry");
        assert_eq!(after_removal.artifacts_file_count, 0);
        assert_ne!(with_created.entries, after_removal.entries);

        std::fs::remove_dir_all(&root).expect("remove bundle snapshot fixture");
    }

    #[test]
    fn archive_stats_fail_closed_on_counter_overflow() {
        let path = Path::new("staged/overflow-fixture");

        let mut count_overflow = TreeSnapshot {
            file_count: u64::MAX,
            ..TreeSnapshot::default()
        };
        let error = add_archive_entry_stats(&mut count_overflow, path, 0)
            .expect_err("archive-entry counter overflow must be explicit");
        assert!(error
            .to_string()
            .contains("archive-entry counter overflowed"));
        assert_eq!(count_overflow.file_count, u64::MAX);
        assert_eq!(count_overflow.uncompressed_bytes, 0);

        let mut byte_overflow = TreeSnapshot {
            uncompressed_bytes: u64::MAX,
            ..TreeSnapshot::default()
        };
        let error = add_archive_entry_stats(&mut byte_overflow, path, 1)
            .expect_err("uncompressed-byte counter overflow must be explicit");
        assert!(error
            .to_string()
            .contains("uncompressed-byte counter overflowed"));
        assert_eq!(byte_overflow.file_count, 0);
        assert_eq!(byte_overflow.uncompressed_bytes, u64::MAX);
    }

    #[test]
    fn snapshot_and_tar_preserve_symlinks_as_links_without_reading_their_targets() {
        let root = std::env::temp_dir().join(format!(
            "qwen38-bundle-links-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let session = root.join("session");
        let extracted = root.join("extracted");
        for directory in ["staged", "artifacts", "control", "output"] {
            std::fs::create_dir_all(session.join(directory)).expect("create bundle fixture");
        }
        for (relative, bytes) in [
            ("control/prompt.txt", b"prompt\n".as_slice()),
            (
                "control/turn-budget.json",
                b"{\"max_session_turns\":400}\n".as_slice(),
            ),
            ("output/ready.json", b"{}\n".as_slice()),
            ("output/events.jsonl", b"{}\n".as_slice()),
            ("output/qwen.stderr", b"".as_slice()),
            ("output/qwen-exit-code", b"0\n".as_slice()),
            ("output/response.txt", b"done\n".as_slice()),
        ] {
            std::fs::write(session.join(relative), bytes).expect("write required bundle file");
        }
        let outside = root.join("outside-secret.txt");
        std::fs::write(&outside, b"must-not-be-archived-as-link-content")
            .expect("write outside target");
        symlink(&outside, session.join("staged/absolute-outside"))
            .expect("create absolute outside link");
        symlink("missing-target", session.join("staged/dangling")).expect("create dangling link");
        let non_utf8_target = OsString::from_vec(vec![b'n', b'o', b'n', b'-', 0xff]);
        symlink(&non_utf8_target, session.join("staged/non-utf8"))
            .expect("create non-UTF8 target link");
        symlink("../staged/dangling", session.join("artifacts/link-chain"))
            .expect("create link chain");
        let snapshot = snapshot_selected(&session).expect("snapshot accepts opaque links");
        assert_eq!(snapshot.file_count, 11);
        assert_eq!(snapshot.artifacts_file_count, 1);
        assert!(snapshot.uncompressed_bytes >= 3);

        // The Rust compiler stage deliberately contains GNU tar but not the
        // production service image's pinned zstd binary. Compression is
        // orthogonal to link traversal, so this focused unit uses the same
        // recursive tar semantics without compression. The accepted runtime
        // path separately exercises create_bundle and its zstd dependency.
        let archive = root.join("bundle.tar");
        let creation = std::process::Command::new("tar")
            .args([
                "--sort=name",
                "--mtime=@0",
                "--owner=0",
                "--group=0",
                "--numeric-owner",
                "--format=posix",
                "--pax-option=delete=atime,delete=ctime",
                "-cf",
                archive.to_str().expect("UTF-8 archive fixture"),
                "-C",
                session.to_str().expect("UTF-8 session fixture"),
                "staged",
                "artifacts",
                "control",
                "output",
            ])
            .env("TAR_OPTIONS", "--dereference")
            .env_remove("TAR_OPTIONS")
            .output()
            .expect("run pinned tar for fixture creation");
        assert!(
            creation.status.success(),
            "fixture archive creation failed: {}",
            String::from_utf8_lossy(&creation.stderr)
        );

        std::fs::create_dir(&extracted).expect("create extraction root");
        let output = std::process::Command::new("tar")
            .args([
                "--extract",
                "--file",
                archive.to_str().expect("UTF-8 archive fixture"),
                "--directory",
                extracted.to_str().expect("UTF-8 extraction fixture"),
            ])
            .output()
            .expect("run pinned tar for fixture extraction");
        assert!(
            output.status.success(),
            "fixture extraction failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        for (relative, expected) in [
            (
                "staged/absolute-outside",
                outside.as_os_str().to_os_string(),
            ),
            ("staged/dangling", OsString::from("missing-target")),
            ("staged/non-utf8", non_utf8_target),
            ("artifacts/link-chain", OsString::from("../staged/dangling")),
        ] {
            let link = extracted.join(relative);
            assert!(std::fs::symlink_metadata(&link)
                .expect("stat extracted link")
                .file_type()
                .is_symlink());
            assert_eq!(
                std::fs::read_link(&link)
                    .expect("read extracted target")
                    .into_os_string(),
                expected
            );
        }
        assert_eq!(
            std::fs::read(&outside).expect("read untouched outside target"),
            b"must-not-be-archived-as-link-content"
        );

        std::fs::remove_dir_all(&root).expect("remove bundle-link fixture");
    }
}
