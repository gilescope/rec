# CPU-efficiency metrics for the perf overlay — research digest

Deep-research run (2026-06-06, 109 agents, adversarially verified: 13 claims
confirmed, 12 killed). Question: beyond exact instruction count, what CPU-
efficiency signals can BugStalker surface "vaguely deterministically" for a
cross-process ptrace/Mach debugger on Apple Silicon (M1–M4)?

## The one-paragraph answer

Two regimes, hard split. **Per-line (single-step): only static-disassembly
signals are honest** — instruction mix, scalar-vs-SIMD, register-spill, and
expensive-op (div/sqrt/barrier) flagging. 100% deterministic, zero privilege,
work on a single instruction. **Run-regime (continue across a hot loop): IPC**
(exact instructions ÷ trap-corrected `rusage` cycles) is the headline yardstick,
read against a **6.0 ceiling** (Firestorm has 6 integer execution ports).
Everything richer (MPKI, cache-miss rate, 4-quadrant top-down) needs the **kpc
PMU, which requires root to configure** — a hard XNU wall, not a signing quirk.

## Load-bearing findings (confirmed)

- **Root gate is absolute (3-0).** `kpc_force_all_ctrs_set(1)` → EPERM for
  non-root; `kpc_get_thread_counters` is self-thread-only (`EINVAL` for tid≠0).
  Cross-process PMU attribution has **no public-API solution** on macOS. Source:
  XNU `bsd/kern/kern_kpc.c`. ⇒ all PMU-derived run-regime metrics need root or a
  privileged helper. Our cycle source today (`proc_pid_rusage` `ri_cycles`) is
  *not* kpc and works without root — but is trap-polluted, hence TrapFloor.
- **Every readable counter is EL0+EL1 (3-0).** User/kernel split lives in
  `PMCR1` (per-counter `PMCR1_COUNT_A64_EL0/EL1_*`), settable only in-kernel.
  TrapFloor correction is mandatory, not optional. Source: Asahi
  `apple_m1_cpu_pmu.c`.
- **IPC ceiling is 6, not 8 (3-0 for 6 ports; the 8-wide claims were refuted
  0-3 / 1-2).** Firestorm: 6 integer execution ports (X0–X5), 4 SIMD/FP ports
  (V0–V3, FP-divide on V0 only). ALU-bound peak ≈ 6.0 IPC; 8.0 only for
  decode/rename-eliminated NOP/MOV. **Display IPC against 6.0.** Source: Dougall
  Johnson `applecpu`.
- **Integer divide: one port (X4/u5), 1 op / 2 cycles, ≥7-cycle latency (3-0).**
  The slot after a divide is open to non-divide ops, so div density is a clean
  "cost per divide" warning. FP-divide/sqrt similarly single-port. Static,
  per-line, zero privilege.
- **llvm-mca has NO Apple core model (medium).** It would silently use a Cortex
  model and mislead. Any static port-pressure work must use the **applecpu
  CPI/port tables**, not llvm-mca.

## PMU event codes (only usable behind a root helper — park for now)

From `/usr/share/kpep/a14.plist`, cross-checked against the Linux driver
(verified on a live M3 Max). M2/M3/M4 may differ — read `as2/as3/as4.plist` at
runtime via `kpep_db_create` (as ibireme's kpc_demo does).

| event | code | use |
| ----------------------- | ---- | ------------------------ |
| CORE_ACTIVE_CYCLE       | 0x2  | cycles (IPC denominator) |
| INST_ALL                | 0x8c | retired instructions     |
| INST_BRANCH             | 0x8d | branch density           |
| BRANCH_MISPRED_NONSPEC  | 0xcb | branch-MPKI              |
| L1D_CACHE_MISS_LD       | 0xa3 | cache-MPKI               |
| L1D_TLB_MISS            | 0xa1 | dTLB pressure            |
| MAP_STALL, MAP_STALL_DISPATCH | 0x76, 0x70 | backend-bound (top-down approx) |
| MAP_DISPATCH_BUBBLE     | 0xd6 | frontend-bound (top-down approx) |
| RETIRE_UOP              | 0x1  | retiring (top-down approx) |

Branch-mispredict counters are restricted to counter slots 5/6/7 (can't program
all simultaneously). Top-down quadrants are reverse-engineered approximations —
no Apple-published formula.

## Tiered build plan

- **Tier 1 — static-disasm efficiency signals (build first).** Zero privilege,
  deterministic, per-line, and directly answers "missing a trick": instruction
  mix, **scalar-vs-SIMD detection** (sleeper), register-spill detection
  (`ldr/str [sp,#…]` density), expensive-op flags (div/sqrt/barrier). Extends
  the `asmDescribe.ts` classifier already shipped for tooltips.
- **Tier 2 — IPC framing (mostly have it).** Show the existing IPC against a 6.0
  gauge in the run regime; suppress/caveat it per-line where cycles are noise.
- **Tier 3 — static port-pressure ratio (stretch, novel, medium-confidence).**
  applecpu tables → theoretical-min CPI for a block; ratio = measured/floor
  ("3.2× the floor"). Deterministic, zero privilege, but the floor is optimistic
  on real code (ignores dependency chains / memory latency).
- **Tier 4 — PMU top-down / MPKI (future, gated).** Needs a root XPC helper that
  ptrace-attaches and reads counters on BugStalker's behalf. Open question.

## Open questions

- Privileged root helper (XPC/socket) to bridge cross-process kpc attribution —
  the only path to run-regime PMU without `sudo bs`.
- P-core vs E-core ceiling differs (6 vs lower); we can't tell which core ran
  without root affinity control → IPC ratio is ambiguous. `thread_policy_set`
  P-core affinity before the window is the candidate fix (unverified).
- Static port-pressure accuracy on real (non-microbenchmark) code.

Primary sources: dougallj.github.io/applecpu · Asahi `apple_m1_cpu_pmu.c` · XNU
`kern_kpc.c` · ocxtal `insn_bench_aarch64` · ibireme kpc_demo.
