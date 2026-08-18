You are fixing a real software issue in the repository at your working
directory. Operational facts about this session, all of which you should
rely on:

- **Time budget.** This session is force-cancelled 3000 seconds (50
  minutes) after it starts. The `date` command works; note the start
  time, check the clock as you go, and budget your work. You must stop
  and write your final answer — a normal message with no tool call —
  comfortably before the limit. Work that ends without a final answer
  and an uncommunicated fix score exactly the same as no work.
- **Grading.** After the session, an automated grader runs the
  repository's own test suite (separately, with its own toolchain)
  against your working tree. Currently-passing tests encode required
  behavior: a change that breaks them fails grading even if your new
  behavior is right. Your edits to existing test files may be reset
  before grading, and new test files you create are not graded — put
  the behavior change in non-test source files, and treat the existing
  tests as the specification when the issue text is ambiguous.
- **Environment.** There is no network access. The task's build
  toolchain and dependency caches are provided in the workspace root as
  `.task-env.tar.gz`. Before building or running tests, extract it
  outside the workspace and load its environment:

      mkdir -p /tmp/task-env
      tar -xzf .task-env.tar.gz -C /tmp/task-env
      . /tmp/task-env/env.sh

  After extracting you may `rm .task-env.tar.gz` so your diff stays
  clean. The environment provides the repository's expected tools
  (Java/Maven, Gradle, Go, Python, Node — whichever apply) configured
  to use the bundled offline caches. Prefer actually compiling and
  running the relevant tests over reasoning about them; an executed
  check beats an argued one.
- **Workspace.** Leave your changes as uncommitted working-tree
  modifications. Do not create commits.

The issue to fix follows.

---
