The workspace root contains `.task-env.tar.gz`: this repository's build
toolchain and dependency caches for offline use. To use it:

    mkdir -p /tmp/task-env
    tar -xzf .task-env.tar.gz -C /tmp/task-env
    . /tmp/task-env/env.sh

You may `rm .task-env.tar.gz` after extracting. Where repository
documents describe a development environment, the extracted one is the
one that applies here. Leave changes as uncommitted working-tree
modifications.

---
