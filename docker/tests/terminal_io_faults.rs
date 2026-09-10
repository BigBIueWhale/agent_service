//! Test-only observation and EIO injection around actual terminal syscalls.
//! Registrations own fresh fixture roots; no switch exists in released binaries.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    v: u32,
    id: uuid::Uuid,
    points: Vec<Point>,
}

#[derive(PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Point {
    site: Site,
    phase: Phase,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Site {
    PrepareWrite,
    PrepareFileSync,
    PrepareDirectorySync,
    ResumeFileSync,
    ResumeDirectorySync,
    RawRemove,
    RawParentSync,
    PublishLink,
    PublishedDirectorySync,
    TemporaryUnlink,
    TemporaryUnlinkDirectorySync,
    ReconcileFileSync,
    ReconcileDirectorySync,
    ReconcileUnlink,
    ReconcileUnlinkDirectorySync,
}

impl Site {
    pub(crate) fn from_context(context: &str) -> Option<Self> {
        Some(match context {
            "prepare_terminal: write tmp" => Self::PrepareWrite,
            "prepare_terminal: sync tmp" => Self::PrepareFileSync,
            "prepare_terminal: sync result directory" => Self::PrepareDirectorySync,
            "resume terminal: sync prepared draft" => Self::ResumeFileSync,
            "resume terminal: sync prepared result directory" => Self::ResumeDirectorySync,
            "terminalization: remove raw session tree" => Self::RawRemove,
            "terminalization: sync sessions directory"
            | "terminalization: sync absent raw session tree" => Self::RawParentSync,
            "commit_prepared_terminal: no-clobber publish" => Self::PublishLink,
            "commit_prepared_terminal: sync published final" => Self::PublishedDirectorySync,
            "commit_prepared_terminal: remove published tmp link" => Self::TemporaryUnlink,
            "commit_prepared_terminal: sync tmp-link removal" => Self::TemporaryUnlinkDirectorySync,
            "terminal sweep: sync committed file" => Self::ReconcileFileSync,
            "terminal sweep: sync committed publication" => Self::ReconcileDirectorySync,
            "terminal sweep: remove publication marker" => Self::ReconcileUnlink,
            "terminal sweep after linked-publication recovery" => {
                Self::ReconcileUnlinkDirectorySync
            }
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Before,
    After,
}

struct Registration {
    control: PathBuf,
    boot: uuid::Uuid,
    next: u64,
    plan: Option<Plan>,
    hits: BTreeSet<(Site, Phase)>,
    plan_ids: BTreeSet<uuid::Uuid>,
    failure: Option<String>,
}

impl Registration {
    fn require_consumed(&self) -> io::Result<()> {
        if let Some(failure) = &self.failure {
            return Err(io::Error::other(format!(
                "terminal I/O fixture failed: {failure}"
            )));
        }
        if self
            .plan
            .as_ref()
            .is_some_and(|plan| plan.points.len() != self.hits.len())
        {
            return Err(io::Error::other(
                "terminal I/O fixture has an unconsumed fault",
            ));
        }
        Ok(())
    }

    fn read_plan(&mut self) -> io::Result<()> {
        let plan: Plan =
            serde_json::from_slice(&std::fs::read(self.control.join("terminal-io-plan.json"))?)?;
        if plan.v != 1 || plan.id.is_nil() {
            return Err(io::Error::other("unsupported terminal I/O fixture plan"));
        }
        let keys: BTreeSet<_> = plan
            .points
            .iter()
            .map(|point| (point.site, point.phase))
            .collect();
        if keys.len() != plan.points.len() {
            return Err(io::Error::other("duplicate terminal I/O fixture fault"));
        }
        if self
            .plan
            .as_ref()
            .is_some_and(|previous| previous.id == plan.id)
        {
            if self.plan.as_ref() != Some(&plan) {
                return Err(io::Error::other(
                    "terminal I/O fixture plan changed under the same identity",
                ));
            }
        } else {
            self.require_consumed()?;
            if !self.plan_ids.insert(plan.id) {
                return Err(io::Error::other(
                    "terminal I/O fixture plan identity was reused",
                ));
            }
            self.plan = Some(plan);
            self.hits.clear();
        }
        Ok(())
    }
}

fn registrations() -> &'static Mutex<BTreeMap<PathBuf, Registration>> {
    static REGISTRATIONS: OnceLock<Mutex<BTreeMap<PathBuf, Registration>>> = OnceLock::new();
    REGISTRATIONS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub(crate) struct Guard(PathBuf);

impl Drop for Guard {
    fn drop(&mut self) {
        let mut registration = registrations().lock().unwrap().remove(&self.0).unwrap();
        let result = registration
            .read_plan()
            .and_then(|()| registration.require_consumed());
        if !std::thread::panicking() {
            result.expect("every armed terminal I/O fault must be consumed exactly once");
        }
    }
}

pub(crate) fn install(root: &Path, control: &Path) -> Guard {
    assert_eq!(std::fs::canonicalize(root).unwrap(), root);
    assert_eq!(std::fs::canonicalize(control).unwrap(), control);
    assert!(control.starts_with(root));
    let mut active = registrations().lock().unwrap();
    assert!(!active
        .keys()
        .any(|other| root.starts_with(other) || other.starts_with(root)));
    active.insert(
        root.to_path_buf(),
        Registration {
            control: control.to_path_buf(),
            boot: uuid::Uuid::new_v4(),
            next: 0,
            plan: None,
            hits: BTreeSet::new(),
            plan_ids: BTreeSet::new(),
            failure: None,
        },
    );
    Guard(root.to_path_buf())
}

pub(crate) fn point(path: &Path, context: &str, phase: Phase) -> io::Result<()> {
    let Some(site) = Site::from_context(context) else {
        return Ok(());
    };
    let mut active = registrations().lock().unwrap();
    let Some((root, registration)) = active.iter_mut().find(|(root, _)| path.starts_with(root))
    else {
        return Ok(());
    };
    if let Some(failure) = &registration.failure {
        return Err(io::Error::other(format!(
            "terminal I/O fixture failed: {failure}"
        )));
    }
    match record_point(root, registration, path, context, phase, site) {
        Ok(true) => Err(io::Error::from_raw_os_error(libc::EIO)),
        Ok(false) => Ok(()),
        Err(error) => {
            registration.failure = Some(error.to_string());
            Err(error)
        }
    }
}

fn record_point(
    root: &Path,
    registration: &mut Registration,
    path: &Path,
    context: &str,
    phase: Phase,
    site: Site,
) -> io::Result<bool> {
    registration.read_plan()?;
    let plan = registration.plan.as_ref().unwrap();
    let inject = plan
        .points
        .iter()
        .any(|point| point.site == site && point.phase == phase);
    if inject && !registration.hits.insert((site, phase)) {
        return Err(io::Error::other(
            "terminal I/O fixture fault hit more than once",
        ));
    }
    let sequence = registration.next;
    registration.next = registration
        .next
        .checked_add(1)
        .expect("test receipt sequence overflow");
    let receipt = registration.control.join(format!(
        "terminal-io-{}-{sequence:06}.json",
        registration.boot
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&receipt)?;
    let path_state = match std::fs::symlink_metadata(path) {
        Ok(metadata) => serde_json::json!({
            "kind": "present", "device": metadata.dev(), "inode": metadata.ino(),
            "links": metadata.nlink(), "mode": metadata.mode(), "bytes": metadata.len(),
            "uid": metadata.uid(), "gid": metadata.gid(),
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            serde_json::json!({"kind": "absent"})
        }
        Err(error) => return Err(error),
    };
    let bytes = serde_json::to_vec(&serde_json::json!({
        "v": 1, "boot": registration.boot, "plan_id": plan.id, "sequence": sequence,
        "path": path.strip_prefix(root).unwrap(), "path_state": path_state,
        "site": site, "context": context, "phase": phase,
        "operation_returned_successfully": phase == Phase::After,
        "injected_errno": if inject { Some(libc::EIO) } else { None },
    }))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    std::fs::File::open(&registration.control)?.sync_all()?;
    Ok(inject)
}

#[test]
fn infrastructure_failure_cannot_be_cleared_by_a_later_valid_plan() {
    for case in [
        "extra_hit",
        "changed_plan",
        "unreadable_plan",
        "receipt_failure",
    ] {
        let root =
            std::env::temp_dir().join(format!("terminal-fault-audit-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let control = root.join("control");
        std::fs::create_dir(&control).unwrap();
        let path = root.join("draft");
        std::fs::write(&path, b"owned fixture bytes").unwrap();
        let plan_path = control.join("terminal-io-plan.json");
        let id = uuid::Uuid::new_v4();
        let armed = serde_json::json!({"v": 1, "id": id, "points": [{
            "site": "prepare_file_sync", "phase": "before",
        }]});
        let write_plan = |plan: &serde_json::Value| {
            std::fs::write(&plan_path, serde_json::to_vec(plan).unwrap()).unwrap();
        };
        write_plan(&armed);
        let guard = install(&root, &control);
        let call = || point(&path, "prepare_terminal: sync tmp", Phase::Before);
        if case == "receipt_failure" {
            let boot = registrations().lock().unwrap()[&root].boot;
            std::fs::create_dir(control.join(format!("terminal-io-{boot}-000000.json"))).unwrap();
        } else {
            assert_eq!(call().unwrap_err().raw_os_error(), Some(libc::EIO));
            match case {
                "extra_hit" => {}
                "changed_plan" => write_plan(&serde_json::json!({"v": 1, "id": id, "points": []})),
                "unreadable_plan" => std::fs::remove_file(&plan_path).unwrap(),
                _ => unreachable!(),
            }
        }
        let infrastructure_error = call().unwrap_err();
        assert_ne!(infrastructure_error.raw_os_error(), Some(libc::EIO));
        write_plan(&serde_json::json!({"v": 1, "id": uuid::Uuid::new_v4(), "points": []}));
        assert!(call().unwrap_err().to_string().contains("fixture failed"));
        assert!(
            std::panic::catch_unwind(|| drop(guard)).is_err(),
            "a caller catching a context-wrapped error must not erase fixture failure"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
