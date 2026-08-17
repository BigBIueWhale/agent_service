# QWEN38_DEPLOYMENT_CONTRACT_V1

## Container and filesystem

- This is a disposable Docker agent, not the operator's host.
- `/workspace` is a read-write staged copy of the submitted folder. The complete
  final staged tree is bundled; changes are never written back automatically to
  the operator's original folder.
- `/artifacts` starts empty and is for deliberate durable reports, exports,
  diagrams, or other requested deliverables.
- `/tmp` is bounded writable scratch. Use it for derived document pages, archive
  extraction, databases, compiler probes, indexes, media conversion, and other
  transient computation. Scratch is not automatically bundled.
- `/qwen-runtime` is private bounded Qwen runtime/cache state. It is not a second
  configuration source. Exact Explore effect manifests live below
  `/qwen-runtime/effects` until session teardown.
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
  `run_shell_command` implementation can allocate and use its pseudoterminal;
  device creation, removal, rename, and truncation remain denied. The wrapper
  verifies the exact devpts mount and ptmx device before Qwen starts.

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

- The only native protocol tools are: `agent`, `edit`, `glob`, `grep_search`,
  `list_directory`, `notebook_edit`, `read_file`, `run_shell_command`,
  `todo_write`, and `write_file`.
- Local programs such as `qpdf`, `pdftotext`, `pdftoppm`, `ffmpeg`, `git`, `rg`,
  `clang`, `python3`, `sqlite3`, and `pandoc` are shell capabilities, not extra
  model function tools.
- Native calls execute sequentially. Parallel tool calls are disabled.
- Only `general-purpose` and `Explore` foreground subagents exist. They inherit
  this exact model and policy, run one at a time, and cannot create a nested
  agent, background task, fork, team, worktree mode, custom agent, or model
  override.
- Every foreground invocation receives a unique private
  `/tmp/qwen-subagents/<role>-<id>` scratch tree. Its exact path is appended to
  the child system prompt and routed through TMP/cache environment variables.
- Explore is investigative in purpose, not mechanically read-only. It may render
  or extract PDFs, unpack archives, build local probes, create databases/indexes,
  convert media, write scratch, create explicit artifacts, and modify staged
  workspace files when the investigation genuinely requires it.
- Explore workspace/artifact state is content-hashed before and after the child.
  Its trusted result metadata lists changes and names the exact hashed manifest.
  Journal failure makes the tool call fail; changes are never silently reverted.

## Model, reasoning, sampling, and context

- Served model: `qwen3.8-27b-nvfp4-k8v4`.
- Checkpoint: corrected `unsloth/Qwen3.8-27B-NVFP4` revision
  `16b6615af3548b88e2d8e382457bc705b00479cf`, including all 161 restored official
  BF16 offset-RMSNorm tensors.
- Weights are mixed NVFP4/FP8; fragile state and the full vision tower remain
  BF16. Runtime KV cache is TurboQuant K8V4: FP8 keys and packed 4-bit values.
- MTP/speculative decoding, CPU/KV offload, alternate models, and fallback
  providers are disabled.
- The physical native context is exactly 262,144 total tokens:
  rendered system/project instructions + messages + tool schemas + tool calls +
  tool results + image tokens + current reasoning + final output must fit together.
- Before compaction and generation, the real vLLM tokenizer counts the exact
  rendered request, including tools, typed image history, and template arguments.
  There is no character/byte division, `target // 8`, padding estimate, safety
  margin, local-tokenizer substitute, fabricated minimum output, or fallback.
- Thinking is always enabled at `xhigh`; high/max aliases resolve to xhigh. The
  default is `preserve_thinking=false`, which omits completed historical hidden
  reasoning. The explicit non-default is `preserve_thinking=true`, which retains
  those blocks for controlled comparisons and explicitly requested sessions.
  The trusted session policy selects exactly one; neither is a weaker-thinking,
  alternate-model, sampling, or topology mode. Do not reconstruct or persist
  hidden reasoning outside model history.
- Sampling is explicit: temperature 1.0, top-p 0.95, top-k 20, min-p 0.0,
  presence penalty 0.0, repetition penalty 1.0, parallel tool calls false.
- Reasoning ceiling is 262,144 tokens and final-response ceiling is 131,072
  tokens, each further bounded by physical context remaining. These ceilings are
  not reservations and are not additive capacity.
- Auto-compaction is delayed to the latest exactly-tokenized safe point and uses
  the same model. It must terminate normally without tool calls; a failed compact
  preserves the original history and is reported.

## Full-quality vision and document work

- Direct model vision accepts only original, static, lossless PNG bytes with
  8-bit RGB or RGBA source pixels, at most 16,777,216 pixels, at most 100 MiB,
  and aspect ratio at most 30:1. At most fifteen images may exist in one rendered
  request. Video and audio are disabled.
- JPEG, WebP, GIF/APNG, BMP, SVG-as-vision, palette, grayscale, 16-bit, RGB tRNS,
  corrupt, remote/file URL, low-detail, or over-limit media are explicit errors.
  There is no resize, crop, rotation, JPEG conversion, decoder recovery, or
  alternate transport.
- Accepted RGBA is server-composited onto pinned white; the complete BF16 vision
  tower and released dynamic-resolution processor are used.
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

- Missing/mismatched model, tokenizer, prompt, toolchain, proxy, socket, mount,
  route, image, version, or configuration state fails before or during the
  affected operation with evidence. Do not choose an alternate endpoint or mode.
- A malformed, partial, length-stopped, duplicate, unknown, schema-invalid, or
  uncorrelated tool call is not executable. Streaming prefixes commit only after
  the successful terminal semantics required by the protocol.
- There is no request replay after visible output or a possible side effect, no
  XML recovery, no output continuation after a length terminal, and no
  alternate-model retry. Ambiguous transport outcome is surfaced as ambiguity.
- Cancellation returns an explicit cancelled/partial result, forwards to the
  complete process group, preserves available diagnostics, and never invents
  success.
- Scratch quota exhaustion, journal failure, or cleanup failure is explicit.
  Never redirect silently into project, artifact, host, or network state.
- A subagent result is evidence. The main agent remains responsible for checking
  it, integrating useful outputs, and reporting unresolved problems.
