# Edit-and-Continue for Rust — Design Synthesis

> Status: in progress. **AOT pipeline mechanically complete on aarch64 macOS.** Linux runtime demo is the remaining gap.
> Started: 2026-05-04. Last updated: 2026-05-22 (JIT path dropped — see §0 below).
> Spans repos: `wild`, `BugStalker`, the VSCode debugger extension. `enc-patcher` was an early same-process spike and is not part of this repo bundle.

## 0. Direction change — 2026-05-22

This project originally pursued **two pipelines in parallel**: a JIT path (cg_clif + a new `cranelift-hotswap` crate) and an AOT path (cargo + wild + BugStalker patch-apply). The JIT path was mechanically complete (17 unit tests, 6.95M-call multi-thread stress, warm same-MIR re-codegen verified) but was **dropped on 2026-05-22** because:

- The AOT path subsumes the same user-facing flow (edit → save → next call hits new code) without needing a JIT.
- Carrying the JIT path meant maintaining forks of two large upstream projects (`wasmtime` and `rustc_codegen_cranelift`) plus a 3,025-line new crate, indefinitely.
- The cg_clif fork existed *only* to wire `--enc-jit`; once JIT is gone the fork has zero non-upstream commits and goes away too.

What survived: AOT pipeline status, BugStalker patch-apply primitives, wild incremental tiers 1-4, the active-frame remap design (now applied to AOT patches), and the smaller forks of `wild` + `BugStalker` + `vscode-lldb`.

The JIT design lives on as a reference branch: `giles-cranelift-hotswap` on `gilescope/wasmtime`, PR #1 closed 2026-05-22.

## What's working today

### AOT path (cargo + wild + BugStalker apply-patch)

- ✅ **Wild emits patch files (`--emit-patch=<path>`, v3 format)** after each incremental link, listing every byte run that differs from the previous link with both old and new bytes inline. Verified: `compute(x)=x+1` → `compute(x)=x+100` produces a 2-byte run at the changed `add` immediate plus a 32-byte run for the codesign signature.
- ✅ **Function offsets stable across edits** thanks to wild's tier-4 4 KiB ALLOC padding and subsections-via-symbols. Verified: `_compute` at `0x100001000` in both v1 and v2 of a tiny C demo.
- ✅ **BugStalker `apply-patch <path> [<hex-base>]` and `watch-patch <path>`** consume wild-emitted patches. Word-aligned read-modify-write via existing `Debugger::write_memory`. 15 unit tests covering format edge cases (v1, v2, v3, multi-entry, malformed input rejection, v3 hash endpoint validation).
- ✅ **Auto-detect `__TEXT` base** from BugStalker's existing dwarf-registry mappings: `apply-patch /tmp/p.patch` (no base required) translates each entry's file offset to a runtime address itself. Linux: `load_base + offset`; macOS: `slide + 0x100000000 + offset`.
- ✅ **`watch-patch <path>`** polls every 250 ms; re-applies on each modification. Blocks the REPL until SIGINT (documented limitation).
- ✅ **Drift detection (v2/v3 wild-patch format).** Each entry carries pre-image bytes inline; BugStalker reads the running process's bytes at the target and compares before writing. On mismatch: skip + diagnostic showing expected vs actual hex, so the user sees "running process is already on a later image" immediately rather than silently corrupting.
- ✅ **Patch metadata + endpoint hash guard (v3 wild-patch format).** Wild now emits whole-output old/new blake3 headers and `# fn: <symbol>` comments for changed runs that map to text symbols. BugStalker parses v1/v2/v3, attaches `# fn:` to the following entry, includes the function name in apply/drift diagnostics, and refuses v3 patches when the debugee executable path hashes to neither the old nor new endpoint. This catches wholesale wrong-binary cases before any writes; the live-process guard remains the per-entry pre-image check.
- ✅ **Historical `enc-patcher` spike** (`~/git/gilescope/enc-patcher`): same-process `patch_self(addr, &[u8])` helper using `mprotect(RW)→memcpy→mprotect(RX)→icache flush`. Linux works without privileges; Apple Silicon is blocked by kernel TEXT enforcement without `com.apple.security.cs.allow-jit`. It is not included as a submodule here because BugStalker's external-process patching subsumes it for the EnC use case.

## What's blocked / pending

- ❌ **Linux runtime test**: NixOS x86 box at 192.168.1.137 / .129 not network-reachable from current machine (different subnet). Pipeline is code-complete; the demo is one Linux box away.
- ⏳ **Conflict detection / frame restart on patch overlap.** When a patch entry overlaps the currently-executing PC of any thread, the program would crash if patched naively. Three approaches discussed:
  - (3) Detect + warn: walk threads, find frames in affected fns, surface a diagnostic. ~2 hours, next concrete step.
  - (1) Frame restart without rr: capture args from DWARF formal-parameter locations at fn-entry, reset IP+SP to fn entry, continue. Function logic re-runs; side effects already done are kept. Bounded ~week.
  - (2) Frame restart with rr: checkpoint at fn entry, replay forward with patched code. True rewind. Multi-month (rr-style record/replay is its own project).
- ⏳ **True live mapped-image hash validation.** BugStalker now fail-fast hashes the debugee executable path against the v3 old/new endpoints before patching. A stricter future guard would hash the mapped executable bytes from the live process itself, which matters if the path has been replaced, remapped, or otherwise diverges from the process image.
- ⏳ **`watch-patch` non-blocking variant.** Current implementation blocks BugStalker's REPL; needs main-event-loop integration to allow other commands while watching. Bounded but BugStalker-architectural.

---

## 1. Goal & non-goals

**Goal.** A debugger-driven edit-and-continue (EnC) experience for Rust, modelled on Visual Basic 6 / Visual Studio C++ EnC and on .NET Hot Reload's protocol — *not* on Subsecond / hot-lib-reloader.

User flow: hit a breakpoint → notice a bug → edit one function in your editor → save → debugger silently recompiles that function and patches the running process → continue execution.

**Non-goals.**

- *Application-cooperative* hot reload (Subsecond/Dioxus). The user does not annotate call sites with `subsecond::call(|| ...)`; EnC works on unmodified code.
- *Time-travel / record-replay* (rr-style). Orthogonal, larger; covered separately by the BugStalker replay tiers. Pharo-style "restart frame" is included in §3 as a cheap surrogate for "step back."
- *Type/layout changes mid-flight*. Refused in v0; supervised migration (Erlang `code_change/3`-style) is a far-future possibility.
- *Optimised builds*. v0 forces `-O0 -Cinline-threshold=0`. Inlining across a swap boundary is a fundamental obstacle; we sidestep by disabling it.
- *Windows*. Deferred. Different process-control primitives (`OpenProcess` / `WriteProcessMemory` / `CreateRemoteThread`), different debug protocol — own project.
- *JIT-mode EnC*. Dropped 2026-05-22 (see §0). The AOT pipeline covers the same use case without needing a custom JIT.

**Platform scope.** Linux **and** macOS are both first-class targets from v0. Architectures: x86-64 and aarch64 (M-series Macs make aarch64 unavoidable from day one).

---

## 2. Architecture

```text
┌────────────────────────────────────────────────────────────────────────┐
│  Editor (VSCode + DAP, or any DAP client)                              │
│   ↕ DAP                                                                 │
│  ┌───────────────────────────────────────────┐                         │
│  │  BugStalker (debugger)                     │                         │
│  │   • ptrace seize / cont / step (existing)  │                         │
│  │   • /proc/PID/mem write or macOS Mach      │  ←── pub APIs:          │
│  │     vm_write for in-place .text patch       │      apply-patch        │
│  │   • drift detection: per-entry pre-image    │      watch-patch        │
│  │     check before write                      │      bs/encApplyPatch   │
│  │   • blake3 endpoint hash guard on v3        │      (DAP)              │
│  └────────┬──────────────────────────────────┘                         │
│           │ file-change → rebuild → patch                                │
│           ↕                                                              │
│  ┌───────────────────────────────────────────┐                         │
│  │  cargo + wild --emit-patch=<path>          │                         │
│  │   • any rustc backend (cg_clif or LLVM)    │                         │
│  │   • wild does incremental link (tier 1-4)  │                         │
│  │   • emits byte-diff v3 patch with pre-image│                         │
│  │     bytes and blake3 endpoint hashes        │                         │
│  └────────┬──────────────────────────────────┘                         │
│           │ patch file                                                   │
│           ↕                                                              │
│  ┌───────────────────────────────────────────┐                         │
│  │  Debuggee (your Rust app, AOT-built)       │                         │
│  │   running normally                          │                         │
│  └───────────────────────────────────────────┘                         │
└────────────────────────────────────────────────────────────────────────┘
```

The VSCode extension's edit-and-continue watcher orchestrates this: on `.rs` save → debounce → `cargo build` with wild + `--emit-patch` → DAP `bs/encApplyPatch` → BugStalker writes the byte runs into the live process. When the patch landed in the currently-paused function, the top frame can be auto-restarted so the next step hits the new code.

---

## 3. Findings (condensed from research dispatches)

> §3.1/§3.2 (cg_clif JIT current reality, `cranelift-hotswap` crate design) were removed when the JIT path was dropped on 2026-05-22. See §0 and the `giles-cranelift-hotswap` branch on `gilescope/wasmtime` if you want the historical content.

### 3.3 BugStalker — existing primitives ready to reuse

Surveyed `~/git/gilescope/BugStalker`. The Linux primitives we need are mostly built:

| Primitive | Location | Status |
|---|---|---|
| ptrace seize / interrupt / waitpid | `src/debugger/process.rs:147-166` | ready |
| ASLR disable at fork (`personality(ADDR_NO_RANDOMIZE)`) | `src/debugger/process.rs:280-288` | ready |
| `read_memory_by_pid` (PTRACE_PEEKDATA loop) | `src/debugger/mod.rs:1598-1616` | ready |
| `Debugger::write_memory` (PTRACE_POKEDATA loop) | `src/debugger/mod.rs:1180-1190` | ready |
| `CallHelper::mmap` — syscall-inject `mmap(NULL, size, PROT_RWX, ANON, ...)` | `src/debugger/call/mod.rs:384-455` | ready, `pub(super)` |
| `Debugger::call(fn_name, args)` — full register-shuffle + INT3 trap call | `src/debugger/call/mod.rs:886-931` | ready, `pub` |
| Oracle trait + transparent breakpoints with `Fn(&mut Debugger)` callback | `src/oracle/mod.rs`, `src/debugger/breakpoint.rs:29-63` | ready |
| DAP custom request `bs/*` namespace | `src/dap/yadap/session/mod.rs:630-667` | hard-coded; unknown commands return error |

### 3.4 Linux process-patching playbook (recommended sequence)

For the paused-tracee EnC patch (in-place `.text` write, not the trampoline+mmap approach the JIT path needed):

1. **Verify pre-image bytes** — wild's v3 patch carries the old bytes inline; BugStalker reads the target address and refuses to write on mismatch.
2. **Bulk-write new code** — `pwrite64` to `/proc/PID/mem` (Linux) / `mach_vm_write` (macOS). Process is fully ptrace-stopped, so the write is atomic w.r.t. other threads.
3. **Endpoint-hash sanity check** — v3 patch carries old/new blake3 hashes of the whole output; BugStalker refuses if the debuggee executable hashes to neither.

W^X considerations: in-place `.text` patching needs `mprotect(RW)` (Linux) or the equivalent Mach VM permission flip (macOS) before the write, then restore RX after. Hardened-runtime macOS doesn't allow RWX but does allow the temporary flip via `vm_protect` while debugger-attached.

### 3.5 wild — incremental status (much further than README claims)

Surveyed `~/git/gilescope/wild`. Tier 1-4 incremental linking is *shipped*:

| Tier | What | Saving |
|---|---|---|
| 1 | Parse-skip cache (per-input symbol-table mmap-cache) | ~50 ms on rust-analyzer (211 of 229 inputs reused) |
| 1.5 | Single-bundle collapse (one `.wild-pi-cache` file) | ~25 ms on bevy-dylib (1649 inputs) |
| 2 | Layout snapshot (per-section file_offset/size/contributors) | foundation for tier 3 |
| 3 phase 1-2 | Per-section dirty bitmap + canary | foundation |
| 3 phase 2b | Speculative whole-output writer-skip | ~280 ms on bevy-dylib |
| 3 phase 3 | Partial writer-skip (pre-fill reusable, re-emit dirty only) | varies |
| **4** | **4 KiB ALLOC section padding** (commit 4a064bf, Giles Cope, 2026-04-26) | "function slot doesn't move" — the EnC-critical piece |
| daemon | `wild --serve <sock>` warm linker socket | ~30-40 ms per incremental link |

**Gap for AOT EnC.** Section-granularity, not function-granularity. Need `.text` partitioned into per-fn subsections (rustc supports this with COMDAT). Wild already supports COMDAT; the question is whether the section count (tens of thousands per large binary) overloads the layout snapshot — engineering, not fundamental.

### 3.6 Subsecond — adjacent, not the model

Subsecond's mechanism (concrete):

1. **ThinLink** wraps the system linker, caches per-crate `.rcgu.o` files, diffs against prior cache to identify changed fns.
2. Re-links *only changed object files* (plus their cascade closure) into a tiny patch dylib, with unchanged-fn references resolved against runtime addresses of the live process.
3. Runtime: `dlopen` the patch dylib, fill `JumpTable: HashMap<u64,u64>` with old→new addresses.
4. User wraps callsites in `subsecond::call(|| f(...))`. Inside, `HotFn` consults the table; on stale-caller detection emits `HotFnPanic`, unwinds to nearest outer `call` boundary, retries.

Achievement: 130-500 ms turnaround on M4-class hardware. Tip-crate-only initially (workspace work landed late in 0.7).

**Why we differ.** Subsecond never writes to `.text`; we do. Subsecond requires app cooperation; we don't. Subsecond's swap point is application-defined; ours is debugger-defined (any breakpoint). Subsecond's audience is "I'm running my Dioxus app and want to tweak a component"; ours is "I'm in a debugger session, hit a bug, want to fix and step over again."

### 3.7 Prior-art lessons worth stealing

- **.NET `OnFunctionRemapOpportunity` + `ICorDebugILFrame2::RemapFunction` callback.** Runtime fires "frame needs remap" event; compiler/IDE provides (old IL offset → new IL offset) translation. Apply: emit `(MIR basic-block id → machine offset)` map; BugStalker uses it for active-frame IP translation. Refuse where no equivalent exists.
- **GDB `compile code` / `libcc1` plugin boundary.** Debugger receives an ELF blob from compiler; knows nothing else. Same shape for our cargo → wild patch → BugStalker interface.
- **Erlang `code_change/3`.** When struct layout changes, demand explicit user-supplied migration. Don't silently reinterpret memory.
- **Live++ out-of-process agent.** Heavy tooling outside target; minimal in-target footprint. Limits blast radius.
- **VS C++ "stale code" warning.** Recursive / on-stack patching silently mixes old and new code paths. Surface explicitly. Don't pretend it's safe.
- **Pharo's "restart frame" UX.** Drop & re-enter current call frame. Our cheapest surrogate for "step back" — pairs naturally with EnC.
- **Muratori's state externalisation + input-replay** (Handmade Hero). Game code owns no state; record memory + input stream. Combined with EnC: deterministic re-run after edit. Far-future ambition.

### 3.8 rustc incremental — fast enough for v0

- Pre-backend granularity: per-function via the query DAG.
- Backend granularity: **per-CGU**. Even with one fn changed, the entire CGU re-emits. Default `-Ccodegen-units=256`; raising further has linker-overhead cost but no fundamental block.
- No rustc daemon RFC exists. `rustc_interface` is the unstable library API; each call is a fresh session (~50-200 ms startup).
- Subsecond achieves 130-500 ms via stock cargo + rustc incremental + ThinLink. cg_clif's faster codegen pulls this lower if you use it as the backend (still as a stock rustc component — no fork needed).
- Sub-100 ms blocked by: (a) CGU granularity; (b) session startup. Out of scope for v0.

**Realistic v0 target: 130-350 ms turnaround.**

---

## 4. Phase 1 — AOT MVP (current focus)

**Scope.** Single fn body changed, single thread, fn not currently on stack, no signature/layout/static changes. AOT mode. Demo: bug-in-`compute`.

### 4.1 Pipeline

1. **Build the debuggee** with `cargo build` using wild as the linker. `wild --emit-patch=<path>` is enabled per-launch via `editContinueCommand` in `launch.json`.
2. **Run the debuggee under BugStalker** (`bs --dap` from the VSCode extension or `bs <binary>` from a shell). Hit a breakpoint.
3. **Edit a `.rs` file** in the IDE. The VSCode extension's watcher debounces (150 ms) and triggers `cargo build`, which produces a fresh patch.
4. **BugStalker applies the patch** via `bs/encApplyPatch` (or interactive `apply-patch <path>` at the REPL). Pre-image bytes are verified per-entry; endpoint blake3 is checked against the executable path.
5. **Continue execution.** Next call to the edited function hits the new bytes.

### 4.2 Out of scope for v0

- Recursion / on-stack patching → refuse with explicit warning.
- Multi-threaded patching → ptrace-stops-all (Linux) / `task_suspend` (macOS) gives safety; v0 ships that.
- Type / layout / static / signature changes → refuse.
- Inlined call sites → `-Cinline-threshold=0` blocks them.
- Windows.

### 4.3 Platform-specific notes

| Concern | Linux | macOS |
|---|---|---|
| Process control | `ptrace` (BugStalker has it) | `task_for_pid` + Mach exception ports + `thread_suspend` (BugStalker macOS support is the long pole — see §4.4) |
| `.text` write | `/proc/<pid>/mem` `pwrite64` after `mprotect(RW)` | `mach_vm_write` after `vm_protect(VM_PROT_READ\|VM_PROT_WRITE\|VM_PROT_COPY)` |
| ASLR disable | `personality(ADDR_NO_RANDOMIZE)` (BugStalker uses) | `POSIX_SPAWN_DISABLE_ASLR_NP` (only on debuggee spawn) |
| W^X permanently enforced? | LSM-dependent (SELinux/AppArmor) | Yes on Apple Silicon hardened runtime — debugger-attached `vm_protect` works; standalone process patching its own `.text` does not |

### 4.4 BugStalker macOS gap

BugStalker today is primarily Linux. macOS support is partial (debugger-attached `vm_protect` + `mach_vm_write` work; ptrace-equivalent process-control surfaces are in progress). Mach `task_for_pid` + exception ports + `thread_get_state`/`thread_set_state` are the remaining bring-up — multi-week work coordinated with godzie44 upstream.

---

## 5. Phase 2 — Multi-fn + active-frame remap

**Scope.** Function may be currently on a thread's stack. Multiple fns per edit (closure of MIR change set). Static-initialiser detection.

**Key new piece.** `.NET RemapFunction` protocol applied to Rust:

- Emit `(MIR block id → machine offset)` map alongside each compiled fn (rustc DWARF or a sidecar; the wild patch can ride it as `# fn` metadata extended with block IDs).
- For each ptrace-stopped thread, BugStalker walks the call stack. For frames whose fn was patched:
  - Look up the current PC in the old fn's machine→MIR-block map.
  - Look up that MIR block id's offset in the *new* fn's map.
  - Hijack the thread's IP to the new offset.
  - If no equivalent block exists (block deleted, control flow restructured), refuse the swap on that frame and surface a "stale code in frame N" warning to the user.

Pharo "restart frame" UX: explicit user action to drop & re-enter the current fn from the top with snapshotted args. Cheap, satisfying, useful even without active-frame remap.

Static-initialiser detection: hash MIR of `MonoItem::Static` initialisers; if changed and layout unchanged, re-run init; if layout changed, refuse the edit.

---

## 6. Phase 3 — UX/IDE & far-future

- VSCode DAP extension that triggers EnC on save while debugger is paused. **Shipped (default-on for cargo launches).**
- Visual indicator: "this fn was edited but is currently on the stack — drop frame to apply?"
- Snapshot+replay mode (Muratori-inspired) — record register state before edit, replay after. Synergy with BugStalker's Tier 2/3 record-replay.
- rr/Tier-3 integration: "replay to bug, edit, continue forward" is the genuinely killer combination — both pieces are in BugStalker already; the join is the engineering task.
- Optional `code_change` callback (Erlang-inspired) for user-supervised struct migration.
- Windows port.

---

## 7. Open architectural questions

1. **Inlining policy.**
   - **No per-fn attribute.** The user wants EnC to work on *any* function at *any* breakpoint without source annotation — that's the VB6 ergonomic baseline. No `#[hot_swappable]` opt-in.
   - Compile-mode flag instead: the *whole crate* (or whole compilation) builds with EnC mode, which forces `-Cinline-threshold=0`, no MIR-level inlining, debug info on. Like VS's `/ZI`.
   - Release mode is unchanged; EnC only kicks in when you're running under the EnC build.
   - **Recommendation: binary mode (whole-build switch) for v0. Never per-fn annotation.**

2. **Type / layout changes.**
   - Refuse in v0 with clear error.
   - Phase 2: detect via dep-graph + layout hash; surface as a build-time error.
   - Phase 3+: optional `code_change` migration callback.

3. **Three-repo coordination.**
   - `wild` is your domain. `BugStalker` is godzie44's. `vscode-lldb` is the IDE shim. Coordinate API extensions with upstream early to avoid carrying forks forever.

---

## 8. Risks and mitigations

| Risk | Mitigation |
|---|---|
| BugStalker upstream rejects API surface extension | Carry a small public-API patch on a fork; pursue re-upstreaming in parallel. |
| `/proc/PID/mem` writes fail under hardened LSM (SELinux) | Document. Fall back to POKEDATA loop. |
| W^X enforcement refuses RWX permission flip | Use debugger-attached `vm_protect` (macOS) or `mprotect(RW)→write→mprotect(RX)` (Linux). |
| macOS hardened runtime forbids RWX on the debuggee's own pages | Patching is debugger-driven; `task_for_pid` with proper entitlements allows the temporary `vm_protect` flip. End-users running `bs` need the right entitlement on the debugger binary. |
| BugStalker has no full macOS port | Partial macOS support today; full `task_for_pid` + exception ports is the long pole. Coordinated with upstream. |
| Subsecond gets there first / takes the audience | Different problem. Cooperative app hot-reload vs debugger-driven EnC. Both can exist. |
| Recursive / on-stack patching mixes old/new code silently | Explicit "stale code" warning UX. v0 refuses entirely. |
| Inlined copies of patched fn keep running old code | `-Cinline-threshold=0` in EnC mode. v0 only. |
| wild section-count blow-up with per-fn `.text` partition | Engineering (layout-snapshot perf), not fundamental. Worst case: limit per-fn partition to the EnC crate(s). |

---

## 9. Smallest first commit (status)

Originally a single PR-shaped change in BugStalker exposing `mmap_rwx_in_target`. That landed and the project has moved well past it; the current "smallest unblocking commit" is the Linux runtime test (blocked on network access to the NixOS box, not on code).

The natural sequence that produced the current working pipeline:

1. ✅ `Debugger::write_memory` extension for bulk byte runs.
2. ✅ DAP custom request `bs/encApplyPatch`.
3. ✅ wild `--emit-patch=<path>` v1 → v2 (pre-image) → v3 (endpoint blake3 + `# fn` metadata).
4. ✅ BugStalker `apply-patch` and `watch-patch` commands with drift detection.
5. ✅ VSCode extension watcher (default-on, debounced rebuild + apply).
6. ⏳ Linux runtime test on the NixOS box.
7. ⏳ Conflict detection / frame restart on patch overlap.

---

## 10. Glossary

- **EnC** — edit-and-continue. The ability to modify code mid-debug-session and continue from the same paused state.
- **CGU** — codegen unit. rustc's compilation parallelism unit; the granularity at which the backend re-emits code under incremental.
- **MIR** — rustc's mid-level intermediate representation. Per-fn, basic-block-structured, post-monomorphisation.
- **DAP** — Debug Adapter Protocol. Microsoft's editor↔debugger protocol. BugStalker speaks it.
- **ptrace** — Linux process tracing syscall. BugStalker's primary control mechanism.
- **Trampoline** — a small stub of code that redirects execution from one address to another. Used by the (now-dropped) JIT path; the AOT pipeline patches `.text` in place and does not install trampolines.
- **Drift** — the live process's bytes at a patch target don't match the patch's expected pre-image. Indicates the running process is already on a different image than the one wild diffed against. BugStalker reports per-entry drift and refuses to write.

---

## 11. References

### Local repo file paths

- `~/git/gilescope/BugStalker/src/debugger/call/mod.rs:384-455` — `CallHelper::mmap`
- `~/git/gilescope/BugStalker/src/debugger/call/mod.rs:886-931` — `Debugger::call`
- `~/git/gilescope/BugStalker/src/debugger/process.rs:147-166` — ptrace seize
- `~/git/gilescope/BugStalker/src/debugger/mod.rs:1180-1190` — `write_memory`
- `~/git/gilescope/BugStalker/src/oracle/mod.rs` — Oracle trait
- `~/git/gilescope/BugStalker/src/dap/yadap/session/mod.rs:630-667` — DAP custom request dispatch
- `~/git/gilescope/BugStalker/src/ui/command/apply_patch.rs` — `apply-patch` / `watch-patch` REPL implementation
- `~/git/gilescope/wild/INCREMENTAL.md` — incremental linking guide
- `~/git/gilescope/wild/libwild/src/incremental_cache.rs` — `LinkCache`, `InputHash`
- `~/git/gilescope/wild/libwild/src/layout_snapshot.rs` — `LayoutSnapshot`
- `~/git/gilescope/wild/libwild/src/tier3_skip.rs` — partial writer-skip state
- `~/git/gilescope/wild/libwild/src/daemon.rs` — `wild --serve`
- `~/git/gilescope/wild/libwild/src/lib.rs` — v3 `--emit-patch` writer with old/new blake3 + `# fn` comments
- wild commit `4a064bf` (2026-04-26) — tier-4 4 KiB ALLOC padding (Giles Cope)
- wild commit `7823430` — tier-3 mmap-COW pre-fill
- wild commit `d470d1f` — tier-3 phase 3 partial writer-skip

### External

- Subsecond source: https://github.com/DioxusLabs/dioxus/tree/main/packages/subsecond
- Subsecond workspace + TLS commit: https://github.com/DioxusLabs/dioxus/commit/33159f366adf7d877c6e5a2987172e3f9992b6f7
- Dioxus v0.7.0 release: https://github.com/DioxusLabs/dioxus/releases/tag/v0.7.0
- Live++: https://liveplusplus.tech/features.html
- Schöner — Fixing bugs with Live++: https://blog.s-schoener.com/2024-12-16-liveplusplus-debug/
- VS C++ EnC supported changes: https://learn.microsoft.com/en-us/visualstudio/debugger/supported-code-changes-cpp?view=vs-2022
- Rider Hot Reload internals: https://blog.jetbrains.com/dotnet/2021/12/02/how-rider-hot-reload-works-under-the-hood/
- Josh Varty — EnC Part 3 (CLR): https://joshvarty.com/2016/05/03/enc-part-3-the-clr/
- ICorDebugModule2::ApplyChanges: https://learn.microsoft.com/en-us/dotnet/core/unmanaged-api/debugging/icordebug/icordebugmodule2-applychanges-method
- ICorDebugILFrame2::RemapFunction: https://github.com/dotnet/docs/blob/main/docs/framework/unmanaged-api/debugging/icordebugilframe2-remapfunction-method.md
- JRebel HotSwap Guide: https://www.jrebel.com/blog/java-hotswap-guide
- HotswapAgent: https://github.com/HotswapProjects/HotswapAgent
- hot-lib-reloader: https://github.com/rksm/hot-lib-reloader-rs
- Robert Krahn — Hot reloading Rust: https://robert.kra.hn/posts/hot-reloading-rust/
- Dexterous Developer (Bevy): https://github.com/lee-orr/dexterous_developer
- GDB Compiling and Injecting Code: https://sourceware.org/gdb/current/onlinedocs/gdb.html/Compiling-and-Injecting-Code.html
- Erlang Code Loading: https://www.erlang.org/doc/system/code_loading.html
- Handmade Hero Day 21: https://yakvi.github.io/handmade-hero-notes/html/day21.html
- nullprogram — Interactive C: https://nullprogram.com/blog/2014/12/23/
- libcare internals: https://github.com/cloudlinux/libcare/blob/master/docs/internals.rst
- GreyNoise — Linux process injection 2025: https://www.labs.greynoise.io/grimoire/2025-01-28-process-injection/
- ptrace(2): https://man7.org/linux/man-pages/man2/ptrace.2.html
- rustc-dev-guide incremental: https://rustc-dev-guide.rust-lang.org/queries/incremental-compilation-in-detail.html
- Subsecond 130ms claim (HN): https://news.ycombinator.com/item?id=44369642
- Production-ready cranelift goal 2025H2: https://rust-lang.github.io/rust-project-goals/2025h2/production-ready-cranelift.html
- (Historical) gilescope/wasmtime PR #1 — `cranelift-hotswap` design, closed 2026-05-22 when JIT path dropped: https://github.com/gilescope/wasmtime/pull/1

---

*Last updated: 2026-05-22 — JIT path dropped; project now AOT-only. AOT pipeline mechanically complete on aarch64 macOS; pre-image drift detection + endpoint hash guard (v3 wild-patch) shipped; BugStalker `apply-patch` + `watch-patch` commands have 15 unit tests. Linux runtime test remains the visible gap.*

## Commit log (this work)

```text
~/git/gilescope/wild/main (2 commits):
  aa10ac1     feat(emit-patch): bump format to v2 with inline pre-image bytes
  03d6bd5     feat(incremental): --emit-patch=<path> writes byte-diff for AOT EnC

~/git/gilescope/BugStalker/main (3 commits):
  4a324f9     feat(apply-patch): pre-image verification for v2 wild-patch format
  da39b9d     feat(apply-patch): auto-detect base + watch-patch poll loop
  34d442b     feat(apply-patch): consume wild-emitted patch files

Uncommitted continuation (2026-05-05):
  wild/libwild/src/lib.rs
    --emit-patch now writes v3 headers with old/new blake3 and optional # fn comments.
  BugStalker/src/ui/command/apply_patch.rs
    apply-patch parser accepts v1/v2/v3 and reports function names in apply/drift diagnostics.

~/git/gilescope/enc-patcher (separate repo, 1 commit):
  0e791ef     enc-patcher v0: same-process .text patcher for AOT EnC

DROPPED 2026-05-22 (JIT path):
  ~/git/gilescope/wasmtime/giles-cranelift-hotswap (5 commits, PR #1 closed)
  ~/git/gilescope/rustc_codegen_cranelift (6 enc-jit commits — fork retired since this was all of its fork-only history)
```
