# Edit-and-Continue for Rust — Design Synthesis

> Status: in progress. **Both JIT and AOT pipelines mechanically complete on aarch64 macOS.** Linux runtime demo + rustc-side MIR re-query are the remaining gaps.
> Date: 2026-05-04 (started); last updated 2026-05-09 (warm enc-jit re-codegen)
> Spans repos: `rustc_codegen_cranelift` (cg_clif), `wasmtime/cranelift/hotswap` (new crate), `BugStalker`, `wild`, and the VSCode debugger extension. `enc-patcher` was an early same-process spike and is not part of this repo bundle.

## What's working today

### JIT path (cg_clif + cranelift-hotswap)

- ✅ `cranelift-hotswap` crate (in our wasmtime fork): JIT with first-class function redefinition. 17 tests pass on aarch64 macOS (native) and x86_64 (via Rosetta), including a 6.95M-call multi-thread stress test.
- ✅ cg_clif's `--features=enc-jit` mode: drop-in replacement for `--features=jit` that uses cranelift-hotswap with `with_hotswap(true)`.
- ✅ Unix-socket redefine listener: when --enc-jit is on, run_jit spawns main as a worker thread and listens on `$CG_CLIF_ENC_SOCKET` (default `/tmp/cg-clif-enc-jit.<pid>.sock`). Raw CLIF connections send `<symbol-name>\n<clif-ir-text>`; warm compiler connections send `RECODEGEN <symbol>`. Worker observes new behaviour on next call.
- ✅ **Live redefine of a long-running mini_core program proven on aarch64 macOS:** `example/encjit_loop.rs` loops calling `compute(x)`; `nc -U` sends new CLIF; transition observed at exact tick (e.g. tick 37: x=38 (x+1) → tick 38: x=138 (x+100)).
- ✅ **`scripts/encjit-redefine.sh`** wraps `rustc-clif --emit=llvm-ir` (which dumps CLIF) + Unix-socket send for one-command edit-and-redefine. cg_clif now annotates imported CLIF function refs with `; enc-symbol <symbol>`, and the `--enc-jit` listener rewrites throwaway-compile `u0:N` references to the live JIT module's `FuncId`s before parsing. Verified on a non-leaf `compute -> helper` demo: redefining `compute` to `helper(x) + 100` produced `OK ... (remapped 2 function refs)` and the running loop switched from `+1` to `+101`.
- ✅ **Warm same-MIR re-codegen path:** `RECODEGEN <symbol>` keeps rustc `TyCtxt` and the hotswap `JITModule` on the listener thread, looks up the symbol's original `Instance`, re-runs cg_clif codegen inside the already-running compiler session, and calls `redefine_function` with the fresh IR. Verified with `RECODEGEN compute` returning `OK recodegen compute -> ...` while the long-running demo was active. Helper script: `scripts/encjit-recodegen.sh`.

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

- ❌ **Linux runtime test**: NixOS x86 box at 192.168.1.137 / .129 not network-reachable from current machine (different subnet). Both pipelines are code-complete; the demo is one Linux box away.
- ⏳ **Edited-source MIR refresh (the remaining rustc piece, task #24).** Warm same-session re-codegen now works for the original MIR. The remaining hard part is making the already-running rustc session see an edited `.rs` file and safely invalidate/re-query only the changed function's MIR before re-running cg_clif codegen. Today's edited-source workflow still invokes a fresh compile to dump CLIF, but the transport is no longer leaf-function-only because imported function refs are remapped by symbol on receive.
- ⏳ **Conflict detection / frame restart on patch overlap.** When a patch entry overlaps the currently-executing PC of any thread, the program would crash if patched naively. Three approaches discussed:
  - (3) Detect + warn: walk threads, find frames in affected fns, surface a diagnostic. ~2 hours, next concrete step.
  - (1) Frame restart without rr: capture args from DWARF formal-parameter locations at fn-entry, reset IP+SP to fn entry, continue. Function logic re-runs; side effects already done are kept. Bounded ~week.
  - (2) Frame restart with rr: checkpoint at fn entry, replay forward with patched code. True rewind. Multi-month (rr-style record/replay is its own project).
- ⏳ **True live mapped-image hash validation.** BugStalker now fail-fast hashes the debugee executable path against the v3 old/new endpoints before patching. A stricter future guard would hash the mapped executable bytes from the live process itself, which matters if the path has been replaced, remapped, or otherwise diverges from the process image.
- ⏳ **`watch-patch` non-blocking variant.** Current implementation blocks BugStalker's REPL; needs main-event-loop integration to allow other commands while watching. Bounded but BugStalker-architectural.
- ⏳ **Programs that need `std`** in cg_clif --enc-jit: `--sysroot none` works for `#![no_core]` mini_core programs; full-std programs need either `--sysroot llvm` (hits the panic-unwind ABI cranelift bug on Apple Silicon) or `--sysroot clif` (rebuild stdlib with cg_clif, slow but should sidestep the bugs).
- ⏳ **DWARF re-registration on redefine.** cg_clif's UnwindContext registers eh_frame at finalize-only; redefine doesn't re-register. Required for debugger backtraces through redefined frames *when unwinding is enabled*. With `--panic-unwind-support` off (current working config), no eh_frame is emitted so this is moot for the abort-only path.
- ⏳ **MAP_JIT for hardened-runtime macOS** in cranelift-hotswap: equivalent to upstream cranelift-jit (i.e. unimplemented). Only matters if cg_clif is run under hardened code signing.

---

## 1. Goal & non-goals

**Goal.** A debugger-driven edit-and-continue (EnC) experience for Rust, modelled on Visual Basic 6 / Visual Studio C++ EnC and on .NET Hot Reload's protocol — *not* on Subsecond / hot-lib-reloader.

User flow: hit a breakpoint → notice a bug → edit one function in your editor → save → debugger silently recompiles that function and patches the running process → continue execution.

**Non-goals.**

- *Application-cooperative* hot reload (Subsecond/Dioxus). The user does not annotate call sites with `subsecond::call(|| ...)`; EnC works on unmodified code.
- *Time-travel / record-replay* (rr-style). Orthogonal, larger, deferred. The combined "rr + EnC" is a Phase 4+ ambition, not a v0 requirement. Pharo-style "restart frame" is included in Phase 2 as a cheap surrogate for "step back."
- *Type/layout changes mid-flight*. Refused in v0; supervised migration (Erlang `code_change/3`-style) is a far-future possibility.
- *Optimised builds*. v0 forces `-O0 -Cinline-threshold=0`. Inlining across a swap boundary is a fundamental obstacle; we sidestep by disabling it.
- *Windows*. Deferred. Different process-control primitives (`OpenProcess` / `WriteProcessMemory` / `CreateRemoteThread`), different debug protocol — own project.

**Platform scope.** Linux **and** macOS are both first-class targets from v0. Architectures: x86-64 and aarch64 (M-series Macs make aarch64 unavoidable from day one).

---

## 2. Architecture

```
┌────────────────────────────────────────────────────────────────────────┐
│  Editor (VSCode + DAP, or any DAP client)                              │
│   ↕ DAP                                                                 │
│  ┌───────────────────────────────────────────┐                         │
│  │  BugStalker (debugger)                     │                         │
│  │   • ptrace seize / cont / step (existing)  │                         │
│  │   • mmap RWX in target via syscall inject  │  ←── new pub APIs:      │
│  │     (existing, just pub(super) → pub)      │      mmap_rwx_in_target │
│  │   • /proc/PID/mem pwrite64 (NEW: bulk     │      write_bytes_to_..  │
│  │     code write, faster than POKEDATA loop) │      install_jmp_redir │
│  │   • install_jmp_redirect (NEW: 14-byte    │      DAP custom request │
│  │     indirect JMP at fn head)               │      bs/encApplyPatch   │
│  └────────┬──────────────────────────────────┘                         │
│           │ DAP custom request bs/encApplyPatch                         │
│           ↕                                                              │
│  ┌───────────────────────────────────────────┐                         │
│  │  enc-driver (small new glue crate)        │                         │
│  │   • file watcher (notify)                  │                         │
│  │   • orchestrates pause-recompile-patch-    │                         │
│  │     resume                                  │                         │
│  └────────┬──────────────────────────────────┘                         │
│           │ JSON-RPC over Unix socket                                   │
│           ↕                                                              │
│  ┌───────────────────────────────────────────┐                         │
│  │  cg_clif --enc-jit (warm compiler)        │                         │
│  │   • holds rustc query state               │                         │
│  │   • re-codegens single fn on demand       │                         │
│  │   • returns: { fn_addr, len, mir_block_   │                         │
│  │              offsets[] }                   │                         │
│  │   • cranelift-jit with hotswap=true       │                         │
│  │     (RESTORED from 0.95.1)                │                         │
│  └───────────────────────────────────────────┘                         │
│                                                                          │
│  ┌───────────────────────────────────────────┐                         │
│  │  Debuggee (your Rust app)                  │                         │
│  │   started under cg_clif --enc-jit          │                         │
│  │   running in same address space as JIT     │                         │
│  └───────────────────────────────────────────┘                         │
└────────────────────────────────────────────────────────────────────────┘
```

In Phase 3 the architecture shifts: the debuggee is an AOT-built binary, cg_clif emits a relocatable `.o`, wild produces a delta patch via its incremental tier-3/tier-4 path, and BugStalker patches the `.text` of the live process via `/proc/PID/mem` + indirect-JMP trampoline.

---

## 3. Findings (condensed from 6 research dispatches)

### 3.1 cg_clif JIT — current reality

- All inter-fn calls in JIT mode are direct `CALL rel32` with the address burned in at `finalize_definitions()`. No GOT, no PLT, no indirection. Source: `src/abi/mod.rs:123-125` (`get_function_ref` → `declare_func_in_func`).
- `is_pic = false` is forced when JIT (`src/lib.rs:261`).
- cranelift-jit 0.131.0 (current) **does not support hotswap**. The machinery — `prepare_for_function_redefine`, `JITBuilder::hotswap`, `function_got_entries: SecondaryMap<FuncId, NonNull<AtomicPtr<u8>>>`, PLT trampoline emission — existed in 0.95.1 and was **removed**. cranelift-jit 0.131 explicitly *panics* on `X86GOTPCRel4` / `X86CallPLTRel4` relocations when `is_pic=false`.

**Path forward.** Restore hotswap in cranelift-jit (Option A, cleaner, contributable upstream) or add cg_clif-owned per-fn trampolines (Option B, no upstream work but per-arch stub code). **Pick A.**

### 3.2 `cranelift-hotswap` — new crate, in-tree at `wasmtime/cranelift/hotswap/`

> **Updated 2026-05-04 after design discussion.** Path chosen: develop independently as a new crate inside the wasmtime monorepo (not a fork; not a separate repo). When the design is proven we open a PR to merge it; until then we work on a local branch. Crate name: `cranelift-hotswap`.

#### 3.2.1 Why the previous attempt was removed

**Why it was removed.** [PR #10345](https://github.com/bytecodealliance/wasmtime/pull/10345) (merged 2025-03-06 by **bjorn3** — who maintains *both* cg_clif and cranelift-jit) removed hotswap from cranelift-jit 0.118.0. [PR #10390](https://github.com/bytecodealliance/wasmtime/pull/10390) followed up by removing `is_pic` support entirely. The stated reason, verbatim:

> "It was originally introduced for cg_clif. cg_clif recently removed its use of hotswapping as the way it is implemented in cranelift-jit has various issues like leaking memory, panicking when the memory allocator decided to put two functions more than 2GB away from each other and only supporting x86_64. Better hotswapping support will likely require a fundamentally different implementation."

Structural issues (per [issue #5005](https://github.com/bytecodealliance/wasmtime/issues/5005)):

1. **2 GB range panic** — GOT/PLT used 32-bit PC-relative offsets. When mmap regions land > 2 GB apart, `i32::try_from(offset)` panics with no recovery.
2. **Memory leaks** — `prepare_for_function_redefine` allocated new executable pages but never freed old ones.
3. **x86_64 only** — aarch64 and riscv64 never had PLT emission. *This alone disqualifies the old design for our macOS Apple-Silicon requirement.*

bjorn3 explicitly said *"better hotswapping support will likely require a fundamentally different implementation"* — not "we don't want it." Maintainers (bjorn3, abrown, alexcrichton) showed no principled objection, only pragmatism.

**Implication for us.** A simple revert won't work. But the new constraints align well with our cross-platform requirements, and the right new design is *simpler* than the old one — we no longer need `is_pic = true` in cg_clif, which eliminates the TLS/static audit.

#### 3.2.2 Lessons learned and explicit fixes

| Original failure | Root cause | Our fix |
|---|---|---|
| 2 GB range panic | 32-bit PC-relative GOT/PLT | Absolute 64-bit pointers via side table |
| Memory leaks on redefine | No retirement API | v0: leak by design (documented); v1: epoch-based reclamation. Per-fn-version page allocation so retirement is possible later |
| x86_64-only | PLT emission was a shortcut | Per-arch dispatch from day one (x86_64 + aarch64 minimum) |
| `is_pic = true` coupling | Tied to PC-relative addressing | Absolute pointers don't need PIC at all |
| Awkward two-phase `prepare_for_function_redefine` API | Leaked impl details | Single-call `redefine_function(FuncId, &Function) -> *const u8` |
| Sole consumer drifted away | "for cg_clif," cg_clif moved on | Consumer is debugger-driven EnC — *we* maintain the consumer |
| Unclear failure modes | Edge cases implicit | Documented invariants: retirement requires no-thread-inside; range is unrestricted; redefine is atomic w.r.t. dispatch read |
| No multi-threaded test coverage | Tests were single-threaded | Stress + property tests (loom / shuttle) from the start |

#### 3.2.3 Design of `cranelift-hotswap`

- **Side-table indirection via absolute 64-bit pointers.** Per-FuncId slot of type `AtomicU64` holding the absolute address of the current function body. Storage is **chunked, lockless, growable** — sealed `[AtomicU64; CHUNK]` blocks installed atomically as FuncIds are issued. Lookup: `chunk_index = funcid / CHUNK; entry = chunks[chunk_index][funcid % CHUNK]`. (Conceptually similar to Subsecond's `JumpTable: HashMap<u64,u64>`, but indexed by FuncId without hashing.)
- **Call lowering — Form A (per-call indirect):** `mov rax, [side_table + funcid*8]; call rax` on x86-64, `ldr x16, [<table_entry>]; blr x16` on aarch64. 7-9 bytes per call site, ~1 extra load per call. **EnC mode is `-O0` anyway, perf cost irrelevant.** This is the likely choice.
- **Form B (per-fn trampoline near call site):** keep `CALL rel32` at call sites, indirect via local trampoline. One fewer load on the hot path. Not worth the complexity for v0; revisit if benchmarks demand.
- **No `is_pic = true`.** Absolute 64-bit pointers reach anywhere; no GOT, no PLT, no PC-relative range. cg_clif's `is_pic = false` stays. **No TLS/static audit needed.** Big simplification vs the original revert plan.
- **2 GB range issue dissolved** — absolute pointers reach anywhere.
- **Cross-platform from day one** — same model works for x86_64 + aarch64. macOS Apple Silicon supported natively. matches our §1 platform scope.
- **Memory management — v0 leaks; v1+ retires.** Each redefinition allocates a fresh page; old pages are kept indefinitely. Worst-case leak in a long session: tens of MB (50 redefines × 100 fns × 16 KB pages = ~80 MB on Apple Silicon; ~20 MB on x86 Linux). Acceptable for prototype use. *But* we allocate per-fn-version (not packed slabs) from day one so retirement can be added later without architectural rework. `JITModule::retire_function(FuncId)` exists as a no-op stub in v0; v1+ implements epoch-based reclamation. `JITModule::drop()` reclaims everything (module-level cleanup is free in v0).
- **Two-phase W^X uniform across platforms.** Both Linux and macOS use the same RW→write→RX flow: allocate `MAP_JIT`-equivalent page, write code while writable, flip to executable. macOS: `MAP_JIT` + `pthread_jit_write_protect_np()`. Linux: `mprotect(RW)` → write → `mprotect(RX)`. Same logic; no Linux-only bugs lurking.
- **API surface:**
  - `JITBuilder::with_hotswap(true)` — enables side-table indirection at codegen time.
  - `JITModule::redefine_function(FuncId, &Function) -> Result<*const u8>` — compile new body, atomic-store new pointer into side table, return new addr.
  - `JITModule::retire_function(FuncId) -> Result<()>` — caller asserts no thread is inside; pages may be reclaimed.

**Steps (revised — independent, no upstream pitch):**

1. **Develop independently** in `~/git/gilescope/wasmtime/cranelift/hotswap/` on the `giles-cranelift-hotswap` branch. No upstream coordination at any point — we own this and use it ourselves.
2. ✅ Build basic JIT (no hotswap) on aarch64 — verify cranelift-codegen + cranelift-module integration. **Done.**
3. ✅ Add chunked side-table dispatch + per-fn trampoline + `redefine_function`. **Done — aarch64.**
4. ✅ Multi-thread stress (concurrent callers + concurrent redefines). **Done — 6.95M calls + 995 redefines, 0 corruptions.**
5. Two-phase W^X uniform across Linux/macOS (currently works on macOS via region::protect; verify on hardened runtime).
6. x86_64 trampoline runtime test (encoded but not yet executed; aarch64 is our local target).
7. DWARF / `.eh_frame` per-version registration. Required for backtraces through redefined frames.
8. **Plumb into cg_clif's JIT driver** as a new `--enc-jit` mode. This is the real edit-and-continue payoff.

**Status (2026-05-04): hotswap engine functional + concurrency-safe on aarch64 macOS. ~5 weeks of work realistically remaining for full Linux + DWARF + cg_clif integration.**

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
| `Debugger::call_fn_raw` — internal | `src/debugger/call/mod.rs` | `pub(super)` |
| Oracle trait + transparent breakpoints with `Fn(&mut Debugger)` callback | `src/oracle/mod.rs`, `src/debugger/breakpoint.rs:29-63` | ready |
| DAP custom request `bs/*` namespace | `src/dap/yadap/session/mod.rs:630-667` | hard-coded; unknown commands return error |

**Gap.** Three new `pub` methods + one new DAP custom request:

```rust
impl Debugger {
    /// Inject mmap(NULL, size, PROT_R|W|X, MAP_PRIVATE|ANON, -1, 0) in target.
    /// Wraps existing CallHelper::mmap. (NEW pub.)
    pub fn mmap_rwx_in_target(&mut self, size: usize) -> Result<*mut u8>;

    /// Bulk write to target via /proc/PID/mem pwrite64 (faster than POKEDATA loop,
    /// no word-alignment constraint). (NEW.)
    pub fn write_bytes_to_target(&mut self, addr: *mut u8, bytes: &[u8]) -> Result<()>;

    /// Install 14-byte indirect JMP at old_fn_addr redirecting to new_fn_addr.
    /// Saves displaced bytes for an optional trampoline-back stub. (NEW.)
    pub fn install_jmp_redirect(&mut self, old_fn_addr: usize, new_fn_addr: usize)
        -> Result<RedirectHandle>;
}

// In src/dap/yadap/session/mod.rs, add:
//   "bs/encApplyPatch" → handler that reads { symbol, code_bytes, mir_block_offsets }
//   from request body and drives mmap_rwx + write_bytes + install_jmp_redirect.
```

### 3.4 Linux process-patching playbook (recommended sequence)

For the paused-tracee EnC patch:

1. **Allocate RWX page** — inject `mmap` syscall (BugStalker `CallHelper::mmap` already does this).
2. **Bulk-write new code** — `pwrite64` to `/proc/PID/mem` (NEW; trivially added). Falls back to POKEDATA loop if `/proc` access denied.
3. **Atomic redirect** — write 14-byte indirect-JMP at the original fn entry: `FF 25 00 00 00 00 <8-byte abs target>` (x86-64) or 12-byte LDR/B sequence (aarch64). Process is fully ptrace-stopped, so the write is atomic w.r.t. other threads.
4. *(optional)* **Preserve old version reachable** — copy displaced bytes + JMP-back into a trampoline stub. Useful for in-flight frames; not required if v0 forbids on-stack patching.

W^X gotcha: hardened systems may refuse `MAP_ANONYMOUS | PROT_WRITE | PROT_EXEC`. Mitigation: allocate `RW`, write, then `mprotect(RX)` (also via syscall injection).

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

**Gap for Phase 3 EnC.** Section-granularity, not function-granularity. Need `.text` partitioned into per-fn subsections (rustc supports this with COMDAT). Wild already supports COMDAT; the question is whether the section count (tens of thousands per large binary) overloads the layout snapshot — engineering, not fundamental.

### 3.6 Subsecond — adjacent, not the model

Subsecond's mechanism (concrete):

1. **ThinLink** wraps the system linker, caches per-crate `.rcgu.o` files, diffs against prior cache to identify changed fns.
2. Re-links *only changed object files* (plus their cascade closure) into a tiny patch dylib, with unchanged-fn references resolved against runtime addresses of the live process.
3. Runtime: `dlopen` the patch dylib, fill `JumpTable: HashMap<u64,u64>` with old→new addresses.
4. User wraps callsites in `subsecond::call(|| f(...))`. Inside, `HotFn` consults the table; on stale-caller detection emits `HotFnPanic`, unwinds to nearest outer `call` boundary, retries.

Achievement: 130-500 ms turnaround on M4-class hardware. Tip-crate-only initially (workspace work landed late in 0.7).

**Why we differ.** Subsecond never writes to `.text`; we do. Subsecond requires app cooperation; we don't. Subsecond's swap point is application-defined; ours is debugger-defined (any breakpoint). Subsecond's audience is "I'm running my Dioxus app and want to tweak a component"; ours is "I'm in a debugger session, hit a bug, want to fix and step over again."

### 3.7 Prior-art lessons worth stealing

- **.NET `OnFunctionRemapOpportunity` + `ICorDebugILFrame2::RemapFunction` callback.** Runtime fires "frame needs remap" event; compiler/IDE provides (old IL offset → new IL offset) translation. Apply: cg_clif emits `(MIR basic-block id → machine offset)` map; BugStalker uses it for active-frame IP translation. Refuse where no equivalent exists.
- **GDB `compile code` / `libcc1` plugin boundary.** Debugger receives an ELF blob from compiler; knows nothing else. Same shape for our cg_clif → BugStalker interface.
- **Erlang `code_change/3`.** When struct layout changes, demand explicit user-supplied migration. Don't silently reinterpret memory.
- **Live++ out-of-process agent.** Heavy tooling outside target; minimal in-target footprint. Limits blast radius.
- **VS C++ "stale code" warning.** Recursive / on-stack patching silently mixes old and new code paths. Surface explicitly. Don't pretend it's safe.
- **Pharo's "restart frame" UX.** Drop & re-enter current call frame. Our cheapest surrogate for "step back" — pairs naturally with EnC.
- **Muratori's state externalisation + input-replay** (Handmade Hero). Game code owns no state; record memory + input stream. Combined with EnC: deterministic re-run after edit. Phase 4+ ambition.

### 3.8 rustc incremental — fast enough for v0

- Pre-backend granularity: per-function via the query DAG.
- Backend granularity: **per-CGU**. Even with one fn changed, the entire CGU re-emits. Default `-Ccodegen-units=256`; raising further has linker-overhead cost but no fundamental block.
- No rustc daemon RFC exists. `rustc_interface` is the unstable library API; each call is a fresh session (~50-200 ms startup).
- Subsecond achieves 130-500 ms via stock cargo + rustc incremental + ThinLink. cg_clif's faster codegen should pull this lower.
- Sub-100 ms blocked by: (a) CGU granularity; (b) session startup. Out of scope for v0.

**Realistic v0 target: 130-350 ms turnaround.**

---

## 4. Phase 1 — JIT MVP (smallest end-to-end demo)

**Scope.** Single fn body changed, single thread, fn not currently on stack, no signature/layout/static changes. JIT mode only. Demo: bug-in-fibonacci.

### 4.1 Deliverables

1. **cranelift-jit hotswap (NEW design — see §3.2)**
   - File design issue on `bytecodealliance/wasmtime`, tag bjorn3.
   - Implement side-table indirection via absolute 64-bit pointers; cross-platform (x86-64 + aarch64) from day one.
   - API: `JITBuilder::with_hotswap`, `JITModule::redefine_function`, `JITModule::retire_function`.
   - Time-box 4-6 weeks. Fallback: cg_clif-owned side table (same design, lives in cg_clif) if upstream conversation stalls.

2. **cg_clif `--enc-jit` flag** (in `src/driver/jit.rs`)
   - Forces `-O0 -Cinline-threshold=0 -Cdebuginfo=2`.
   - `is_pic = false` stays (new hotswap design uses absolute 64-bit pointers, no PIC needed).
   - Calls `jit_builder.with_hotswap(true)`.
   - Spawns a Unix-socket JSON-RPC endpoint accepting `recompile_fn(symbol_name)` returning `{ addr: u64, len: usize, mir_block_offsets: [(block_id, byte_offset)] }`.
   - For v0: cg_clif compiler runs **in the same process** as the JIT'd app (the JIT is in-process anyway). "Warm rustc daemon" = Phase 3+.

3. **BugStalker pub-API extension**
   - Make `Debugger::call_fn_raw` `pub` (or add a `pub` wrapper).
   - Add `Debugger::mmap_rwx_in_target(size) -> Result<*mut u8>` (wraps existing `CallHelper::mmap`).
   - Add `Debugger::write_bytes_to_target(addr, &[u8])` — use `/proc/PID/mem` `pwrite64`, fall back to POKEDATA loop.
   - Add `Debugger::install_jmp_redirect(old_addr, new_addr) -> RedirectHandle`.
   - Add DAP custom request `bs/encApplyPatch { symbol, code_bytes, mir_block_offsets }`.

4. **`enc-driver` glue crate** (new, in a fresh repo or `~/git/gilescope/enc-driver/`)
   - File watcher (notify crate, 200 ms debounce).
   - DAP client connection to BugStalker.
   - JSON-RPC client to cg_clif.
   - Orchestrates: pause → ask cg_clif to recompile → send `bs/encApplyPatch` → resume.

5. **Demo binary** — 60-line Rust program with buggy `fib(n)`. Pause at breakpoint, edit, save, watch corrected behaviour from same paused state.

### 4.2 Out of scope for v0

- Recursion / on-stack patching → refuse with explicit warning.
- Multi-threaded patching → ptrace-stops-all (Linux) / `task_suspend` (macOS) already gives safety, but verify per-platform.
- Type / layout / static / signature changes → refuse.
- Inlined call sites → `-Cinline-threshold=0` blocks them.
- Windows.

### 4.3 Platform-specific notes (Linux & macOS, v0)

| Concern | Linux | macOS |
|---|---|---|
| Process control | `ptrace` (BugStalker has it) | `task_for_pid` + Mach exception ports + `thread_suspend` (BugStalker does **not** support — see §4.4) |
| RWX memory in target | `mmap(PROT_R\|W\|X, MAP_ANON)` via syscall injection | `mmap(MAP_JIT)` + `pthread_jit_write_protect_np` toggle; requires `com.apple.security.cs.allow-jit` entitlement on hardened runtime |
| Bulk code write | `/proc/<pid>/mem` `pwrite64` | `mach_vm_write` |
| ASLR disable | `personality(ADDR_NO_RANDOMIZE)` (BugStalker uses) | `POSIX_SPAWN_DISABLE_ASLR_NP` (only on debuggee spawn; can't be done post-hoc) |
| Trampoline at fn head | 14-byte indirect JMP (x86-64) / 12-byte LDR/B (aarch64) | Same instruction sequences; `pthread_jit_write_protect_np(false)` before write, `(true)` after |
| W^X permanently enforced? | LSM-dependent (SELinux/AppArmor) | Yes on Apple Silicon hardened runtime — must use `MAP_JIT` |

### 4.4 BugStalker macOS gap

BugStalker today is Linux-only (aarch64-Linux experimental, no macOS). For Phase 1 macOS we have two paths:

- **Path A (preferred):** add macOS support to BugStalker. Mach `task_for_pid` + exception ports + `thread_get_state`/`thread_set_state`. Substantial — multi-week project on its own. Worth coordinating with godzie44 upstream.
- **Path B (interim):** for macOS Phase 1, use an *in-process* EnC orchestrator instead of an external debugger. The cg_clif JIT runs in-process anyway; a file-watcher thread inside the JIT'd app can `pthread_kill(thread, SIGSTOP)` siblings (or use `task_suspend` on its own task), recompile, swap, resume. Same JIT swap mechanism; just no external debugger. Loses the breakpoint-paused UX but unblocks the engine.

**v0 decision: ship Path B for macOS in parallel with Path A's bring-up.** Path B is also a useful capability on Linux as a "no-debugger-attached" mode.

### 4.3 Estimated effort

4-6 weeks for one engineer familiar with all three repos. Cranelift-jit hotswap is the largest single piece (~2 weeks). The rest is plumbing.

---

## 5. Phase 2 — Multi-fn + active-frame remap

**Scope.** Function may be currently on a thread's stack. Multiple fns per edit (closure of MIR change set). Static-initialiser detection.

**Key new piece.** `.NET RemapFunction` protocol applied to Rust:

- cg_clif emits `(MIR block id → machine offset)` map alongside each compiled fn.
- For each ptrace-stopped thread, BugStalker walks the call stack. For frames whose fn was patched:
  - Look up the current PC in the old fn's machine→MIR-block map (already in DWARF).
  - Look up that MIR block id's offset in the *new* fn's map.
  - Hijack the thread's IP to the new offset.
  - If no equivalent block exists (block deleted, control flow restructured), refuse the swap on that frame and surface a "stale code in frame N" warning to the user.

Pharo "restart frame" UX: explicit user action to drop & re-enter the current fn from the top with snapshotted args. Cheap, satisfying, useful even without active-frame remap.

Static-initialiser detection: hash MIR of `MonoItem::Static` initialisers; if changed and layout unchanged, re-run init; if layout changed, refuse the edit.

---

## 6. Phase 3 — AOT mode with wild

**Scope.** The debuggee is no longer JIT-mode; it's a normal AOT-built Rust binary running.

**Pipeline.**

1. cg_clif (or cg_llvm) re-compiles the changed fn(s) to a relocatable `.o`.
2. wild's daemon (`wild --serve`) produces a delta patch — leveraging:
   - Tier-1 parse-skip cache (already shipped)
   - Tier-2 layout snapshot (already shipped)
   - Tier-3 partial writer-skip (already shipped)
   - Tier-4 4 KiB ALLOC padding (your commit 4a064bf, 2026-04-26)
   - **NEW: function-granularity `.text` partition** (current gap; rustc supports COMDAT, wild needs to handle the section-count blow-up)
3. wild emits a "delta patch" — the changed bytes plus relocation fixups for function entries.
4. BugStalker writes the new code into the running process (`/proc/PID/mem`), installs trampoline at old fn head.

This is *Subsecond's architecture but driven by a debugger and patching real `.text` in place* (Live++ style) rather than `dlopen`-ing a separate dylib — which Subsecond does for safety reasons but which has the Linux TLS-leak issue on `dlclose`.

---

## 7. Phase 4 — UX/IDE & far-future

- VSCode DAP extension that triggers EnC on save while debugger is paused.
- Visual indicator: "this fn was edited but is currently on the stack — drop frame to apply?"
- Snapshot+replay mode (Muratori-inspired) — record register state before edit, replay after.
- rr integration: "replay to bug, edit, continue forward" is the genuinely killer combination. Requires both pieces. Dependent on independent rr-style work in BugStalker.
- *(macOS is first-class from v0; not deferred.)*
- Optional `code_change` callback (Erlang-inspired) for user-supervised struct migration.
- Windows port.

---

## 8. Open architectural questions

1. **Where does warm rustc state live?**
   - Option A: in-process with the JIT'd app. Simple. Shares fate.
   - Option B: separate `enc-rustc-daemon` process. Survives crashes. IPC cost.
   - Option C: Subsecond-style cargo+rustc subprocess. ~150 ms session-startup cost.
   - **Recommendation: A for v0, revisit at Phase 3.**

2. **cranelift-jit hotswap: contribute upstream or fork?**
   - Contributing back to bytecodealliance is the right long-term move; fork is technical debt.
   - **Recommendation: open a PR + design discussion in `bytecodealliance/wasmtime`. Carry our branch in parallel.**

3. **Inlining policy.**
   - **No per-fn attribute.** The user wants EnC to work on *any* function at *any* breakpoint without source annotation — that's the VB6 ergonomic baseline. No `#[hot_swappable]` opt-in.
   - Compile-mode flag instead: the *whole crate* (or whole compilation) builds with EnC mode, which forces `-Cinline-threshold=0`, no MIR-level inlining, debug info on. Like VS's `/ZI`.
   - Release mode is unchanged; EnC only kicks in when you're running under the EnC build.
   - **Recommendation: binary mode (whole-build switch) for v0. Never per-fn annotation.**

4. **Type / layout changes.**
   - Refuse in v0 with clear error.
   - Phase 2: detect via dep-graph + layout hash; surface as a build-time error.
   - Phase 3+: optional `code_change` migration callback.

5. **Three-repo coordination.**
   - cg_clif and wild are your domain (or contribution-heavy). BugStalker is godzie44's. Coordinate API extensions with upstream early to avoid carrying a fork forever.

---

## 9. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Cranelift-jit hotswap design discussion stalls upstream | Time-box 4-6 weeks. Fall back to cg_clif-owned side table (same design, just lives in cg_clif). bjorn3 maintains both crates, so the upstream contact and the cg_clif contact are the same person — should accelerate alignment. |
| BugStalker upstream rejects API surface extension | Carry a small public-API patch on a fork; pursue re-upstreaming in parallel. |
| `/proc/PID/mem` writes fail under hardened LSM (SELinux) | Document. Fall back to POKEDATA loop. |
| W^X enforcement refuses RWX `mmap` | Allocate `RW`, write, `mprotect(RX)`. |
| macOS hardened runtime forbids RWX | First-class in v0. Use `MAP_JIT` + `pthread_jit_write_protect_np` toggle; project requires `com.apple.security.cs.allow-jit` entitlement (developers building EnC-mode binaries opt in). |
| BugStalker has no macOS port | Path A: contribute macOS support upstream (multi-week). Path B (interim): in-process EnC orchestrator on macOS, ship in parallel. |
| Subsecond gets there first / takes the audience | Different problem. Cooperative app hot-reload vs debugger-driven EnC. Both can exist. |
| Recursive / on-stack patching mixes old/new code silently | Explicit "stale code" warning UX. v0 refuses entirely. |
| Inlined copies of patched fn keep running old code | `-Cinline-threshold=0` in EnC mode. v0 only. |
| Linux TLS dlclose memleak | Avoid by patching `.text` in place (Phase 3) rather than `dlopen`/`dlclose` churn. |

---

## 10. Smallest first commit

Single PR-shaped change in `~/git/gilescope/BugStalker`:

> `src/debugger/call/mod.rs` — change `CallHelper::mmap`'s visibility from `pub(super)` to `pub`. Add a thin wrapper `Debugger::mmap_rwx_in_target(&mut self, size: usize) -> Result<*mut u8>` that forwards to it. Tests: spawn a debuggee, call the new method, verify a fresh RWX page exists in `/proc/<pid>/maps`.

Useful on its own (process-injection tools, observability tooling), validates the API shape, and is the foundation everything else hangs from. A day's work; merge it before any of the larger pieces start.

After that, the natural sequence:

1. `Debugger::write_bytes_to_target` (small).
2. `Debugger::install_jmp_redirect` (small, x86-64 only initially).
3. DAP custom request `bs/encApplyPatch` (small).
4. cg_clif `--enc-jit` flag scaffolding (medium).
5. cranelift-jit hotswap restoration (large).
6. `enc-driver` glue crate (small).
7. End-to-end demo.

---

## 11. Glossary

- **EnC** — edit-and-continue. The ability to modify code mid-debug-session and continue from the same paused state.
- **CGU** — codegen unit. rustc's compilation parallelism unit; the granularity at which the backend re-emits code under incremental.
- **GOT / PLT** — Global Offset Table / Procedure Linkage Table. Indirection layer used by dynamic linkers; cranelift-jit 0.95.1 used these for hotswap.
- **MIR** — rustc's mid-level intermediate representation. Per-fn, basic-block-structured, post-monomorphisation.
- **DAP** — Debug Adapter Protocol. Microsoft's editor↔debugger protocol. BugStalker speaks it.
- **ptrace** — Linux process tracing syscall. BugStalker's primary control mechanism.
- **Hotswap** — runtime replacement of a function's code while the process is running.
- **Trampoline** — a small stub of code that redirects execution from one address to another (typically via `JMP`).

---

## 12. References

### Local repo file paths

- `~/git/gilescope/rustc_codegen_cranelift/src/driver/jit.rs` — JIT driver (run_jit, codegen_and_compile_fn)
- `~/git/gilescope/rustc_codegen_cranelift/src/abi/mod.rs:123-125` — call lowering via `declare_func_in_func`
- `~/git/gilescope/rustc_codegen_cranelift/src/lib.rs:261` — `is_pic = false` in JIT
- `~/git/gilescope/rustc_codegen_cranelift/src/driver/mod.rs:17-52` — `predefine_mono_items`
- `~/git/gilescope/BugStalker/src/debugger/call/mod.rs:384-455` — `CallHelper::mmap`
- `~/git/gilescope/BugStalker/src/debugger/call/mod.rs:886-931` — `Debugger::call`
- `~/git/gilescope/BugStalker/src/debugger/process.rs:147-166` — ptrace seize
- `~/git/gilescope/BugStalker/src/debugger/mod.rs:1180-1190` — `write_memory`
- `~/git/gilescope/BugStalker/src/oracle/mod.rs` — Oracle trait
- `~/git/gilescope/BugStalker/src/dap/yadap/session/mod.rs:630-667` — DAP custom request dispatch
- `~/git/gilescope/wild/INCREMENTAL.md` — incremental linking guide
- `~/git/gilescope/wild/libwild/src/incremental_cache.rs` — `LinkCache`, `InputHash`
- `~/git/gilescope/wild/libwild/src/layout_snapshot.rs` — `LayoutSnapshot`
- `~/git/gilescope/wild/libwild/src/tier3_skip.rs` — partial writer-skip state
- `~/git/gilescope/wild/libwild/src/daemon.rs` — `wild --serve`
- wild commit `4a064bf` (2026-04-26) — tier-4 4 KiB ALLOC padding (Giles Cope)
- wild commit `7823430` — tier-3 mmap-COW pre-fill
- wild commit `d470d1f` — tier-3 phase 3 partial writer-skip

### External

- cranelift-jit 0.131.0 source (no hotswap): https://docs.rs/cranelift-jit/0.131.0/src/cranelift_jit/backend.rs.html
- cranelift-jit 0.95.1 source (had hotswap): https://docs.rs/cranelift-jit/0.95.1/cranelift_jit/struct.JITModule.html
- PR #10345 — Remove hotswapping support from cranelift-jit (bjorn3, 2025-03-06): https://github.com/bytecodealliance/wasmtime/pull/10345
- PR #10390 — Remove support for is_pic (bjorn3, 2025-03-20): https://github.com/bytecodealliance/wasmtime/pull/10390
- Issue #5005 — PLT panic + hotswap design problems: https://github.com/bytecodealliance/wasmtime/issues/5005
- PR #2786 — Original hotswap addition (June 2021): https://github.com/bytecodealliance/wasmtime/pull/2786
- PR #12239 — Veneer insertion for arm64 (related technique): https://github.com/bytecodealliance/wasmtime/pull/12239
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
- rustc-dev-guide queries: https://rustc-dev-guide.rust-lang.org/query.html
- rustc-dev-guide CGU partitioning: https://rustc-dev-guide.rust-lang.org/backend/monomorph.html
- Subsecond 130ms claim (HN): https://news.ycombinator.com/item?id=44369642
- Production-ready cranelift goal 2025H2: https://rust-lang.github.io/rust-project-goals/2025h2/production-ready-cranelift.html
- bjorn3 cg_clif Nov 2024 progress: https://bjorn3.github.io/2024/11/14/progress-report-nov-2024.html

---

*Last updated: 2026-05-05 — both JIT and AOT pipelines mechanically complete on aarch64 macOS; pre-image drift detection (v2 wild-patch) shipped; BugStalker `apply-patch` + `watch-patch` commands added with 10 unit tests; `encjit-redefine.sh` wraps the JIT-side workflow. Open to revision after Linux runtime feedback.*

## Commit log (this work)

```
~/git/gilescope/wasmtime/giles-cranelift-hotswap (3 commits):
  d8e3770bea  cranelift-hotswap: link to live_demo from DESIGN.md status
  7770a00ef8  cranelift-hotswap: add live_demo example
  ee52935e44  Add cranelift-hotswap: JIT with first-class function redefinition

~/git/gilescope/rustc_codegen_cranelift/main (4 commits):
  ec9fc2ba    Add scripts/encjit-redefine.sh: recompile + send CLIF wrapper
  88f10ec7    Add encjit_loop.rs: long-running mini_core demo for live redefine
  66519be1    cg_clif: add Unix-socket redefine listener in --enc-jit mode
  bdd13dff    Wire cranelift-hotswap into cg_clif as a new --enc-jit mode

~/git/gilescope/wild/giles-mac (2 commits):
  aa10ac1     feat(emit-patch): bump format to v2 with inline pre-image bytes
  03d6bd5     feat(incremental): --emit-patch=<path> writes byte-diff for AOT EnC

~/git/gilescope/BugStalker/giles-rust-visuals (3 commits):
  4a324f9     feat(apply-patch): pre-image verification for v2 wild-patch format
  da39b9d     feat(apply-patch): auto-detect base + watch-patch poll loop
  34d442b     feat(apply-patch): consume wild-emitted patch files

Uncommitted continuation (2026-05-05):
  wild/libwild/src/lib.rs
    --emit-patch now writes v3 headers with old/new blake3 and optional # fn comments.
  BugStalker/src/ui/command/apply_patch.rs
    apply-patch parser accepts v1/v2/v3 and reports function names in apply/drift diagnostics.

~/git/gilescope/enc-patcher (new, 1 commit):
  0e791ef     enc-patcher v0: same-process .text patcher for AOT EnC
```

To sign + push (after review):

```sh
cd ~/git/gilescope/wasmtime          && git rebase --exec 'git commit --amend --no-edit -S' HEAD~3 && git push
cd ~/git/gilescope/rustc_codegen_cranelift && git rebase --exec 'git commit --amend --no-edit -S' HEAD~4 && git push
cd ~/git/gilescope/wild              && git rebase --exec 'git commit --amend --no-edit -S' HEAD~2 && git push
cd ~/git/gilescope/BugStalker        && git rebase --exec 'git commit --amend --no-edit -S' HEAD~3 && git push
cd ~/git/gilescope/enc-patcher       && git rebase --exec 'git commit --amend --no-edit -S' HEAD~1 && git push
```
