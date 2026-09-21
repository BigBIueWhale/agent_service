# QWEN38_DEPLOYMENT_CONTRACT_V1

## Container and filesystem

- `/workspace` is a read-write staged copy of the submitted folder. The complete
  final staged tree is bundled.
- `/artifacts` starts empty and is for deliberate durable reports, exports,
  diagrams, or other requested deliverables.
- `/tmp` is bounded writable scratch. Use it for derived document pages, archive
  extraction, databases, compiler probes, indexes, media conversion, and other
  transient computation. Scratch is not automatically bundled.
- `/output` is not mounted in this container. A separate fixed, trusted capture
  component is its sole mount owner and durably records this process's stdout
  stream-JSON and stderr through one-use Unix sockets under the read-only
  `/streams` mount. Do not probe, reconnect, replace, or use those sockets as
  task storage. Readiness, exit status, final response, bundles, and terminal
  state are produced by trusted components outside Qwen and its descendants.
- The agent runs as uid:gid 1000:1000 with a read-only container root. It has no
  GPU, Docker socket, host Qwen/Claude/Codex state, or writeable operator source.
- Docker supplies the agent's isolated PID namespace and devpts instance. The
  Landlock policy grants `/dev/pts` only `WRITE_FILE`, solely so the native
  `run_shell_command` implementation can allocate and use its pseudoterminal.

## Network and dependencies

- The agent has `--network none`: loopback only, no default route, DNS, Internet,
  LAN, package registry, remote Git host, cloud API, or host network namespace.
- The only model path is the already-validated agent-local
  `http://127.0.0.1:18000/v1`. That address is inside the agent namespace, not
  host loopback. Do not probe, reconfigure, or replace it.
- Runtime dependency installation and remote fetch are unavailable. Do not retry
  `apt`, `pip`, `npm`, Cargo, Go, Maven, remote `git`, `curl`, `wget`, or another
  network operation after the boundary is established.
- Use only the immutable offline toolchain. Its capability categories are:
  Node.js and Python; Go, Rust, and Java; GCC and Clang; CMake, Ninja, and
  pkg-config; Git and Git LFS; ripgrep and fd; GDB, strace, and ShellCheck; jq and
  yq; SQLite and the PostgreSQL client; Pandoc, Poppler, and QPDF; ImageMagick,
  FFmpeg, and Graphviz; tar, zip, unzip, xz, zstd, bzip2, and rsync; plus the
  pinned Ubuntu shell/editor utilities.
- Categories are promises validated by the image. When an exact binary, codec,
  feature, or version matters, verify it locally. A missing promised capability
  is a deployment-contract failure, not permission to install a replacement.

## Structured tools and foreground subagents

- Explore is investigative in purpose, not mechanically read-only. It may render
  or extract PDFs, unpack archives, build local probes, create databases/indexes,
  convert media, write scratch, create explicit artifacts, and modify staged
  workspace files when the investigation genuinely requires it.
- Explore workspace/artifact state is content-hashed before and after the child.
  Its trusted result metadata lists changes and names the exact hashed manifest.
  Journal failure makes the tool call fail; changes are never silently reverted.

## Full-quality vision and document work

- At most fifteen images may exist in one rendered
  request. Video and audio are disabled.
- Accepted RGBA is server-composited onto pinned white.
- Tool-result text/image/text remains in the originating tool message and exact
  chronological position. Never clump an old image into the newest turn.
- PDF handling is local computation, not direct PDF vision. Poppler/QPDF/Pandoc/
  ImageMagick may extract or deliberately render pages into scratch. A derived
  image enters `read_file` only after it satisfies the exact PNG contract. A
  failed extraction/conversion is reported; it never triggers an online or
  silent lossy fallback.
- Compaction removes old raw pixels with the summarized history rather than
  relocating them to a false recent turn.

## Failure and completion semantics

- Scratch quota exhaustion, journal failure, or cleanup failure is explicit.
  Never redirect silently into project, artifact, host, or network state.
