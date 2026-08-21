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
use std::os::unix::net::UnixStream;
use std::process::{Command, ExitCode};

const SANDBOX_ID: &str =
    "landlock-fs-v4-write-roots-v1+private-devpts-rw-v1+output-unmounted-v1";
const EVENTS_SOCKET: &str = "/streams/events.sock";
const STDERR_SOCKET: &str = "/streams/stderr.sock";
/// Session turn budget: the one bound on how long a session may work. Turns,
/// never wall time -- a wall-clock budget would measure backend generation speed
/// rather than agent progress. Qwen Code stops itself here and exits 53, an
/// ordinary terminal outcome. This value must equal `limits.max_session_turns`
/// in the stack lock, `execution.max_session_turns` in the agent runtime
/// contract, and `model.maxSessionTurns` in both sealed settings files; the
/// in-image runtime-contract verifier proves the contract and settings agree,
/// and the service proves the lock equals its own compiled constant.
const MAX_SESSION_TURNS: u32 = 400;
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
            .arg(format!("--max-session-turns={MAX_SESSION_TURNS}"))
            .arg("--max-tool-calls=-1")
            .arg("--no-chat-recording"),
    );
    Err(format!("exec pinned Qwen Code entrypoint: {error}"))
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
    fn fixed_command_and_sandbox_identity_are_exact() {
        assert_eq!(
            SANDBOX_ID,
            "landlock-fs-v4-write-roots-v1+private-devpts-rw-v1+output-unmounted-v1"
        );
        assert_eq!(NODE, "/usr/local/bin/node");
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
