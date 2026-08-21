You are working in a sealed offline container. There is **no network access
of any kind** — no package installs, no registry or index updates, no
downloads. Everything you need is already on disk. If a command appears to
hang trying to reach the network, stop and use the offline caches described
below instead.

## Your workspace and what gets graded

`/workspace` is a checkout of the repository at the task's base commit.
Your work is graded by taking the **complete diff of `/workspace`** against
that base commit and applying it to a fresh checkout. Therefore:

- Leave your changes as uncommitted working-tree modifications. Do not commit.
- **Anything you create inside `/workspace` becomes part of your submitted
  patch.** Never unpack archives, install toolchains, create virtualenvs, or
  write scratch files inside `/workspace`. Use `/tmp` for all scratch space.

## The offline build environment: `.task-env.tar.gz`

The workspace root contains `.task-env.tar.gz`: this repository's build
toolchain and dependency caches, captured from the same image the grader
uses. Extract it **outside the workspace** and source its environment:

    mkdir -p /tmp/task-env
    tar -xzf /workspace/.task-env.tar.gz -C /tmp/task-env
    . /tmp/task-env/env.sh

Extract to `/tmp/task-env` exactly as shown — never into `/workspace`, and
never by running `tar -xzf .task-env.tar.gz` without `-C`, which would unpack
it into the workspace and pollute your patch. You may `rm
/workspace/.task-env.tar.gz` after extracting; that removal is ignored by
grading.

`env.sh` exports the paths for whichever ecosystems this repository uses —
`JAVA_HOME`, `GOROOT`/`GOPATH`/`GOMODCACHE`, `M2_HOME`/Maven local repo,
`GRADLE_USER_HOME`, `CARGO_HOME`/`RUSTUP_HOME`, `PYTHONHOME`, `NODE_PATH` —
and prepends their `bin` directories to `PATH`. Source it in **every** shell
you run builds or tests in; a shell that has not sourced it may resolve a
different compiler version than the grader uses.

Where repository documents describe a development environment, the extracted
one is the one that applies here.

## Tools available in the container

Base toolchains (the extracted task environment may override these with the
exact versions this repository pins — prefer the ones `env.sh` puts on `PATH`):

    node 22.23.2        python3 3.12.3 (pytest 7.4.4)   go 1.25.13
    rustc/cargo 1.75.0  java/javac 21.0.11 (maven 3.8.7) gcc 13.3.0
    clang 18.1.3        cmake 3.28.3 / ninja             git 2.43.0 (+git-lfs)

Also installed: `rg` (ripgrep), `fd`, `jq`, `yq`, `sqlite3`, `psql`,
`shellcheck`, `gdb`, `strace`, `rsync`, `curl`, `tar`, `zip`/`unzip`, `xz`,
`bzip2`, `zstd`, `pkg-config`, `pandoc`, `ffmpeg`, `convert` (ImageMagick),
`dot` (Graphviz), `pdftotext`/`pdftoppm`, `qpdf`.

Nothing else can be installed. If a tool you want is absent, solve the task
with what is listed here.

## Approach

Establish a working build-and-test loop early — extract the task
environment, source `env.sh`, and run the repository's existing tests once —
before making substantial changes, so you can tell whether an edit helps.
Prefer running the specific tests relevant to the task over the full suite,
which may be slow. Submit a real fix: diagnostic logging, instrumentation, or
disabled tests do not resolve the task.

---
