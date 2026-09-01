//! Fixed Qwen launcher for the sole agent runtime.
//!
//! The trusted shell wrapper launches this binary with no arguments and two
//! already-open control descriptors: fd 3 receives one exact sandbox
//! attestation and fd 4 releases execution. Qwen stdout/stderr are connected
//! to the fixed capture sockets before the launcher enters a Landlock domain
//! that permits ordinary filesystem writes only in the four documented mutable
//! roots. It separately permits write-open access to the container's private
//! devpts instance so Qwen Code's native shell tool can allocate a PTY. `/output`
//! is not mounted in this container at all.

use std::ffi::CString;
use std::io;
use std::mem;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::process::{Command, ExitCode};

const SANDBOX_ID: &str =
    "landlock-fs-v4-write-roots-v1+private-devpts-rw-v1+output-unmounted-v1";
const EVENTS_SOCKET: &str = "/streams/events.sock";
const STDERR_SOCKET: &str = "/streams/stderr.sock";
/// The sole per-session turn budget input.
///
/// The agent container has no environment knob, argument, or network path that
/// could carry a per-session value, so the service publishes the accepted
/// budget as one canonical read-only record in the control mount and this
/// launcher reads it here. The launcher still accepts no arguments: the record
/// is trusted service-owned state, not caller-controlled argv.
const TURN_BUDGET_FILE: &str = "/run/agent/turn-budget.json";
/// The canonical record is one short line; anything larger is malformed by
/// construction and is refused without being read as a budget.
const MAX_TURN_BUDGET_BYTES: u64 = 64;
/// Session turn budget: the one bound on how long a session may work. Turns,
/// never wall time -- a wall-clock budget would measure backend generation speed
/// rather than agent progress. Qwen Code stops itself here and exits 53, an
/// ordinary terminal outcome.
///
/// This is the budget a session that named none was accepted with. It must
/// equal `limits.max_session_turns` in the stack lock,
/// `execution.max_session_turns` in the agent runtime contract, and
/// `model.maxSessionTurns` in both sealed settings files; the in-image
/// runtime-contract verifier proves the contract, the settings, and this
/// declaration agree, and the service proves the lock equals its own compiled
/// constant.
const DEFAULT_MAX_SESSION_TURNS: u32 = 400;
/// The largest budget a session may have been accepted with. The service
/// refuses a larger request before acceptance; this launcher refuses a larger
/// record before exec, so a turn budget beyond the reviewed bound cannot reach
/// Qwen Code even if the control mount were wrong. It must equal
/// `limits.max_session_turns_ceiling` in the stack lock and
/// `execution.max_session_turns_ceiling` in the agent runtime contract.
const MAX_SESSION_TURNS_CEILING: u32 = 2000;
// The sealed default must itself be a runnable budget.
const _: () = assert!(
    DEFAULT_MAX_SESSION_TURNS >= 1 && DEFAULT_MAX_SESSION_TURNS <= MAX_SESSION_TURNS_CEILING,
    "the sealed default session turn budget must lie inside the accepted range"
);
const NODE: &str = "/usr/local/bin/node";
const CLI: &str = "/opt/qwen-code/scripts/cli-entry.js";
const CONTROL_ATTEST_FD: RawFd = 3;
const CONTROL_RELEASE_FD: RawFd = 4;

const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1;
const LANDLOCK_RULE_PATH_BENEATH: u32 = 1;
const LANDLOCK_ACCESS_FS_WRITE_FILE: u64 = 1 << 1;
const LANDLOCK_ACCESS_FS_REMOVE_DIR: u64 = 1 << 4;
const LANDLOCK_ACCESS_FS_REMOVE_FILE: u64 = 1 << 5;
const LANDLOCK_ACCESS_FS_MAKE_CHAR: u64 = 1 << 6;
const LANDLOCK_ACCESS_FS_MAKE_DIR: u64 = 1 << 7;
const LANDLOCK_ACCESS_FS_MAKE_REG: u64 = 1 << 8;
const LANDLOCK_ACCESS_FS_MAKE_SOCK: u64 = 1 << 9;
const LANDLOCK_ACCESS_FS_MAKE_FIFO: u64 = 1 << 10;
const LANDLOCK_ACCESS_FS_MAKE_BLOCK: u64 = 1 << 11;
const LANDLOCK_ACCESS_FS_MAKE_SYM: u64 = 1 << 12;
const LANDLOCK_ACCESS_FS_REFER: u64 = 1 << 13;
const LANDLOCK_ACCESS_FS_TRUNCATE: u64 = 1 << 14;
const DIRECTORY_WRITE_ACCESS: u64 = LANDLOCK_ACCESS_FS_WRITE_FILE
    | LANDLOCK_ACCESS_FS_REMOVE_DIR
    | LANDLOCK_ACCESS_FS_REMOVE_FILE
    | LANDLOCK_ACCESS_FS_MAKE_CHAR
    | LANDLOCK_ACCESS_FS_MAKE_DIR
    | LANDLOCK_ACCESS_FS_MAKE_REG
    | LANDLOCK_ACCESS_FS_MAKE_SOCK
    | LANDLOCK_ACCESS_FS_MAKE_FIFO
    | LANDLOCK_ACCESS_FS_MAKE_BLOCK
    | LANDLOCK_ACCESS_FS_MAKE_SYM
    | LANDLOCK_ACCESS_FS_REFER
    | LANDLOCK_ACCESS_FS_TRUNCATE;
const FILE_WRITE_ACCESS: u64 = LANDLOCK_ACCESS_FS_WRITE_FILE | LANDLOCK_ACCESS_FS_TRUNCATE;
const DEVICE_WRITE_ACCESS: u64 = LANDLOCK_ACCESS_FS_WRITE_FILE;

const STRICT_TOOLS: &str =
    "agent,edit,glob,grep_search,list_directory,notebook_edit,read_file,run_shell_command,todo_write,write_file";

#[repr(C)]
struct LandlockRulesetAttr {
    handled_access_fs: u64,
    handled_access_net: u64,
}

#[repr(C, packed)]
struct LandlockPathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

fn main() -> ExitCode {
    match run() {
        Ok(never) => match never {},
        Err(error) => {
            eprintln!("agent_exec: {error}");
            ExitCode::from(111)
        }
    }
}

fn run() -> Result<std::convert::Infallible, String> {
    let arguments = std::env::args_os().collect::<Vec<_>>();
    if arguments.len() != 1 {
        return Err("no arguments are accepted; the Qwen command is compiled in".into());
    }
    if std::env::consts::ARCH != "x86_64" || std::env::consts::OS != "linux" {
        return Err(format!(
            "{SANDBOX_ID} supports exactly linux/x86_64, observed {}/{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
    }
    if unsafe { libc::geteuid() } != 1000 || unsafe { libc::getegid() } != 1000 {
        return Err(format!(
            "launcher identity drift: expected 1000:1000, observed {}:{}",
            unsafe { libc::geteuid() },
            unsafe { libc::getegid() }
        ));
    }
    require_control_fd(CONTROL_ATTEST_FD, "attestation")?;
    require_control_fd(CONTROL_RELEASE_FD, "release")?;
    if std::path::Path::new("/output").exists() {
        return Err("/output must be absent from the Qwen container mount namespace".into());
    }
    // Read the session's budget before anything else is set up: a session that
    // cannot be given an exact bound must not reach the model at all.
    let max_session_turns = read_session_turn_budget()?;

    let events = UnixStream::connect(EVENTS_SOCKET)
        .map_err(|error| format!("connect fixed event capture socket {EVENTS_SOCKET}: {error}"))?;
    let stderr = UnixStream::connect(STDERR_SOCKET)
        .map_err(|error| format!("connect fixed stderr capture socket {STDERR_SOCKET}: {error}"))?;
    install_filesystem_sandbox()?;

    duplicate_stream(events.as_raw_fd(), libc::STDOUT_FILENO, "stdout")?;
    duplicate_stream(stderr.as_raw_fd(), libc::STDERR_FILENO, "stderr")?;
    drop(events);
    drop(stderr);

    write_all_fd(
        CONTROL_ATTEST_FD,
        format!("AGENT_EXEC_READY sandbox={SANDBOX_ID}\n").as_bytes(),
        "write sandbox attestation",
    )?;
    let mut release = [0u8; 5];
    read_exact_fd(
        CONTROL_RELEASE_FD,
        &mut release,
        "read wrapper release gate",
    )?;
    if release != *b"EXEC\n" {
        return Err(format!(
            "wrapper release gate drift: expected EXEC\\n, observed {release:?}"
        ));
    }

    // No wrapper/control descriptor, capture-socket original, or incidental
    // shell descriptor may reach Qwen. stdin/stdout/stderr are the only
    // inherited descriptors by contract.
    let close_result = unsafe { libc::close_range(3, u32::MAX, 0) };
    if close_result != 0 {
        return Err(format!(
            "close every non-standard descriptor before Qwen exec: {}",
            io::Error::last_os_error()
        ));
    }

    let error = std::os::unix::process::CommandExt::exec(
        Command::new(NODE)
            .arg(CLI)
            .arg("--input-format=text")
            .arg("--approval-mode=yolo")
            .arg("--output-format=stream-json")
            .arg(format!("--strict-tools={STRICT_TOOLS}"))
            .arg("--foreground-agents-only")
            .arg("--max-subagent-depth=1")
            .arg(format!("--max-session-turns={max_session_turns}"))
            .arg("--max-tool-calls=-1")
            .arg("--no-chat-recording"),
    );
    Err(format!("exec pinned Qwen Code entrypoint: {error}"))
}

/// Read the accepted per-session turn budget from the sealed control mount.
///
/// The record is service-owned state on a read-only bind mount, and it is
/// checked like one: exactly the ownership, mode, and bytes the service
/// publishes. There is no default-on-failure path -- an unreadable, drifted,
/// or malformed record fails the session instead of silently substituting the
/// sealed default, because a session that quietly ran on a different budget
/// than the one it was accepted with would be graded as though it had not.
fn read_session_turn_budget() -> Result<u32, String> {
    let metadata = std::fs::symlink_metadata(TURN_BUDGET_FILE).map_err(|error| {
        format!("stat sealed session turn budget {TURN_BUDGET_FILE}: {error}")
    })?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.uid() != 1000
        || metadata.gid() != 1000
        || metadata.permissions().mode() & 0o777 != 0o444
        || metadata.len() > MAX_TURN_BUDGET_BYTES
    {
        return Err(format!(
            "{TURN_BUDGET_FILE} must be a regular non-symlink uid:gid 1000:1000 mode-0444 record of at most {MAX_TURN_BUDGET_BYTES} bytes; observed type={:?} uid={} gid={} mode={:o} bytes={}",
            metadata.file_type(),
            metadata.uid(),
            metadata.gid(),
            metadata.permissions().mode() & 0o777,
            metadata.len()
        ));
    }
    let raw = std::fs::read(TURN_BUDGET_FILE)
        .map_err(|error| format!("read sealed session turn budget {TURN_BUDGET_FILE}: {error}"))?;
    parse_session_turn_budget(&raw)
}

/// Parse the one canonical spelling of the record and nothing else.
///
/// The service writes these bytes; both sides compare bytes rather than
/// accepting whatever a JSON parser would tolerate, so a rewritten,
/// re-indented, or extended record is a failure rather than a reinterpretation.
fn parse_session_turn_budget(raw: &[u8]) -> Result<u32, String> {
    let malformed = |detail: &str| {
        format!(
            "{TURN_BUDGET_FILE} is not one canonical {{\"max_session_turns\":N}} line with N in 1..={MAX_SESSION_TURNS_CEILING} (the sealed default is {DEFAULT_MAX_SESSION_TURNS}): {detail}"
        )
    };
    let text = std::str::from_utf8(raw).map_err(|error| malformed(&format!("{error}")))?;
    let digits = text
        .strip_prefix("{\"max_session_turns\":")
        .and_then(|rest| rest.strip_suffix("}\n"))
        .ok_or_else(|| malformed(&format!("observed {text:?}")))?;
    if digits.is_empty()
        || digits.len() > 10
        || digits.starts_with('0')
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(malformed(&format!("observed turn count {digits:?}")));
    }
    let turns: u32 = digits
        .parse()
        .map_err(|error| malformed(&format!("turn count {digits:?}: {error}")))?;
    if turns == 0 || turns > MAX_SESSION_TURNS_CEILING {
        return Err(malformed(&format!("turn count {turns} is outside the accepted range")));
    }
    Ok(turns)
}

fn require_control_fd(fd: RawFd, label: &str) -> Result<(), String> {
    if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 {
        return Err(format!(
            "required {label} control fd {fd} is absent: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn duplicate_stream(source: RawFd, destination: RawFd, label: &str) -> Result<(), String> {
    if unsafe { libc::dup2(source, destination) } != destination {
        return Err(format!(
            "install captured Qwen {label}: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn write_all_fd(fd: RawFd, mut bytes: &[u8], label: &str) -> Result<(), String> {
    while !bytes.is_empty() {
        let written = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if written < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("{label}: {error}"));
        }
        if written == 0 {
            return Err(format!("{label}: zero-byte write"));
        }
        bytes = &bytes[written as usize..];
    }
    Ok(())
}

fn read_exact_fd(fd: RawFd, mut bytes: &mut [u8], label: &str) -> Result<(), String> {
    while !bytes.is_empty() {
        let count = unsafe { libc::read(fd, bytes.as_mut_ptr().cast(), bytes.len()) };
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("{label}: {error}"));
        }
        if count == 0 {
            return Err(format!("{label}: unexpected EOF"));
        }
        let (_, remaining) = mem::take(&mut bytes).split_at_mut(count as usize);
        bytes = remaining;
    }
    Ok(())
}

fn install_filesystem_sandbox() -> Result<(), String> {
    install_filesystem_sandbox_with_rules(&[
        ("/workspace", DIRECTORY_WRITE_ACCESS),
        ("/artifacts", DIRECTORY_WRITE_ACCESS),
        ("/tmp", DIRECTORY_WRITE_ACCESS),
        ("/qwen-runtime", DIRECTORY_WRITE_ACCESS),
        ("/dev/null", FILE_WRITE_ACCESS),
        // Docker gives the agent its own PID namespace and private devpts
        // instance. forkpty(3), used by Qwen Code's run_shell_command tool,
        // must write-open both ptmx and the resulting slave under /dev/pts.
        // WRITE_FILE is sufficient; creation, removal, rename, and truncation
        // remain denied throughout this device filesystem.
        ("/dev/pts", DEVICE_WRITE_ACCESS),
    ])
}

fn install_filesystem_sandbox_with_rules(rules: &[(&str, u64)]) -> Result<(), String> {
    let abi = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<LandlockRulesetAttr>(),
            0usize,
            LANDLOCK_CREATE_RULESET_VERSION,
        )
    };
    if abi < 4 {
        if abi < 0 {
            return Err(format!(
                "query Landlock ABI for {SANDBOX_ID}: {}",
                io::Error::last_os_error()
            ));
        }
        return Err(format!(
            "{SANDBOX_ID} requires Landlock ABI >=4, observed ABI {abi}"
        ));
    }
    let attr = LandlockRulesetAttr {
        handled_access_fs: DIRECTORY_WRITE_ACCESS,
        handled_access_net: 0,
    };
    let ruleset_fd = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            &attr,
            mem::size_of::<LandlockRulesetAttr>(),
            0u32,
        )
    };
    if ruleset_fd < 0 {
        return Err(format!(
            "create {SANDBOX_ID} ruleset: {}",
            io::Error::last_os_error()
        ));
    }
    let ruleset_fd = ruleset_fd as RawFd;
    let mut result = Ok(());
    for &(path, access) in rules {
        if let Err(error) = add_path_rule(ruleset_fd, path, access) {
            result = Err(error);
            break;
        }
    }
    if result.is_ok() && unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        result = Err(format!(
            "set no_new_privs before {SANDBOX_ID}: {}",
            io::Error::last_os_error()
        ));
    }
    if result.is_ok() {
        let restricted =
            unsafe { libc::syscall(libc::SYS_landlock_restrict_self, ruleset_fd, 0u32) };
        if restricted != 0 {
            result = Err(format!(
                "enter {SANDBOX_ID}: {}",
                io::Error::last_os_error()
            ));
        }
    }
    unsafe { libc::close(ruleset_fd) };
    result
}

fn add_path_rule(ruleset_fd: RawFd, path: &str, allowed_access: u64) -> Result<(), String> {
    let path_c = CString::new(path).map_err(|_| format!("sandbox path contains NUL: {path:?}"))?;
    let parent_fd = unsafe {
        libc::open(
            path_c.as_ptr(),
            libc::O_PATH | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if parent_fd < 0 {
        return Err(format!(
            "open fixed sandbox path {path}: {}",
            io::Error::last_os_error()
        ));
    }
    let attr = LandlockPathBeneathAttr {
        allowed_access,
        parent_fd,
    };
    let added = unsafe {
        libc::syscall(
            libc::SYS_landlock_add_rule,
            ruleset_fd,
            LANDLOCK_RULE_PATH_BENEATH,
            &attr,
            0u32,
        )
    };
    let error = io::Error::last_os_error();
    unsafe { libc::close(parent_fd) };
    if added != 0 {
        return Err(format!("add {SANDBOX_ID} path rule for {path}: {error}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_devpts_remains_usable_after_landlock() {
        // Landlock confinement is irreversible for the calling thread. Run the
        // real ruleset and PTY allocation in a child so the Rust test harness
        // remains unaffected, then require an exact clean exit.
        let child = unsafe { libc::fork() };
        assert!(child >= 0, "fork PTY sandbox test: {}", io::Error::last_os_error());
        if child == 0 {
            let exit_code = match install_filesystem_sandbox_with_rules(&[(
                "/dev/pts",
                DEVICE_WRITE_ACCESS,
            )]) {
                Ok(()) => {
                    let mut master = -1;
                    let mut slave = -1;
                    let opened = unsafe {
                        libc::openpty(
                            &mut master,
                            &mut slave,
                            std::ptr::null_mut(),
                            std::ptr::null(),
                            std::ptr::null(),
                        )
                    };
                    if opened == 0 {
                        unsafe {
                            libc::close(master);
                            libc::close(slave);
                        }
                        0
                    } else {
                        82
                    }
                }
                Err(_) => 81,
            };
            unsafe { libc::_exit(exit_code) };
        }

        let mut status = 0;
        loop {
            let waited = unsafe { libc::waitpid(child, &mut status, 0) };
            if waited == child {
                break;
            }
            assert_eq!(waited, -1, "waitpid returned an unexpected child");
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                panic!("wait for PTY sandbox test: {error}");
            }
        }
        assert!(libc::WIFEXITED(status), "PTY sandbox child status={status}");
        assert_eq!(
            libc::WEXITSTATUS(status),
            0,
            "PTY allocation failed after Landlock; child status={status}"
        );
    }

    #[test]
    fn session_turn_budget_records_are_read_exactly_and_never_defaulted() {
        for turns in [1u32, DEFAULT_MAX_SESSION_TURNS, MAX_SESSION_TURNS_CEILING] {
            let record = format!("{{\"max_session_turns\":{turns}}}\n");
            assert_eq!(
                parse_session_turn_budget(record.as_bytes()),
                Ok(turns),
                "canonical record for {turns} turns was not read back exactly"
            );
        }

        // Every rejection must fail the session. None of these may fall back
        // to the sealed default: the whole point of the record is that the
        // budget Qwen Code enforces is the budget the session was accepted
        // with, and a session graded on a bound nobody chose is worse than a
        // session that refuses to start.
        for malformed in [
            // Not the canonical spelling.
            "{\"max_session_turns\":400}".to_string(),
            "{\"max_session_turns\": 400}\n".to_string(),
            " {\"max_session_turns\":400}\n".to_string(),
            "{\"max_session_turns\":400}\n\n".to_string(),
            "{\"max_session_turns\":0400}\n".to_string(),
            "{\"max_session_turns\":+400}\n".to_string(),
            "{\"max_session_turns\":\"400\"}\n".to_string(),
            "{\"max_session_turns\":400,\"unrelated_field\":false}\n".to_string(),
            // A different sealed record must never be read as a budget.
            "{\"unrelated_record\":false}\n".to_string(),
            String::new(),
            // Outside the accepted range.
            "{\"max_session_turns\":0}\n".to_string(),
            "{\"max_session_turns\":-1}\n".to_string(),
            format!("{{\"max_session_turns\":{}}}\n", MAX_SESSION_TURNS_CEILING + 1),
            format!("{{\"max_session_turns\":{}}}\n", u64::from(u32::MAX) + 1),
        ] {
            let error = parse_session_turn_budget(malformed.as_bytes())
                .expect_err(&format!("malformed record was accepted: {malformed:?}"));
            assert!(
                error.contains(TURN_BUDGET_FILE) && error.contains("max_session_turns"),
                "refusal does not name the record it read: {error}"
            );
        }
    }

    #[test]
    fn fixed_command_and_sandbox_identity_are_exact() {
        assert_eq!(
            SANDBOX_ID,
            "landlock-fs-v4-write-roots-v1+private-devpts-rw-v1+output-unmounted-v1"
        );
        assert_eq!(NODE, "/usr/local/bin/node");
        assert_eq!(TURN_BUDGET_FILE, "/run/agent/turn-budget.json");
        assert_eq!(DEFAULT_MAX_SESSION_TURNS, 400);
        assert_eq!(MAX_SESSION_TURNS_CEILING, 2000);
        assert_eq!(CLI, "/opt/qwen-code/scripts/cli-entry.js");
        assert_eq!(CONTROL_ATTEST_FD, 3);
        assert_eq!(CONTROL_RELEASE_FD, 4);
        assert_eq!(DIRECTORY_WRITE_ACCESS, 0x7ff2);
        assert_eq!(FILE_WRITE_ACCESS, 0x4002);
        assert_eq!(DEVICE_WRITE_ACCESS, 0x2);
        assert_eq!(
            LandlockRulesetAttr {
                handled_access_fs: 0,
                handled_access_net: 0
            }
            .handled_access_net,
            0
        );
        assert_eq!(mem::size_of::<LandlockPathBeneathAttr>(), 12);
    }
}
