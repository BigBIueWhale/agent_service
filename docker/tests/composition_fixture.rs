use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub root: PathBuf,
    pub case: Case,
    pub agent_image: String,
    pub relay_image: String,
    pub capture_image: String,
    pub session_ids: Vec<String>,
}

#[derive(Debug, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Case {
    ToolCycle,
    CancelStartGate,
    CancelHeldSse,
    ProviderMalformedSse,
    ProviderDisconnectedSse,
    CaptureProofHeldCancel,
    CaptureProofLost,
    CaptureByteMismatch,
    CaptureModeMismatch,
    WaitObservationLost,
    RemoveStoppedCaptureFailed,
    QuiescenceObservationLost,
    TerminalPrepareFileSync,
    TerminalPrepareDirectorySync,
    TerminalPublishLink,
    TerminalPublishDirectorySync,
    TerminalRawParentSync,
    TerminalTemporaryUnlink,
    TerminalTemporaryUnlinkDirectorySync,
}

impl Case {
    pub fn has_terminal_faults(&self) -> bool {
        matches!(
            self,
            Self::TerminalPrepareFileSync
                | Self::TerminalPrepareDirectorySync
                | Self::TerminalPublishLink
                | Self::TerminalPublishDirectorySync
                | Self::TerminalRawParentSync
                | Self::TerminalTemporaryUnlink
                | Self::TerminalTemporaryUnlinkDirectorySync
        )
    }
}

impl Fixture {
    pub fn read() -> Self {
        let fixture: Self = serde_json::from_slice(
            &std::fs::read("/gate/fixture.json").expect("read mounted gate fixture"),
        )
        .expect("decode exact gate fixture");
        assert_eq!(unsafe { libc::geteuid() }, 1000);
        assert_eq!(fixture.root.parent(), Some(std::path::Path::new("/tmp")));
        assert!(fixture
            .root
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("as-gate-"));
        assert_eq!(std::fs::canonicalize(&fixture.root).unwrap(), fixture.root);
        let metadata = std::fs::symlink_metadata(&fixture.root).unwrap();
        assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
        assert_eq!(metadata.uid(), 1000);
        assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
        assert!(!fixture.session_ids.is_empty());
        assert_eq!(
            fixture
                .session_ids
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            fixture.session_ids.len()
        );
        for id in &fixture.session_ids {
            assert_eq!(id.len(), 66);
            assert!(id.starts_with("s-"));
            assert!(id[2..]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        }
        for id in [
            &fixture.agent_image,
            &fixture.relay_image,
            &fixture.capture_image,
        ] {
            assert_eq!(id.len(), 71);
            assert!(id.starts_with("sha256:"));
            assert!(id[7..]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        }
        fixture
    }
}
