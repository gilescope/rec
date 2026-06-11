<!-- markdownlint-disable -->
# rec — BugStalker debugger workspace

Umbrella repo wiring together the `bs` Rust debugger and its VS Code client.
Submodules:

- `bugstalker/` — the `bs` debug adapter (DAP) + core debugger. Ships the perf
  overlay (`--features perf`, **on by default**). Installed extension id:
  `vadimcn.vscode-lldb-0.1.0`.
- `vscode-extension/` — the VS Code client (packaged to a `.vsix`).
- `linker/` — linker work.

Feature/handoff notes live in `debug-step-costs.md` (perf overlay / step costs).

## Install on macOS (the one true way)

```sh
cd ~/git/gilescope/rec
earthly +install-darwin      # then Cmd-R / "Reload Window" in VS Code
```

`+install-darwin` (see `Earthfile`) builds from the **LOCAL submodule checkouts**
— uncommitted/unpushed work lands in the editor. It:

1. `cargo install` `bs` (perf overlay default) + **adhoc codesign with the
   `com.apple.security.cs.debugger` entitlement** (delegated to
   `bugstalker+install-darwin`). The entitlement is what lets the perf
   PC-sampler call `task_for_pid`.
2. Packages the extension to `vscode-extension/build/vscode-bugstalker.vsix`
   (containerised — **needs Docker running**) and installs it via the `code`
   CLI with `--force`.

Then **reload the VS Code window** (Cmd-R) — nothing is live until you do.

Prereqs (all `LOCALLY`): macOS, host `cargo`, `codesign`, and the `code` CLI on
PATH (Command Palette → "Shell Command: Install 'code' command in PATH").

`cargo build`/`cargo test` alone do **not** install anything — they don't touch
`~/.cargo/bin/bs` (what VS Code launches) or the `.vsix`.

## Build / test (bugstalker)

```sh
cd bugstalker
cargo clippy --features perf --tests          # clippy ⊇ check
cargo test --features perf --test debugger perf_cost   # perf/step-cost suite
cargo test -p bs-perf --lib trap_floor        # TrapFloor unit tests
```

Functional debugger tests live in `tests/debugger/`; they run real debuggees
from `examples/` (build the examples first: `cargo build --manifest-path
examples/Cargo.toml`). DAP integration tests are in `tests/dap/`.

## Instruction-level stepping in VS Code (native)

Not a keybinding — it's view-driven. Pause, then **Open Disassembly View**
(`editor.debug.action.openDisassemblyView`, or right-click a Call Stack frame).
While that view is focused, the normal step keys (F10/F11) send
`granularity: "instruction"` and step one machine instruction; focus a source
editor to go back to line stepping. Requires the adapter's
`supportsDisassembleRequest` + `supportsSteppingGranularity` (both advertised).
