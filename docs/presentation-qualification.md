# Presentation qualification

Typhon keeps direct scanout disabled by default. Set
`OBLIVION_ONE_DIRECT_SCANOUT=experimental-auto` only for an explicitly labeled
qualification run; `auto` is retained as a compatibility alias and is not a
production default.

The reproducible matrix tool is:

```bash
bin/qualify-presentation --dry-run
```

It prints the sequential matrix across direct policy (`off`,
`experimental-auto`), triple buffering (`off`, `auto`, `force`), and cursor
scheduling (`auto`, `piggyback`, `software`). Every combination has a distinct
phase label. No result is considered a qualification until it has been run on
a real TTY with the same hardware and driver.

For a live run, provide the command that owns one session:

```bash
OBLIVION_ONE_QUALIFY_COMMAND="$PWD/bin/start-oblivion-one-tty" \
  bin/qualify-presentation
```

Each phase writes bounded, labeled artifacts under
`~/.local/state/oblivion-one/qualifications/<timestamp>/`, including the
session log, trace placeholder, metrics placeholder, environment snapshot,
and summary. The summary reports trace drops when the running compositor emits
that metric. The tool does not enable VRR or tearing and does not change the
default direct policy.
