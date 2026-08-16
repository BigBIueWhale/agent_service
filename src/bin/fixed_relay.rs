//! Fixed-purpose byte relays for the one Qwen3.8 deployment topology.
//!
//! There is deliberately no general address or path syntax. The only accepted
//! argument is one reviewed role, and every role resolves to a compile-time
//! loopback address plus `/sock/relay.sock`.

use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::{io, mem};

use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};

const SOCKET_PATH: &str = "/sock/relay.sock";
const SANDBOX_ID: &str = "landlock-net-v4+seccomp-socket-v2";

const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1;
const LANDLOCK_RULE_NET_PORT: u32 = 2;
const LANDLOCK_ACCESS_NET_BIND_TCP: u64 = 1 << 0;
const LANDLOCK_ACCESS_NET_CONNECT_TCP: u64 = 1 << 1;

const AUDIT_ARCH_X86_64: u32 = 0xc000_003e;
const SECCOMP_SET_MODE_FILTER: libc::c_uint = 1;
const SECCOMP_FILTER_FLAG_TSYNC: libc::c_ulong = 1;
const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
const SECCOMP_DATA_ARCH_OFFSET: u32 = 4;
const SECCOMP_DATA_NR_OFFSET: u32 = 0;
const SECCOMP_DATA_ARG0_OFFSET: u32 = 16;
const SECCOMP_DATA_ARG1_OFFSET: u32 = 24;
const BPF_LD_W_ABS: u16 = 0x20;
const BPF_JMP_JEQ_K: u16 = 0x15;
const BPF_ALU_AND_K: u16 = 0x54;
const BPF_RET_K: u16 = 0x06;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Direction {
    UnixListenToTcp,
    TcpListenToUnix,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Role {
    name: &'static str,
    direction: Direction,
    tcp: &'static str,
}

impl Role {
    fn tcp_port(self) -> Result<u16, String> {
        self.tcp
            .rsplit_once(':')
            .and_then(|(_, port)| port.parse::<u16>().ok())
            .ok_or_else(|| format!("compiled role {} has an invalid TCP address", self.name))
    }

    fn landlock_access(self) -> u64 {
        match self.direction {
            Direction::UnixListenToTcp => LANDLOCK_ACCESS_NET_CONNECT_TCP,
            Direction::TcpListenToUnix => LANDLOCK_ACCESS_NET_BIND_TCP,
        }
    }
}

impl Role {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "model-bridge" => Ok(Self {
                name: "model-bridge",
                direction: Direction::UnixListenToTcp,
                tcp: "127.0.0.1:8000",
            }),
            "model-ingress" => Ok(Self {
                name: "model-ingress",
                direction: Direction::TcpListenToUnix,
                tcp: "127.0.0.1:8000",
            }),
            "service-bridge" => Ok(Self {
                name: "service-bridge",
                direction: Direction::UnixListenToTcp,
                tcp: "127.0.0.1:8090",
            }),
            "service-ingress" => Ok(Self {
                name: "service-ingress",
                direction: Direction::TcpListenToUnix,
                tcp: "127.0.0.1:8090",
            }),
            "agent-model" => Ok(Self {
                name: "agent-model",
                direction: Direction::TcpListenToUnix,
                tcp: "127.0.0.1:18000",
            }),
            _ => Err(format!(
                "unsupported relay role {value:?}; expected exactly one of: \
                 model-bridge, model-ingress, service-bridge, service-ingress, agent-model"
            )),
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let arguments = std::env::args_os().collect::<Vec<_>>();
    if arguments.len() != 2 {
        eprintln!("fixed_relay: exactly one fixed role argument is required");
        return ExitCode::from(2);
    }
    let role_text = match arguments[1].to_str() {
        Some(value) => value,
        None => {
            eprintln!("fixed_relay: role is not valid UTF-8");
            return ExitCode::from(2);
        }
    };
    let role = match Role::parse(role_text) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("fixed_relay: {error}");
            return ExitCode::from(2);
        }
    };
    let result = match role.direction {
        Direction::UnixListenToTcp => serve_unix(role).await,
        Direction::TcpListenToUnix => serve_tcp(role).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fixed_relay: role={} failed: {error}", role.name);
            ExitCode::from(1)
        }
    }
}

fn validate_socket_parent(path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("socket path has no parent: {}", path.display()))?;
    let metadata = std::fs::symlink_metadata(parent)
        .map_err(|error| format!("cannot stat socket parent {}: {error}", parent.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "socket parent must be a real directory, observed {:?} at {}",
            metadata.file_type(),
            parent.display()
        ));
    }
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(metadata) => Err(format!(
            "refusing to replace pre-existing socket path {} of type {:?}",
            path.display(),
            metadata.file_type()
        )),
        Err(error) => Err(format!(
            "cannot inspect socket path {}: {error}",
            path.display()
        )),
    }
}

async fn serve_unix(role: Role) -> Result<(), String> {
    let shutdown_signals = install_termination_signals()?;
    let socket = PathBuf::from(SOCKET_PATH);
    validate_socket_parent(&socket)?;
    let listener = UnixListener::bind(&socket)
        .map_err(|error| format!("bind Unix listener {}: {error}", socket.display()))?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o660))
        .map_err(|error| format!("chmod 0660 {}: {error}", socket.display()))?;
    let metadata = std::fs::symlink_metadata(&socket)
        .map_err(|error| format!("stat bound Unix listener {}: {error}", socket.display()))?;
    if !metadata.file_type().is_socket() || metadata.permissions().mode() & 0o777 != 0o660 {
        return Err(format!(
            "bound Unix listener contract drift at {}: type={:?} mode={:o}",
            socket.display(),
            metadata.file_type(),
            metadata.permissions().mode() & 0o777
        ));
    }
    install_network_sandbox(role)?;
    prove_network_sandbox()?;
    println!(
        "RELAY_READY role={} sandbox={} listen=unix:{} target=tcp:{}",
        role.name, SANDBOX_ID, SOCKET_PATH, role.tcp
    );
    let shutdown = termination_signal(shutdown_signals);
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (incoming, _) = accepted.map_err(|error| format!("accept Unix connection: {error}"))?;
                tokio::spawn(relay_unix_to_tcp(incoming, role));
            }
            signal = &mut shutdown => {
                signal?;
                drop(listener);
                remove_owned_unix_socket(&socket)?;
                println!("RELAY_STOPPED role={} socket=removed", role.name);
                return Ok(());
            }
        }
    }
}

fn remove_owned_unix_socket(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("cannot stat owned Unix socket {}: {error}", path.display()))?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != 1000
        || metadata.gid() != 1000
        || metadata.permissions().mode() & 0o777 != 0o660
    {
        return Err(format!(
            "refusing to remove Unix socket with drift at {}: socket={} uid={} gid={} mode={:o}",
            path.display(),
            metadata.file_type().is_socket(),
            metadata.uid(),
            metadata.gid(),
            metadata.permissions().mode() & 0o777
        ));
    }
    std::fs::remove_file(path)
        .map_err(|error| format!("remove owned Unix socket {}: {error}", path.display()))
}

async fn serve_tcp(role: Role) -> Result<(), String> {
    let shutdown_signals = install_termination_signals()?;
    let listener = TcpListener::bind(role.tcp)
        .await
        .map_err(|error| format!("bind TCP listener {}: {error}", role.tcp))?;
    let observed = listener
        .local_addr()
        .map_err(|error| format!("inspect TCP listener {}: {error}", role.tcp))?;
    if observed.to_string() != role.tcp || !observed.ip().is_loopback() {
        return Err(format!(
            "kernel bound unexpected TCP address {observed}; required {}",
            role.tcp
        ));
    }
    install_network_sandbox(role)?;
    prove_network_sandbox()?;
    println!(
        "RELAY_READY role={} sandbox={} listen=tcp:{} target=unix:{}",
        role.name, SANDBOX_ID, role.tcp, SOCKET_PATH
    );
    let shutdown = termination_signal(shutdown_signals);
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (incoming, _) = accepted.map_err(|error| format!("accept TCP connection: {error}"))?;
                tokio::spawn(relay_tcp_to_unix(incoming, role));
            }
            signal = &mut shutdown => {
                signal?;
                return Ok(());
            }
        }
    }
}

#[repr(C)]
struct LandlockRulesetAttr {
    handled_access_fs: u64,
    handled_access_net: u64,
}

#[repr(C)]
struct LandlockNetPortAttr {
    allowed_access: u64,
    port: u64,
}

fn install_network_sandbox(role: Role) -> Result<(), String> {
    // This deployment is pinned to linux/amd64. Refusing another architecture
    // is safer than loading a filter whose audit-architecture constant or
    // syscall layout might have different semantics.
    if std::env::consts::ARCH != "x86_64" || std::env::consts::OS != "linux" {
        return Err(format!(
            "{SANDBOX_ID} supports exactly linux/x86_64, observed {}/{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
    }
    install_landlock_port_policy(role)?;
    install_socket_domain_seccomp(role)?;
    install_no_new_bind_seccomp()?;
    Ok(())
}

fn install_landlock_port_policy(role: Role) -> Result<(), String> {
    // SAFETY: These are the stable Linux Landlock UAPI structures and syscall
    // argument shapes from linux/landlock.h. Every pointer refers to a live,
    // correctly sized repr(C) value for the duration of its call.
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
            "{SANDBOX_ID} requires Landlock ABI >=4 network rules, observed ABI {abi}"
        ));
    }

    let ruleset_attr = LandlockRulesetAttr {
        handled_access_fs: 0,
        handled_access_net: LANDLOCK_ACCESS_NET_BIND_TCP | LANDLOCK_ACCESS_NET_CONNECT_TCP,
    };
    // SAFETY: See the UAPI safety statement above.
    let ruleset_fd = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            &ruleset_attr,
            mem::size_of::<LandlockRulesetAttr>(),
            0u32,
        )
    };
    if ruleset_fd < 0 {
        return Err(format!(
            "create Landlock network ruleset for {}: {}",
            role.name,
            io::Error::last_os_error()
        ));
    }
    let ruleset_fd = ruleset_fd as libc::c_int;
    let port_attr = LandlockNetPortAttr {
        allowed_access: role.landlock_access(),
        port: u64::from(role.tcp_port()?),
    };
    // SAFETY: The ruleset descriptor is owned by this function and port_attr
    // has the exact UAPI representation required for LANDLOCK_RULE_NET_PORT.
    let add_result = unsafe {
        libc::syscall(
            libc::SYS_landlock_add_rule,
            ruleset_fd,
            LANDLOCK_RULE_NET_PORT,
            &port_attr,
            0u32,
        )
    };
    if add_result != 0 {
        let error = io::Error::last_os_error();
        // SAFETY: ruleset_fd is a valid descriptor opened above.
        unsafe { libc::close(ruleset_fd) };
        return Err(format!(
            "add fixed Landlock TCP-port rule for {}: {error}",
            role.name
        ));
    }
    // Landlock requires no_new_privs for unprivileged self-restriction. Docker
    // already sets it; repeating it here makes the relay's own invariant
    // independent of an optimistic assumption about the launcher.
    // SAFETY: prctl is called with the documented PR_SET_NO_NEW_PRIVS shape.
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        let error = io::Error::last_os_error();
        // SAFETY: ruleset_fd is a valid descriptor opened above.
        unsafe { libc::close(ruleset_fd) };
        return Err(format!("set no_new_privs before Landlock: {error}"));
    }
    // SAFETY: ruleset_fd is a live Landlock ruleset and flags must be zero.
    let restrict_result =
        unsafe { libc::syscall(libc::SYS_landlock_restrict_self, ruleset_fd, 0u32) };
    // SAFETY: ruleset_fd is no longer needed after restrict_self returns.
    unsafe { libc::close(ruleset_fd) };
    if restrict_result != 0 {
        return Err(format!(
            "enter fixed Landlock network domain for {}: {}",
            role.name,
            io::Error::last_os_error()
        ));
    }
    Ok(())
}

const fn bpf_stmt(code: u16, value: u32) -> libc::sock_filter {
    libc::sock_filter {
        code,
        jt: 0,
        jf: 0,
        k: value,
    }
}

const fn bpf_jump(code: u16, value: u32, jt: u8, jf: u8) -> libc::sock_filter {
    libc::sock_filter {
        code,
        jt,
        jf,
        k: value,
    }
}

fn install_socket_domain_seccomp(role: Role) -> Result<(), String> {
    // Non-socket syscalls continue through Docker's already-active built-in
    // seccomp profile. For socket(2), this stacked filter permits only stream
    // Unix and IPv4 sockets. Landlock independently constrains every TCP bind
    // and connect to the role's one fixed port/direction.
    // TCP-listening roles bind their one fixed listener before sandbox
    // installation and thereafter need only new Unix sockets. Refusing every
    // later AF_INET socket makes their loopback-only binding mechanical even
    // after code compromise. Unix-listening bridge roles need AF_INET stream
    // sockets for their fixed outbound connection in a network-none
    // namespace, so Landlock separately limits those connects to the one port.
    let mut filter = match role.direction {
        Direction::UnixListenToTcp => vec![
            bpf_stmt(BPF_LD_W_ABS, SECCOMP_DATA_ARCH_OFFSET),
            bpf_jump(BPF_JMP_JEQ_K, AUDIT_ARCH_X86_64, 1, 0),
            bpf_stmt(BPF_RET_K, SECCOMP_RET_KILL_PROCESS),
            bpf_stmt(BPF_LD_W_ABS, SECCOMP_DATA_NR_OFFSET),
            bpf_jump(BPF_JMP_JEQ_K, libc::SYS_socket as u32, 0, 8),
            bpf_stmt(BPF_LD_W_ABS, SECCOMP_DATA_ARG0_OFFSET),
            bpf_jump(BPF_JMP_JEQ_K, libc::AF_UNIX as u32, 2, 0),
            bpf_jump(BPF_JMP_JEQ_K, libc::AF_INET as u32, 1, 0),
            bpf_stmt(BPF_RET_K, SECCOMP_RET_ERRNO | libc::EPERM as u32),
            bpf_stmt(BPF_LD_W_ABS, SECCOMP_DATA_ARG1_OFFSET),
            bpf_stmt(BPF_ALU_AND_K, 0x0f),
            bpf_jump(BPF_JMP_JEQ_K, libc::SOCK_STREAM as u32, 1, 0),
            bpf_stmt(BPF_RET_K, SECCOMP_RET_ERRNO | libc::EPERM as u32),
            bpf_stmt(BPF_RET_K, SECCOMP_RET_ALLOW),
        ],
        Direction::TcpListenToUnix => vec![
            bpf_stmt(BPF_LD_W_ABS, SECCOMP_DATA_ARCH_OFFSET),
            bpf_jump(BPF_JMP_JEQ_K, AUDIT_ARCH_X86_64, 1, 0),
            bpf_stmt(BPF_RET_K, SECCOMP_RET_KILL_PROCESS),
            bpf_stmt(BPF_LD_W_ABS, SECCOMP_DATA_NR_OFFSET),
            bpf_jump(BPF_JMP_JEQ_K, libc::SYS_socket as u32, 0, 7),
            bpf_stmt(BPF_LD_W_ABS, SECCOMP_DATA_ARG0_OFFSET),
            bpf_jump(BPF_JMP_JEQ_K, libc::AF_UNIX as u32, 1, 0),
            bpf_stmt(BPF_RET_K, SECCOMP_RET_ERRNO | libc::EPERM as u32),
            bpf_stmt(BPF_LD_W_ABS, SECCOMP_DATA_ARG1_OFFSET),
            bpf_stmt(BPF_ALU_AND_K, 0x0f),
            bpf_jump(BPF_JMP_JEQ_K, libc::SOCK_STREAM as u32, 1, 0),
            bpf_stmt(BPF_RET_K, SECCOMP_RET_ERRNO | libc::EPERM as u32),
            bpf_stmt(BPF_RET_K, SECCOMP_RET_ALLOW),
        ],
    };
    install_seccomp_filter(&mut filter, "socket-domain")
}

fn install_no_new_bind_seccomp() -> Result<(), String> {
    // All required listeners—Unix or loopback TCP—already exist at this
    // point. No relay role legitimately binds another socket. This second
    // stacked policy closes the remaining address-level gap that Landlock's
    // port-only network ABI cannot express.
    let mut filter = [
        bpf_stmt(BPF_LD_W_ABS, SECCOMP_DATA_ARCH_OFFSET),
        bpf_jump(BPF_JMP_JEQ_K, AUDIT_ARCH_X86_64, 1, 0),
        bpf_stmt(BPF_RET_K, SECCOMP_RET_KILL_PROCESS),
        bpf_stmt(BPF_LD_W_ABS, SECCOMP_DATA_NR_OFFSET),
        bpf_jump(BPF_JMP_JEQ_K, libc::SYS_bind as u32, 0, 1),
        bpf_stmt(BPF_RET_K, SECCOMP_RET_ERRNO | libc::EPERM as u32),
        bpf_stmt(BPF_RET_K, SECCOMP_RET_ALLOW),
    ];
    install_seccomp_filter(&mut filter, "no-new-bind")
}

fn install_seccomp_filter(filter: &mut [libc::sock_filter], label: &str) -> Result<(), String> {
    let program = libc::sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_mut_ptr(),
    };
    // SAFETY: program points to a live classic-BPF array for the call. TSYNC
    // makes the rule fail closed if any current thread cannot be synchronized.
    let result = unsafe {
        libc::syscall(
            libc::SYS_seccomp,
            SECCOMP_SET_MODE_FILTER,
            SECCOMP_FILTER_FLAG_TSYNC,
            &program,
        )
    };
    if result != 0 {
        return Err(format!(
            "install stacked {label} seccomp filter: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn prove_network_sandbox() -> Result<(), String> {
    fn require_denied<T>(label: &str, result: io::Result<T>) -> Result<(), String> {
        match result {
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => Ok(()),
            Err(error) => Err(format!(
                "{SANDBOX_ID} negative probe {label} failed for the wrong reason: {error}"
            )),
            Ok(_) => Err(format!(
                "{SANDBOX_ID} negative probe {label} unexpectedly succeeded"
            )),
        }
    }

    // A denied arbitrary TCP connect proves the Landlock connect policy; a
    // denied arbitrary bind proves its bind policy. A denied UDP socket proves
    // that the stacked socket-domain seccomp filter is active. Finally, a Unix
    // stream pair must remain usable because each relay needs Unix transport.
    require_denied(
        "arbitrary TCP connect",
        std::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, 1)),
    )?;
    require_denied(
        "arbitrary TCP bind",
        std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)),
    )?;
    require_denied(
        "UDP socket creation",
        std::net::UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0)),
    )?;
    let (left, right) = std::os::unix::net::UnixStream::pair()
        .map_err(|error| format!("{SANDBOX_ID} blocked required Unix streams: {error}"))?;
    drop((left, right));
    Ok(())
}

async fn relay_unix_to_tcp(mut incoming: UnixStream, role: Role) {
    match TcpStream::connect(role.tcp).await {
        Ok(mut outgoing) => {
            if let Err(error) = copy_bidirectional(&mut incoming, &mut outgoing).await {
                eprintln!(
                    "fixed_relay: role={} connection copy failed: {error}",
                    role.name
                );
            }
        }
        Err(error) => {
            eprintln!(
                "fixed_relay: role={} fixed TCP target {} unavailable: {error}",
                role.name, role.tcp
            );
        }
    }
}

async fn relay_tcp_to_unix(mut incoming: TcpStream, role: Role) {
    match UnixStream::connect(SOCKET_PATH).await {
        Ok(mut outgoing) => {
            if let Err(error) = copy_bidirectional(&mut incoming, &mut outgoing).await {
                eprintln!(
                    "fixed_relay: role={} connection copy failed: {error}",
                    role.name
                );
            }
        }
        Err(error) => {
            eprintln!(
                "fixed_relay: role={} fixed Unix target {} unavailable: {error}",
                role.name, SOCKET_PATH
            );
        }
    }
}

struct TerminationSignals {
    terminate: tokio::signal::unix::Signal,
    interrupt: tokio::signal::unix::Signal,
}

fn install_termination_signals() -> Result<TerminationSignals, String> {
    let terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|error| format!("install SIGTERM handler: {error}"))?;
    let interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|error| format!("install SIGINT handler: {error}"))?;
    Ok(TerminationSignals {
        terminate,
        interrupt,
    })
}

async fn termination_signal(mut signals: TerminationSignals) -> Result<(), String> {
    tokio::select! {
        value = signals.terminate.recv() => value.ok_or_else(|| "SIGTERM stream closed".to_string()),
        value = signals.interrupt.recv() => value.ok_or_else(|| "SIGINT stream closed".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{Direction, Role, SANDBOX_ID};

    #[test]
    fn exact_roles_have_fixed_directions_and_addresses() {
        let cases = [
            ("model-bridge", Direction::UnixListenToTcp, "127.0.0.1:8000"),
            (
                "model-ingress",
                Direction::TcpListenToUnix,
                "127.0.0.1:8000",
            ),
            (
                "service-bridge",
                Direction::UnixListenToTcp,
                "127.0.0.1:8090",
            ),
            (
                "service-ingress",
                Direction::TcpListenToUnix,
                "127.0.0.1:8090",
            ),
            ("agent-model", Direction::TcpListenToUnix, "127.0.0.1:18000"),
        ];
        for (name, direction, tcp) in cases {
            let role = Role::parse(name).expect("reviewed role must parse");
            assert_eq!(role.name, name);
            assert_eq!(role.direction, direction);
            assert_eq!(role.tcp, tcp);
        }
    }

    #[test]
    fn arbitrary_role_is_rejected() {
        let error = Role::parse("tcp-listen:0.0.0.0:22").expect_err("dynamic relay is forbidden");
        assert!(error.contains("unsupported relay role"));
    }

    #[test]
    fn sandbox_identity_and_port_policy_are_fixed_per_role() {
        assert_eq!(SANDBOX_ID, "landlock-net-v4+seccomp-socket-v2");
        for (name, port, access) in [
            ("model-bridge", 8000, super::LANDLOCK_ACCESS_NET_CONNECT_TCP),
            ("model-ingress", 8000, super::LANDLOCK_ACCESS_NET_BIND_TCP),
            (
                "service-bridge",
                8090,
                super::LANDLOCK_ACCESS_NET_CONNECT_TCP,
            ),
            ("service-ingress", 8090, super::LANDLOCK_ACCESS_NET_BIND_TCP),
            ("agent-model", 18000, super::LANDLOCK_ACCESS_NET_BIND_TCP),
        ] {
            let role = Role::parse(name).expect("reviewed role");
            assert_eq!(role.tcp_port().expect("compiled port"), port);
            assert_eq!(role.landlock_access(), access);
        }
    }
}
