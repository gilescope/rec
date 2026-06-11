# debug-step-costs — handoff

State of the BugStalker **perf overlay / Step Costs** feature and the open
questions, so a fresh session can continue. Spans two repos under
`~/git/gilescope/rec/`: `bugstalker` (the `bs` debug adapter) and
`vscode-extension` (the VS Code client, ships as `vadimcn.vscode-lldb-0.1.0`).

## How to build / test / install

- **Install both** (macOS): `cd ~/git/gilescope/rec && earthly +install-darwin`
  (builds `bs` with `--features perf` + adhoc cs.debugger sign, packages the
  `.vsix`, installs it). Reload the VS Code window after.
- **Extension tests** (headless, real VS Code): `cd vscode-extension && npm test`.
  If it `SIGTRAP`s at startup, `rm -rf .vscode-test` to force a fresh download.
- **bs tests**: `cd bugstalker && cargo test --features perf --lib -- <filter>`;
  the x16 capture test is `cargo test -p bs-perf parked_thread_x16…`.
- **Thread isolation** is opt-in: relaunch VS Code as
  `BS_PERF_ISOLATE_THREAD=1 code` so the spawned `bs` inherits it.

## What's built (the pipeline)

`bs` samples + measures each step window and attaches `bs_perf` to the DAP
`stopped` event; the extension renders it.

- **`bugstalker/crates/bs-perf/src/darwin/poll_sampler.rs`** — 1 kHz poll
  sampler. `ThreadSample { pc, syscall }`; on aarch64 it reads the PC *and*
  `x16` (syscall number) from `arm_thread_state64_t`. `PollDrain.syscalls`.
- **`bugstalker/crates/bs-perf/src/darwin/rusage.rs`** — `ProcessSnapshot`
  (proc_pid_rusage): cpu_time, instructions, cycles, pageins, disk, and
  `phys_footprint`; `ProcessDelta` incl. signed `phys_footprint_delta`.
- **`bugstalker/src/dap/yadap/session/perf.rs`** — the session glue:
  - `begin_perf_run` / `finish_perf_stop` bracket the step window.
  - `perf_stopped_summary_body` emits `bs_perf`: `runCycles`, `runInstructions`,
    `runCpuTimeNs`, `ipc`, `mode`, `diagnosis {emoji,label,summary,hint}`,
    `physFootprintDelta`, `hot`, `unresolvedSamples`.
  - `diagnose` — categorisation; the `mostly-waiting` branch now refines via
    `classify_wait(x16)` + `dominant_wait_syscall` → 🔒 lock / 😴 sleep /
    🌐 i/o / 📨 ipc. Numbers from `bsd/kern/syscalls.master`.
  - `ThreadFreezeGuard` / `install_thread_freeze` — `BS_PERF_ISOLATE_THREAD`
    freezes non-focus threads for the window (focus matched by **thread id**),
    1 s watchdog thaws on a block-on-frozen-thread stall; `thaw()` is
    swap-idempotent.
- **`vscode-extension/extension/perfOverlay.ts`** — the client:
  - Gutter **column** (`before` decoration, GitLens-style): heat-tinted band +
    emoji + magnitude, accumulated instructions drive the heat tier.
  - **Step Costs pane** (TreeView, `bugstalker.stepCosts`): cursor-driven
    (`focusLine` follows `onDidChangeTextEditorSelection`); rows = instructions,
    cycles, IPC, memory Δ, then **categorisation last** with full summary+hint.
  - `attributeStep` — forward step records cost on the line just executed
    (`prevStop`); reverse step (`stepBack`) pops an undo stack and subtracts.
  - `HoverProvider` (only fires outside a debug session — VS Code reserves the
    hover for the debugger's value eval during one).
  - Tests: `extension/test/perfOverlay.test.ts` (6+ passing).

Relevant step machinery (not perf): `src/debugger/step.rs`
(`step_over_any` ~L351: single-step loop, but **continue-to-temp-breakpoint at
~L478 to step over calls**), `src/debugger/debugee/tracer.rs` `single_step`
(already freezes non-focus threads), `finish_perf_stop` called at
`src/dap/yadap/session/mod.rs:~383`.

## Open questions (revisit these)

### 1. `i = i * 2;` shows as TWO steps (the `* 2`, then the assignment)

Are there really two stops for one source line? Likely the DWARF line table has
multiple `is_stmt` rows (or column entries) for that line — bs stops at each.
**Investigate**: dump the `.debug_line` rows for the test file around that line;
decide whether "next line" should mean next *line* (skip same-line rows) or keep
sub-line stops.

### 2. Pane should be LINE cost with a drill-down LIST of steps

User's words: *"step costs probably wants to be line costs with a list of
steps."* Current mismatch: the **gutter column** shows the line's *accumulated*
instructions (~185k after both sub-steps) while the **pane** shows only the
*last step* (~half). Redesign: a line node = total, expandable to the list of
individual step records (each step's instructions/cycles/cause). `StepCost`
would become a per-line `Vec<StepRecord>` instead of one accumulated blob.

### 3. Residual over-count even with isolation (the big one)

`BS_PERF_ISOLATE_THREAD=1` cut `i = i*2` from **4.4M → ~94k instructions /
186k cycles** — other threads are gone, but it's still ~10⁴× too big (a
multiply is ~1–3 instructions). So there's a *second* source, on the focus
thread / in the window itself. Hypotheses:

- The step-over **continue-over-call** window runs the focus thread through
  debug-build overflow-check / panic-machinery / runtime code, not just the line.
- The rusage window brackets **debugger round-trip** the *debuggee* executes:
  the exception-return trampoline (libsystem), software-breakpoint restore +
  re-arm, stepping over the current breakpoint before the real step
  (`step_over_breakpoint`). Those are real debuggee instructions inside
  `begin_perf_run`→`finish_perf_stop`.
- Per-stop, not per-line: two stops × ~94k each ≈ the 185k line total.

**Next experiments**:

- Tighten the window: snapshot rusage as late as possible before the resume and
  as early as possible after the trap, excluding breakpoint restore / exception
  return.
- Compare a pure single-step (no continue path) vs the continue-over-call path
  on the same line to see which contributes the 94k.
- Sanity-check against a single-threaded, no-overflow-check (`--release` or
  `wrapping_mul`) build — expect the count to collapse toward a handful.
- Optional ground truth: per-thread instruction counter (Linux `perf_event` is
  already per-thread; macOS would need kperf/PMU entitlement).

## Tests (TDD for #3/#1) — now GREEN after (A) shipped

`bugstalker/tests/debugger/perf_cost.rs` (macOS + `perf`-gated) + the
`examples/perf_lines` debuggee pin per-*line* perf cost via `proc_pid_rusage`
deltas across `step_over` — the same window the overlay measures. Run:
`cargo test --features perf --test debugger perf_cost`.

History: started RED on the **raw** counter (`i = i * 2` ≈ 73k instr / 87k
cycles vs ~8 real; `i = i + 1` ≈ 149k = 2× → confirmed #1's double-stop *and*
per-stop overhead). Now the tests assert the **corrected** value (what the pane
shows) via `bs_perf::TrapFloor`, walking trivial lines from L16 to prime the
floor — multiply/add land at single-digit-k (≤10k budget, ≥4× reduction), 8/8
stable. Two `#[ignore]` investigation harnesses remain (`investigate_step_cost_…`
decomposition; `investigate_floor_subtraction` double-trap validation).

### #3 ROOT CAUSE (found — decomposition harness `investigate_step_cost_…`)

`ri_instructions` is **trap-dominated**. Each ptrace trap (Mach exception
deliver → ptrace stop → resume) charges ~**35.5k kernel/IPC instructions** to
the debuggee. Decomposition (arm64 debug, run the `#[ignore]` test):

- 1× `stepi` (1 trap, 1 user instr): **~35.5k** (first is ~42k, warmup), and
  *flat* across repeats to **±0.1%** (35411/35472/35478/35492).
- 1× `continue`-from-bp (2 traps: step-over-current-bp + run-to-next): **~72k**
  = exactly the multiply line; add line = 2 step_overs = ~149k.
- 1× `continue` across a 1e6-iter loop: **~19M** (accurate — user work dwarfs
  the floor). ⇒ rusage is RIGHT for hot regions, useless at step scale
  (SNR ≈ 8 : 35 000).

So it is NOT the capture window (kernel cost happens *during* the resume; can't
bracket out) and NOT the step algorithm. It's the counter. The clean fix — a
**user-mode-only PMU counter** (kperf) that the kernel trap doesn't increment —
is blocked: `crates/bs-perf/src/darwin/kperf.rs` is scaffold-only
(`open_for_pid` → `Unsupported`) and real counters need the Apple-private
`com.apple.private.kpc.read-or-trace` entitlement or root. Fix fork (pick one):

- **(A) Trap-floor subtraction ("double-trap")** — ✅ SHIPPED. `bs_perf::TrapFloor`
  (`crates/bs-perf/src/trap_floor.rs`) learns the per-trap floor *passively* as a
  rolling-min over the user's own steps (no extra traps, no perturbation — the
  adjacent-reference form perturbs the program, so it's test-only). `Debugee`
  counts traps (`trace_until_stop` + the two `single_step` sites in step.rs),
  exposed via `Debugger::trap_count`/`reset_trap_count`; the macOS perf session
  resets at `begin_perf_run`, reads at `finish_perf_stop`, and emits
  `corrected = raw − traps×floor`. Result: multiply **73k → ~0.1–4.5k**, add
  **149k → ~2–5k** (corrected, what the pane shows). Tests assert ≤ 10k +
  ≥4× reduction (`perf_cost.rs`, 8/8 stable). Precision floor is ~`traps × few-k`
  (noisy ±5k) — the cold/warm per-trap spread (~6k) that min-subtraction can't
  remove. Tighten next via per-trap-*type* floors (count single-step vs
  bp-hit separately) or (C).
- **(B) Reframe the metric** — absolute counts only for run-to-cursor/continue
  over regions; fine steps show wall/cpu time + wait-cause + sampled hotspots
  (already built). Simplest, most honest.
- **(C) Chase PMU** — true per-instruction counts. FEASIBILITY PROBED
  (`crates/bs-perf/examples/kperf_probe.rs`, run as user + `sudo`):
  - kperf dylib loads for both; **5 configurable PMU counters** visible on this
    M-series host.
  - `kpc_force_all_ctrs_set(1)`: **EPERM as user, OK as root** → the gate is
    *privilege*, not signing. The `com.apple.private.kpc.read-or-trace`
    entitlement is Apple-internal (not issued to 3rd-party dev certs), so
    signing won't unlock it — **running `bs` as root will**.
  - Remaining unknown before building: **cross-process per-thread attribution**.
    `kpc_get_thread_counters` is self-thread; bs and the debuggee are separate
    processes. Options: (i) per-CPU counters (`kpc_get_cpu_counters`) +
    user-only (EL0) config + the existing thread-freeze isolation → delta ≈
    debuggee user instrs on a quiet box; (ii) read `PMEVCNTRn_EL0` from *inside*
    the debuggee via the existing Mach CallHelper trampoline (we already inject
    calls), needs `PMUSERENR_EL0.EN`; (iii) verify whether `kpc_get_thread_counters`
    honours a foreign tid. With user-only config the kernel trap (EL1) doesn't
    increment, so the floor (B-tier) vanishes → true ~8.
  - PROBE 2 (`crates/bs-perf/examples/kperf_inst_probe.rs`, root): per-thread
    `kpc` FIXED counter (`kpc_get_thread_counters`) is a clean, linear
    instruction count — **8.43 instr/iter stable to ±0.1%** across 100k–10M.
    BUT FIXED counts **user+kernel**: a `close(-1)` loop reads **339 instr/iter**
    (~330 kernel). So reading FIXED around a step includes the ~35k trap → no
    gain over rusage. ⇒ the precise path **requires CONFIGURABLE user-only (EL0)
    events** (KPEP db: `kpep_db_create` / `kpep_config_*` / `kpep_config_kpc` →
    `kpc_set_config`), chip-portable. The easy FIXED route is out.
  - Architecture unchanged: the helper still does a *one-time* privileged config
    (KPEP user-only instead of FIXED-enable); per-step reads unaffected; attack
    surface identical (one narrow PMU-config mandate). Next probe: program
    EL0-only `INST_RETIRED` on a CONFIGURABLE counter, re-run the syscall loop,
    confirm it does NOT inflate (proves EL0-only excludes the kernel trap).
  - PROBE 3 (`crates/bs-perf/examples/kperf_useronly_probe.rs`, root): the full
    KPEP pipeline works (`INST_ALL` event → config word `0x8c` → `kpc_set_config`
    → per-thread read). But the configured counter **still counts kernel**
    (syscall loop ≫ pure-user on the same counter), and the config word `0x8c`
    is a bare event-select with **no EL0/EL1 bits** — user/kernel masking is in
    **PMCR1**, not the KPEP word. ⇒ **user-only is NOT reachable via the clean
    KPEP/`kpc_set_config` path**; it needs RAWPMU PMCR1 bit-banging (undocumented,
    per-chip M1–M4, root, fragile across OS/silicon updates).
  - **DECISION: stop here on PMU.** Beating the rusage floor *requires*
    user-only counting, which *requires* RAWPMU bit-banging — disproportionate
    risk/maintenance for a perf-overlay nicety, and it still needs the root
    helper. The shipped `TrapFloor` (A) already does ~73k→single-k (96%+) with
    zero root/entitlement; **per-trap-*type* floors** are the no-risk way to
    tighten the ±5k residual further. PMU stays documented-but-unbuilt.
    (Probes `kperf_probe` / `kperf_inst_probe` / `kperf_useronly_probe` kept as
    the evidence trail.)

### Deep-research verdict (exact user-mode count — 16 sources, adversarially verified)

- **THE SLEEPER (best route, exact, unprivileged): step-counting + disassembly.**
  Don't measure instructions with a counter at all — *count* them. One ptrace
  single-step == exactly one retired user instruction; the trap cost is excluded
  by construction (you count steps, not a kernel-polluted counter). Exact, zero
  privilege, cross-process by nature (it's our own stepping), works M1–M4 + Linux.
  Complement with **disassembly-static counting** (we already have a
  `Disassembler`) for straight-line spans at zero overhead. Cost: a trap/instr,
  so hybrid — single-step short lines, static-count long/hot spans;
  step-over-call's callee needs static-count or is out of scope. This gives the
  true ~8 and **beats the whole PMU path** for the *count* goal.
- **PMU user-only IS real but heavy + fragile.** `PMCR1_EL1` bits [15:8]=EL0,
  [23:16]=EL1 per counter (PMC0–7; [41:40]/[49:48] for PMC8–9); set EL0-only →
  user-only `INST_RETIRED` (event `0x8c`). Stable M1–M4. BUT: `PMCR0`/`PMCR1`
  are **EL1-only** (Apple-custom sysregs `s3_1_c15_c0/1_0`; PMCs at
  `s3_2_c15_c{0..10}_0`, CRm8 skipped) → **root or kext** to set, and `PMCR0`
  bit30 USER_EN must be set for EL0 `mrs`. macOS re-resets PMCR0/1 from a kernel
  thread ~every 100µs → a kext must re-arm per window. Only buys **cycles**
  (counts can be had by stepping); not worth it for counts alone.
- **Cross-process is structurally blocked on the kpc path:** XNU
  `kpc_get_thread_counters` hard-rejects `tid != 0` (EINVAL). So PMU-for-debuggee
  needs code-injection (inject an `mrs` of `s3_2_c15_*` after USER_EN) or a kext.
- **Confirms PROBE 3:** the kpc config-word EL0/EL1 route was *refuted* 0-3 —
  `kpc_set_config` can't do user-only; masking is PMCR1-only. We weren't missing a flag.
- **Dead ends:** Frida Stalker = software instrumentation, not retired-instr
  counting, no foreign-thread follow (refuted 0-3). Apple Processor Trace (ETM) =
  exact stream but **M4+ only**, Instruments-only, no public API. `rr`'s
  `perf_event_open(exclude_kernel=1, tid)` is **Linux-only** (works on Asahi, not
  macOS) — but it's the model for the Linux backend.

## Implemented this session (step-count + disasm view)

- **Exact instruction count (the real fix for #3).** `Debugger::count_line_instructions(budget) -> LineInstrCount` (`src/debugger/mod.rs`): single-steps until the source line changes (skipping DWARF line-0 glue rows), counting — each step is exactly one retired instruction, trap overhead excluded by construction. Returns `Exact(n)` or `Capped(n)`. **`i = i * 2` measures EXACTLY 9** (`mov/mov/smull/asr/mul/stur/subs/b.ne/b`) vs rusage's ~73k — the whole arc, resolved. Test: `perf_cost::exact_instruction_count_multiply_line`.
- **Step-over-correct exact count.** `Debugger::step_over_or_count(budget)`: runs the counter, but if it descended into a callee (runtime frame-depth check — a debug line's *untaken* overflow-check `bl` correctly does NOT count) or blew budget, recovers step-over semantics (`step_out`+`step_over`) and returns `Capped` (caller uses rusage estimate). Exact for no-call lines, clean fall-back for call lines, lands on the next line either way. Test: `perf_cost::step_over_or_count_exact_then_fallback` (multiply → Exact ~9 lands L18; `busy_loop` call → Capped lands L41).
- **Debug-build detector** (the "pinch of salt" banner). `Debugger::is_likely_debug_build()` — `/target/debug/` path heuristic; the robust signal (arithmetic overflow-check symbols `panic_const_{add,mul,…}_overflow`, debug-only — verified) is the documented refinement. Test: `perf_cost::detects_debug_build`.
- **DAP source-interleaved disassembly (Godbolt-in-debugger, part 1).** `Debugger::source_location_at(addr)` + `handle_disassemble` now attaches `location`+`line` per source-line run → VS Code shows Rust-left / asm-right. Test: `dap_integration::test_disassemble_request` asserts interleaving.
- **DAP instruction-granularity stepping (part 2).** `supportsSteppingGranularity` advertised; `next`/`stepIn` route `granularity:"instruction"` → `Debugger::stepi` (shared `step_one_instruction`). Step one machine instruction in the disasm view.
- **Trap counting plumbing** (from the TrapFloor work): `Debugee`/`Debugger` `trap_count`/`reset_trap_count`.

- **Live overlay wiring (the last mile) — DONE.** When the overlay is enabled,
  `handle_next` routes line-granularity `next` through `step_over_or_count`
  (`STEP_COUNT_BUDGET=4096`); an `Exact(n)` is stashed on the perf session
  (`last_exact_instructions`, with feature-gated `perf_overlay_enabled` /
  `set_perf_exact_instructions` accessors), reset each window in `begin_running`,
  and the macOS `finish_perf_stop` prefers it over `corrected_instructions` for
  `runInstructions`. Overlay-off path is the unchanged `step_over` (no
  regression — DAP `test_next_request` green). Caveat: the override only fires
  when the macOS rusage run set up (`darwin_run` present); if `task_for_pid` is
  denied, no `runInstructions` at all (exact or estimate).
- **Instruction-granularity stepping DAP test — DONE** (`test_next_instruction_granularity`,
  PC-diff ≤ one instruction).

Still pending (decorations): surfaced debug-build banner in the overlay/extension,
per-instruction exec-count overlay, and "debug scaffolding" tags on the
overflow-check/panic instructions in the disasm view. An end-to-end DAP test for
the exact-count `runInstructions` is omitted — it's `darwin_run`/task-port
fragile in CI; covered by the `step_over_or_count` unit test + build-verified
wiring.

## Gotchas / context (saved memories)

- `[[macos-arm64-syscall-from-x16]]` — x16 holds the syscall number for a parked
  thread; basis of the wait classifier.
- `[[bugstalker-perf-overlay-macos]]` — install/activate; empty gutter on
  I/O-bound code is correct; `rusage` `ri_cycles` *does* populate on macOS 13+
  (cycles aren't kperf-gated; only per-line PC-sampled cycles are).
- `[[earthly-build-is-async-use-wait]]` — `+install-darwin` uses WAIT so the
  install RUN doesn't grab a stale `.vsix`.
- `memory Δ` is `phys_footprint` delta — page-granular (16 KB on Apple Silicon)
  and process-wide; not a malloc count (a true count needs an inferior
  `malloc_zone_statistics` call — unbuilt).

## Unsigned commits

Both repos have a run of unsigned `ai`-assisted commits this session; sign with
`git rebase --exec 'git commit --amend --no-edit -S' HEAD~<n> && git push` to
the depth that's still local-only (don't rewrite pushed history).
