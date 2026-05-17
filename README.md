# Rust Edit-and-Continue

This repository is a coordination checkout for the Rust edit-and-continue
prototype. It keeps the active forks and debugger/editor pieces together as Git
submodules, with the design notes in [edit_and_continue.md](edit_and_continue.md).

## Why

Most Rust debugging today is *cargo build, run, hit the bug, change one line,
rebuild for forty seconds, run, hit a slightly different version of the bug,
change another line*. The compile loop costs you the state you spent ten
minutes setting up.

The other gap: gdb and lldb don't speak Rust natively. They show `Box<dyn
Error>` as an opaque pair of pointers; they print
`alloc::collections::btree::map::BTreeMap` instead of `BTreeMap`; they don't
know what a Tokio task is.

This project is two things glued together to fix both:

* **A Rust-first debugger** (the BugStalker fork) that knows the language —
  vtables, niches, futures, smart pointers, the whole stdlib.
* **An edit-and-continue pipeline** that patches running processes when you
  save a file, so the *next* call hits your fix without losing the state
  you'd already built up.

You debug at the rate you can think rather than the rate cargo can rebuild.

## What the debugger gives you today

(Beyond the obvious — breakpoints, stepping, watchpoints, multi-threaded
support, DAP for VSCode and any DAP client.)

* **`dyn Trait` rendered as a typed record.** `Box<dyn Error + Send + Sync>`
  shows the concrete pointee inline, the vtable's drop / size / align, and
  every method grouped by its declaring trait with the dispatch target's
  source location:

  ```text
  Box<dyn Error, _> [→ ShowcaseErr] [+ Send + Sync] {
    data: 0x5555… → ShowcaseErr("boom"),
    vtable: 0x5555… {
      drop: <no Drop impl>,
      size: 16, align: 8,
      Debug:   { fmt: → <ShowcaseErr as Debug>::fmt (main.rs:125) },
      Display: { fmt: → <ShowcaseErr as Display>::fmt (main.rs:128) },
      Error:   { source: → Error::source (error.rs:105), … },
    },
  }
  ```

  Pin, depth-aware collapse for nested dyn, and method truncation for the
  `dyn Iterator` case are all handled.

* **Niche-aware enum decoding.** `Option<NonZeroU32>`, `Option<&T>`,
  `Result<bool, char>` — every niche path rustc uses, including the
  fn-pointer one.

* **Tokio await-trace.** Per-task chain of suspended `async fn`s with the
  recovered source coordinates of each `.await`. Works without
  `tokio_console` instrumentation — the oracle reads the runtime directly.

* **Smart pointers and cycles.** `Box`, `Rc`, `Arc`, `Weak` with strong/weak
  counters; depth-bounded rendering and cycle detection so `Rc<Self>` graphs
  don't loop forever in the renderer.

* **Time-travel debugging.** Three independent tiers — reverse stepping
  over a recorded trace, fork-based checkpoint replay, and clean-room
  deterministic record-and-replay. See
  [Time-travel debugging](#time-travel-debugging) below.

* **`#[derive(DebugView)]`.** Annotate your types with a summary template
  (`Person({name}, age {age})`) and the debugger shows that instead of the
  generic struct dump.

* **JSON-RPC scripting front-end.** `bs --script` exposes every command as a
  typed JSON-RPC method so agents and IDE plugins can drive the debugger
  without scraping a TTY. `bs --test` turns a `.json5` script of assertions
  into a TAP-emitting test runner; `--bless` re-pins expectations after an
  intentional output change.

* **Pure-Rust toolchain.** No LLDB libraries, no Python, no per-platform
  adapter binaries to download.

## Time-travel debugging

Three independent tiers, in increasing cost and durability. Pick the
one that matches the bug you're hunting.

### Tier 1 — Reverse step over a recorded trace (`bs-replay-driver`)

Loads a Tier 3 trace and walks it backward. The debugger sees the
same `ptrace`-event surface it would on a live process, so
breakpoints, watchpoints, and step semantics work as usual — they
just navigate recorded history rather than driving execution.

* REPL: `rstep` (`rs`) steps the playhead back one event,
  `rstep-fwd` (`rsf`) steps it forward, `rcontinue` (`rc`) rewinds
  to the previous breakpoint hit.
* DAP: `stepBack`, `reverseContinue`, plus custom requests
  `bs/replayLoad`, `bs/replayJump`, `bs/replayTimeline`,
  `bs/replayCheckpointList`. The session advertises
  `supportsStepBack` and `supportsReverseContinue` once a trace is
  loaded.
* `replay-doctor` validates a trace, prints per-event-kind
  histograms, dumps the first N events in readable form, and diffs
  two traces for the first divergence.

### Tier 2 — Fork-based checkpoint replay (`bs-replay`)

Periodically `fork(2)`s the debuggee at safe points; the children
sleep, copy-on-write. To inspect state at an earlier PC, attach to
the nearest checkpoint, set a one-shot breakpoint at the target,
and let it run forward. Up to 32 checkpoints in a ring; oldest gets
`SIGKILL` when full.

Linux uses native `fork` + `PTRACE_SEIZE`. macOS uses `mach_vm_remap`
of writable regions because Mach doesn't expose `fork`-with-ptrace
cleanly. Best-effort across I/O — fds diverge once either side reads
or writes — but fine for UI work and pure-compute bugs.

### Tier 3 — Deterministic record-and-replay (`bs-replay-engine`)

Clean-room rr-style record-and-replay. No GPL dependency, no
external `rr` binary, no Python. Linux x86-64 today; aarch64 is
scoped, multi-threaded MVP uses single-CPU serialisation.

The trick to near-native record speed is **never single-step the
program**. We intercept only at non-determinism boundaries:

| Source                    | Mechanism                                  |
| ------------------------- | ------------------------------------------ |
| Syscalls                  | `seccomp-bpf` user-notify                  |
| `RDTSC` / `RDTSCP`        | `PR_SET_TSC = SIGSEGV`, trap, record       |
| `RDRAND` / `RDSEED`       | `CPUID` mask + `#UD` trap fallback         |
| `CPUID`                   | seccomp emulate or trap-and-virtualise     |
| vDSO time fast paths      | vDSO entry-point patching at startup       |
| `io_uring` SQ/CQ pages    | `userfaultfd` page-fault interception      |
| Thread interleaving       | single-CPU pinning; PT instruction counts  |
| Initial env / args / cwd  | full snapshot at record start              |

Targeted overhead: ~1.5–2× single-threaded, ~2× multi-threaded once
PT-assisted instruction counts replace software single-step between
context switches. Trace on disk is a directory: `manifest.toml` +
RFC-8478 zstd-compressed event segments + periodic full snapshots
for seek without full rescan.

### Live macOS step-back (no trace required)

The DAP session ring-buffers the last 32 stops on macOS — Mach
writable-region snapshot + per-thread register dump — and rewinds
the focused thread on `stepBack`. This is on by default for macOS
debug sessions; no `replay-record` needed. Linux-side equivalent
goes through Tier 1/2/3.

### Binaries and entry points

| Tool | Purpose | Platform |
| --- | --- | --- |
| `replay-record TRACE_DIR -- PROGRAM ...` | Tier 3 recorder | Linux x86-64 |
| `replay-load TRACE_DIR -- PROGRAM ...` | Tier 3 replay shim (no debugger) | Linux x86-64 |
| `replay-doctor TRACE_DIR` | Validate / summarise / diff a trace | any |
| `bs --dap` + `bs/replayLoad` request | Attach BugStalker to a recorded trace | any |
| REPL `rstep` / `rstep-fwd` / `rcontinue` | Tier 1 reverse stepping at the prompt | any |

`replay-doctor` flags include `--diff OTHER`, `--counts`,
`--dump-events N`, and `--check-host`. Run `replay-doctor --help` for
the full list.

> **Not the same thing:** `bs --record FILE` records a *JSON-RPC
> debugging session* as a runnable `.json5` test script — handy for
> pinning behaviour after a refactor, unrelated to time travel. The
> time-travel recorder is `replay-record`.

## Edit-and-continue (in progress)

The AOT pipeline (cg_clif → wild → BugStalker patch-apply) lands working
demos on aarch64 macOS today; the Linux runtime demo is the remaining gap.
The JIT pipeline (cg_clif + `cranelift-hotswap`) has a working warm
re-codegen path. See [edit_and_continue.md](edit_and_continue.md) for the
component-by-component status.

The VSCode extension's edit-and-continue watcher (file-change → rebuild via
wild + `--emit-patch` → DAP custom request → process-patched, top frame
auto-restarted when the change landed in the paused function) is the
user-facing front of all of this; the
[VSCode extension](#vscode-extension--edit-and-continue) section below
documents the launch-config knobs.

## Repositories

| Path | Repository | Purpose | Branch |
| --- | --- | --- | --- |
| `rustc_codegen_cranelift` | `git@github.com:gilescope/rustc_codegen_cranelift.git` | cg_clif edit-and-continue JIT integration | `main` |
| `wasmtime` | `git@github.com:gilescope/wasmtime.git` | `cranelift-hotswap` implementation | `giles-cranelift-hotswap` |
| `linker` | `https://github.com/gilescope/wild` | Wild linker AOT patch emission | `giles-mac` |
| `bugstalker` | `https://github.com/gilescope/BugStalker` | Debugger patch application and reverse/debugger work | `giles-rust-visuals` |
| `vscode-extension` | `https://github.com/gilescope/vscode-lldb.git` | VSCode debug adapter — DAP shim + edit-and-continue watcher | pinned commit |

`enc-patcher` was an early same-process patching spike and is intentionally not
included as a submodule here.

## VSCode extension — edit-and-continue

The `vscode-extension` fork is the user-facing front of the AOT pipeline. Two
modes:

* **One-shot:** `BugStalker: Apply Patch` command — pick a `.patch` file, the
  extension sends it to BugStalker via the `bs/encApplyPatch` DAP custom request
  and prints the apply summary.
* **Watcher (default-on for cargo launches):** edit a `.rs` file → debounce →
  rebuild via `cargo build` with the wild linker + `--emit-patch` → apply the
  emitted patch in the paused process. When the patch landed in the
  currently-paused function, the top frame is auto-restarted so the next step
  hits the new code.

Each apply reports: entries applied, bytes written, entries skipped for
*drift* (process bytes no longer match wild's "old bytes" — usually a build
mismatch), and entries skipped because the target page is read-only. Drift
entries are surfaced with `(offset, expected, actual, symbol)` per byte so the
cause is debuggable.

Special-cased: rust-analyzer's "Debug" code-lens launches the binary out of
`/tmp/ra/debug/...` while the watcher rebuilds into the project's target dir.
The extension redirects the launch at the project-target binary so the running
process and the watcher's rebuilds share an output dir and produce
byte-identical artifacts.

Launch-config knobs (set in `launch.json` per debug configuration):

| Setting | Default | Purpose |
| --- | --- | --- |
| `editContinue` | `true` | Opt out per-launch when the project isn't ready for EnC. |
| `editContinueCommand` | derived | Rebuild command that must produce a wild patch. |
| `editContinuePatchPath` | `${cwd}/target/.../*.patch` | Where the rebuild drops the patch. |
| `editContinueLinker` | discovered | Path to the `wild` binary. |
| `editContinueTarget` | host triple | Cargo target triple for the EnC rebuild. |
| `editContinueWatch` | `[ "**/*.rs" ]` | Globs the watcher reacts to. |
| `editContinueCwd` | launch `cwd` | Working directory for the rebuild command. |
| `editContinueDebounceMs` | `150` | Debounce window for file-change events. |
| `editContinueBase` | first text symbol | Base for the wild patch's offset interpretation. |

Platform support is symmetric on Linux (x86_64, aarch64) and macOS
(aarch64, x86_64) — the extension picks the right default target triple per
host, and BugStalker's patch-application path uses the appropriate
ptrace / `task_for_pid` flow underneath. Windows is not in scope today.

The extension is otherwise a thin DAP shim: it spawns `bs --dap`, registers
the `bugstalker` debug-adapter type, and stays out of the way. No LLDB
libraries, no Python, no per-platform adapter binaries to download.

## Checkout

Clone with submodules:

```sh
git clone --recurse-submodules git@github.com:gilescope/rec.git
```

For an existing checkout:

```sh
git submodule update --init --recursive
```

To move branch-tracking submodules to the latest configured branch heads:

```sh
git submodule update --remote --merge
```
