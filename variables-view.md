# Variables View — Design & User Reference

> Status: design. Not yet implemented.
> Started: 2026-05-28.
> Spans repos: `bugstalker` (DAP server + DWARF walker), the VSCode debugger extension (renderer).
> Sister doc: [edit_and_continue.md](edit_and_continue.md).

## 0. Why

Today the variables pane is the usual `name: Type = value` list, with one
piece of Rust-specific enrichment: a 🔒/🔑/☠️ glyph on `Mutex<T>` to show
whether the lock is held (`bugstalker/src/debugger/variable/render.rs:1550`).
That single glyph carries more *runtime* signal per character than anything
else in the pane.

This document generalises that idea into a coherent visual vocabulary so
that, at a glance, the variables pane tells you:

1. The **current runtime access state** of any synchronisation wrapper —
   not just `Mutex` (sharing-mode glyph).
2. Whether each binding is **writable through this name** (mutability).
3. **Where the binding lives** — stack, register, static, TLS,
   optimised-away (storage-class glyph).
4. The **health of the current stack frame** — its size, recursion depth,
   how close the thread is to overflow.
5. How much of each value's memory footprint is **real payload vs padding
   slack** (payload/padding split-bar).

All five derive from data the debugger already has or can recover cheaply
from DWARF + ELF program headers + CFI we parse for unwinding. None of
them needs a new low-level subsystem.

## 1. What the user sees

### 1.1 Sharing-mode glyph — wrapper runtime state

A trailing glyph on the rendered value of any synchronisation wrapper,
reflecting the wrapper's **current** runtime state — not its static type.

| glyph | meaning                                                  | where used                         |
|-------|----------------------------------------------------------|------------------------------------|
| 🔒    | exclusive access in effect — someone is holding it       | `Mutex` locked, `RwLock` write-held, `RefCell` `borrow_mut` held |
| 🔑    | free — nobody holds it, anyone may take it               | `Mutex` unlocked, `RwLock` free, `RefCell` idle |
| 👥`N` | shared by N readers                                      | `RwLock` with N readers, `RefCell` with N immutable borrows |
| ⚛    | interlocked / atomic — every access is hardware-exclusive | `AtomicU32`, `AtomicPtr`, etc.    |
| ☠️    | poisoned — held by a thread that panicked                | any lock                           |

Rendered example:

```
counter: Mutex<u64>     = 🔒 42                  // someone has the lock
shared:  RwLock<Vec<…>> = 👥3 [1, 2, 3, …]       // three readers
broken:  Mutex<State>   = 🔑 State { … } ☠️       // free but poisoned
flag:    AtomicBool     = ⚛ true
```

Lock-state detection works on the futex backend (Linux, FreeBSD) and
Darwin pthread (`os_unfair_lock` owner probe). Win7 SRWLOCK still
reports `locked=false` unconditionally — known limitation.

### 1.2 Mutability — row background colour (hue)

The **hue** of each row's background indicates whether the binding grants
write access through normal Rust code.

| hue    | meaning                                       | examples                                  |
|--------|-----------------------------------------------|-------------------------------------------|
| grey   | read-only through this binding                | `&T`, `static FOO: u32`, `let x` of a plain owned type |
| orange | writable through this binding                 | `&mut T`, `static mut FOO`, `let mut x`, any type with interior mutability (`Cell`, `RefCell`, `Mutex`, `Atomic*`) |
| none   | unknown / not classifiable                    | optimised-away bindings                   |

The hue is the **only** thing mutability controls. Lightness within that
hue is used by the payload/padding split in §1.5, so the two channels
multiplex into a single row background.

For **statics**, the classifier is the segment's `PF_W` bit from the ELF
program headers — the linker has already encoded "could this ever be
mutated through normal code" by placing the address in `.rodata` (read-only)
vs `.data` / `.bss` (writable). This is more accurate than parsing
`static` vs `static mut` from DWARF: `static FOO: AtomicU32 = …` is
non-mut in source but lands in `.data` because it *is* mutable via atomic
ops — and the row correctly shows orange.

For **locals and arguments**, we use the binding's type: shared
references are grey, mutable references are orange, anything containing
`UnsafeCell` (interior mutability) is orange.

`const` items are not in the variables pane at all — the compiler inlines
them, so there is no binding and no address to show. The clue is in the name.

### 1.3 Storage class — leading glyph

A tiny glyph at the start of each row indicating where the binding lives.

| glyph | storage class       | source                                                |
|-------|---------------------|-------------------------------------------------------|
| ⬛    | stack               | `DW_OP_fbreg <off>` or `DW_OP_breg7/6` (frame-pointer-relative) |
| 🟦    | register            | `DW_OP_reg<N>` / `DW_OP_regx` — register name on hover |
| ⬜    | static, read-only   | `DW_OP_addr` in a `PT_LOAD` segment with `PF_W` clear |
| 🟧    | static, writable    | `DW_OP_addr` in a `PT_LOAD` segment with `PF_W` set   |
| 🟣    | thread-local        | `DW_OP_form_tls_address` / `GNU_push_tls_address`     |
| 👻    | optimised away      | `DW_OP_implicit_value` or no location at this PC      |
| ↗     | (overlay) points into heap | binding's pointee address ∈ `[heap]` / anon RW mapping in `/proc/PID/maps` |

The `↗` is an overlay — it stacks on top of any of the other glyphs to
say "the binding itself is e.g. on the stack, but the value it points
to is on the heap." This is how `Box<T>`, `Vec<T>`, `String`, etc.
typically render: `⬛↗ buf: Vec<u8> = …`.

### 1.4 Scopes — coarse grouping in the pane

Currently the DAP server exposes two scopes: `Locals` and `Arguments`.
This expands to four:

- **Arguments** — function parameters (as today).
- **Locals** — `let` bindings (as today), with stack/register
  differentiated per-row via the §1.3 glyph.
- **Statics** — new. File-scope `DW_TAG_variable` DIEs whose parent is
  not a `DW_TAG_subprogram`. Default filter: "statics defined in the
  user's own crate(s) for the current frame" — otherwise the pane is
  flooded with `std::io::stdio::STDOUT_INSTANCE` and friends. A setting
  widens to "current compilation unit" or "everything".
- **Thread-locals** — new. Variables matched via the existing TLS
  detection path (`bugstalker/src/debugger/debugee/dwarf/mod.rs:1107`),
  currently buried inside the locals walk.

### 1.5 Payload/padding — row background lightness split

The row background is split horizontally into two regions of the **same
hue** (the one set by §1.2 mutability), differing only in HSL **lightness**:

- **Left** — proportion that is real payload. Lightness at the hue's base.
- **Right** — proportion that is padding or alignment slack. Lightness
  shifted by `Δ` (default ±15%) so the padding portion reads as "faded /
  recessed" relative to the payload portion.

Using HSL keeps the colour the same — grey stays grey, orange stays
orange — and lets a single hue carry both signals. Hue answers "can this
be written?"; the dark/light split within that hue answers "how much
of this memory is doing real work?"

**Direction of the shift** is theme-dependent so the padding side always
appears to recede *towards* the panel background:

- **Light theme**: padding lightness **+ Δ** (pushed towards white).
- **Dark theme**: padding lightness **− Δ** (pushed towards black).

For a struct, payload = sum of members' `DW_AT_byte_size`. Padding =
the type's `DW_AT_byte_size` minus that sum (interior + trailing).

For enums, payload = the active variant's size, waste = enum size −
variant size − discriminant (zero when niche optimisations apply, e.g.
`Option<&T>`).

For primitives, slices, and arrays: zero padding, solid background at
the base lightness.

Rendered example (ASCII approximation — actual rendering is HSL on the
row background):

```
24-byte Foo with 10 payload + 14 padding (58% waste), orange hue (RW):

  ████████████░░░░░░░░░░░░░░░░░░░░       row background
  └─ payload ─┘└──── padding ─────┘
   base L       L ± Δ (theme-dep)
```

Thresholds:

- **Show the split only when waste ≥ `showThresholdPct`** (default 5%).
  Below that, the difference is noise and the background stays solid at
  base lightness.
- **Severity is read from the *width* of the padding region**, not from
  hue or shade. We don't paint the padding amber or red because hue is
  already taken; the proportion is enough signal on its own. A user who
  wants categorical severity colouring can opt in via
  `paddingSplit.severityHueShift` (default off — see §4).

Tooltip on hover gives the breakdown: `24 bytes total · 10 payload · 14 padding (58%)`.

> **Note**: we considered a "deep waste" mode that transitively sums
> padding in nested structs. Deferred — the shallow number is the
> actionable one ("reorder this struct's fields"); deep waste is
> harder to act on and easy to misread.

### 1.6 Stack health hints

A small status pill at the top of the variables pane, summarising the
current frame and thread stack budget.

```
frame: 12 KB · 3% of 2 MB thread stack          ✓ healthy
frame: 768 KB · 38% of 2 MB thread stack        ⚠ large
frame: 1.4 MB · 68% of 2 MB thread stack        ⛔ near overflow
                                                [rec ×7]  ← if applicable
```

Per-local hotspot signals:

- Each local's byte size shown in a faded trailing column:
  `buf: [u8; 65536] = […]  (64 KB)`.
- Tinge **amber** when an individual local exceeds 1 KB.
- Tinge **red** + show hint marker `→ consider Box<…>` when it
  exceeds 16 KB.
- A `[rec N]` tag on the frame name when the same function appears
  N ≥ 10 times further up the stack.

A separate **Stack hotspots** panel (command palette: "Show stack hotspots")
gives a sortable view across all frames in the current thread:

```
┌──────────────────────────────────┬───────┬──────┬───────────┐
│ frame                            │ size  │ %    │ note      │
├──────────────────────────────────┼───────┼──────┼───────────┤
│ parser::parse_json               │ 768K  │ 38%  │ ⚠ huge    │
│ ┊  table: [Slot; 16384] = …      │ 768K  │      │ → Box<>?  │
│ codec::decode_inner              │  64K  │  3%  │           │
│ codec::decode                    │  16K  │  1%  │ [rec 7]   │
└──────────────────────────────────┴───────┴──────┴───────────┘
                                    total: 64% of 2 MB
```

All thresholds (25% / 50% / 1 KB / 16 KB / recursion ×10) are settings-
configurable and shipped as defaults — they are *hints*, not diagnoses.

## 2. Visual vocabulary at a glance

Compact reference card for the eventual `vscode-extension` README.

| element              | position             | meaning                                          |
|----------------------|----------------------|--------------------------------------------------|
| background **hue**   | row background       | mutability (grey = RO, orange = RW)              |
| background **lightness split** | row background, L/R | payload (left, base L) vs padding (right, L ± Δ) — same hue |
| ⬛ 🟦 ⬜ 🟧 🟣 👻      | leading glyph        | storage class                                    |
| ↗                    | overlay on storage   | binding's value points into heap                 |
| `name: Type`         | row text             | binding name and DWARF type                      |
| `= value`            | row text             | rendered value                                   |
| 🔒 🔑 👥`N` ⚛ ☠️      | trailing on value    | wrapper runtime access state                     |
| size column          | trailing, faded      | byte size; amber > 1 KB; red > 16 KB             |
| header pill          | top of pane          | frame size, thread budget, recursion             |

## 3. How the five compose — worked examples

**Example A**: a writable static `Mutex<Vec<i32>>` currently free, in a
non-stressed frame, struct has 0% padding.

```
🟧  COUNTER: Mutex<Vec<i32>> = 🔑 [1, 2, 3]
    └ solid orange row ──────────────────────┘
```

The 🟧 glyph says "writable static". The row background is **orange**
(the mutex's interior mutability makes the binding effectively
writable). Solid — no lightness split because no padding to flag. The
🔑 says "lock currently free". The size column shows the byte cost of
the Mutex+Vec headers.

**Example B**: a stack local `Foo { a: u8, b: u64, c: u8 }`.

```
⬛  foo: Foo = Foo { a: 1, b: 2, c: 3 }
    └ orange (base L) ──┘└ orange (L±Δ) ──┘
    └ payload 42% ──────┘└ padding 58% ───┘
```

The ⬛ says "stack". The row background is **orange** (owned, normally
writable). The background is split: ~42% of the width at the base
lightness (payload), ~58% at lightness shifted by Δ (padding). Tooltip
on hover: "24 bytes · 10 payload · 14 padding (58%) — try reordering
fields".

**Example C**: a register-resident loop counter.

```
🟦  i: usize = 7
    └ solid orange row ──┘
```

🟦 (register `rax` on hover). Background **orange** (`let mut i`),
solid — primitive has zero padding.

**Example D**: a Box that points into the heap.

```
⬛↗  tree: Box<Node> = Box(Node { children: […] })
     └ orange row (split depends on Node's payload/padding) ────────┘
```

⬛ for the stack pointer itself, ↗ overlay for "the pointee is on the
heap". Background orange (owned mutable through this name). The
lightness split applies to the `Node` payload, not the box header.

## 4. Configuration

All thresholds and toggles live under `bugstalker.variablesView.*` in
the VSCode extension settings (and as REPL `set` keys in the bare
`bs` driver).

| setting                                          | default | meaning                                 |
|--------------------------------------------------|---------|-----------------------------------------|
| `showSharingGlyph`                               | `true`  | 🔒/🔑/👥/⚛/☠️ on wrappers              |
| `showMutabilityBackground`                       | `true`  | row background hue (grey vs orange)     |
| `showStorageGlyph`                               | `true`  | ⬛/🟦/⬜/🟧/🟣/👻 leading                |
| `showHeapOverlay`                                | `true`  | ↗ overlay (one `/proc/maps` read/step)  |
| `showPaddingSplit`                               | `true`  | HSL lightness split within the hue      |
| `paddingSplit.showThresholdPct`                  | `5`     | hide split below this waste %           |
| `paddingSplit.lightnessDeltaPct`                 | `15`    | Δ in HSL L between payload and padding  |
| `paddingSplit.severityHueShift`                  | `false` | opt-in: shift padding hue towards red as waste %  rises (off by default — proportion is the signal) |
| `mutability.hue.readOnly`                        | theme   | HSL of the RO background (overridable)  |
| `mutability.hue.readWrite`                       | theme   | HSL of the RW background (overridable)  |
| `stackHealth.show`                               | `true`  | header pill + per-local size column     |
| `stackHealth.frameAmberPctOfStack`               | `25`    |                                         |
| `stackHealth.frameRedPctOfStack`                 | `50`    |                                         |
| `stackHealth.localAmberBytes`                    | `1024`  |                                         |
| `stackHealth.localRedBytes`                      | `16384` |                                         |
| `stackHealth.recursionDepthThreshold`            | `10`    |                                         |
| `statics.scope`                                  | `crate` | `crate` / `unit` / `all`                |

## 5. Design notes & implementation plan

### 5.1 Sharing-mode glyph (extend existing Mutex path)

- **Signal source**: existing `SpecializedValue` peeling. `Mutex` already
  exposes `locked` / `poisoned`. Extend to `RwLock` (read count from its
  `Futex` field, writer-bit from the same), `RefCell` (the `borrow: Cell<isize>`
  field is right there — negative ⇒ `borrow_mut`, positive ⇒ N readers,
  zero ⇒ idle).
- **Where**: `bugstalker/src/debugger/variable/value/specialization/mod.rs`
  and `bugstalker/src/debugger/variable/render.rs:1529`.
- **Cost**: small. Mostly mirrors the existing `Mutex` path.

### 5.2 Mutability background hue

- **Signal source — statics**: address → segment via ELF program headers
  (`object::ObjectSegment::flags()`), check `PF_W` bit. One parse of the
  object file, sorted ranges, binary-search lookup.
- **Signal source — locals/args**: DWARF type inspection. `&T` vs
  `&mut T` is preserved (reference type with/without const qualifier);
  interior mutability is detected by recursive scan for `UnsafeCell`.
  Whether rustc preserves `let mut` vs `let` for owned locals at the
  binding level is a TODO — needs an `objdump --dwarf=info` probe on a
  test binary. If it doesn't, owned locals default to orange (writable),
  which is the conservative answer.
- **Where**: new segment-index helper in
  `bugstalker/src/debugger/debugee/registry.rs` (alongside the existing
  `object::Object` parse); classifier in the variable rendering path.
- **DAP**: emit `presentationHint.attributes = ["readOnly"]` for grey
  rows (stock DAP clients render this as italics) **and** a custom
  field `bugstalker.mutability = "ro" | "rw" | "unknown"`. The
  vscode-extension reads the latter and paints the row background.

### 5.3 Storage-class glyph

- **Signal source**: the `DW_OP_*` head of each variable's location
  expression. We already evaluate this when reading the value; classifying
  it into one of the six buckets is a `match` on the same DIE.
- **Heap overlay**: one `/proc/PID/maps` parse per stop, sorted by
  address, binary-search the pointee. macOS equivalent: `mach_vm_region`
  loop. Cache invalidated on every continue.
- **DAP**: emit via `presentationHint.kind` (`"data"` / `"property"`)
  plus a custom field `bugstalker.storage = "stack" | "register" | ...`
  consumed by our extension.

### 5.4 Statics + thread-locals as new scopes

- **DWARF traversal**: walk each compilation unit collecting
  `DW_TAG_variable` nodes whose parent is not `DW_TAG_subprogram`.
  Filter by the configured scope (`crate` / `unit` / `all`).
- **New `Debugger::read_statics()`** alongside the existing
  `read_local_variables` / `read_argument` (`bugstalker/src/debugger/mod.rs:1425`).
- **New DAP scopes** in `frame.rs:178` — add `Statics` and `Thread-locals`
  to the vector, populated via matching `read_*` helpers in
  `bugstalker/src/dap/yadap/session/data.rs`.

### 5.5 Stack health

- **Per-frame size**: `CFA(this) − CFA(parent)` from CFI we already parse
  for unwinding. Topmost frame: `CFA − current_sp`.
- **Thread stack budget**: read `/proc/PID/task/TID/maps` (Linux),
  `vm_region_recurse_64` for the `MEM_STACK` mapping (macOS); cache per
  thread. Used = `mapping.end − current_sp`.
- **Recursion**: count `fn_name` repeats in the unwound stack.
- **Per-local size**: `DW_AT_byte_size` on the type, already resolved.
- **Where (DAP)**: new event `bugstalker.stackHealth` emitted on each
  `stopped`, consumed by the extension to paint the header pill.

### 5.6 Payload/padding HSL lightness split

- **Shallow padding**: for struct/enum types, `type.byte_size − Σ(members.byte_size)`.
  Enums: `type.byte_size − active_variant.byte_size`. Both numbers are
  in DWARF.
- **Where**: new helper `compute_padding(&Type) -> PaddingBreakdown`
  in the variable rendering layer, called for each rendered row.
- **DAP**: custom field `bugstalker.layout = { totalBytes, payloadBytes, paddingBytes }`
  per variable. The extension takes the hue from §5.2's
  `bugstalker.mutability` field, parses it as HSL, applies `±Δ` (theme-
  dependent) to the lightness for the padding portion, and renders the
  row background as `linear-gradient(to right, base 0%, base
  payloadPct%, shifted payloadPct%, shifted 100%)` (a hard stop —
  not a true gradient, so the boundary is crisp).
- **HSL maths**: both base colours are stored as HSL triples in the
  extension's theme config (`mutability.hue.readOnly`,
  `mutability.hue.readWrite`). Computing the padding shade is one
  clamp on the L channel: `shifted = clamp(base.l ± Δ, 0, 100)`.
  Hue and saturation are untouched, which is why the colour stays
  recognisably "the same orange / the same grey".

### 5.7 Channel multiplexing — hue × lightness

The two row-background features (mutability §5.2 and payload/padding
§5.6) are multiplexed onto a single visual channel rather than fighting
for it:

- **Hue** = mutability (one bit: RO grey, RW orange).
- **Lightness** = payload/padding split (continuous: position of the
  L-step encodes the payload proportion).

This is the user's design and is genuinely better than either of the
earlier proposals (background-for-mutability + separate-band-for-padding,
or background-split-for-padding + stripe-for-mutability). One channel
carries two orthogonal signals because hue and lightness are
perceptually independent dimensions in HSL. The colour stays
recognisably "the same orange / the same grey" regardless of how much
slack the type carries.

Saturation is held in reserve — currently unused, but available as a
third dimension if a future signal wants in (one idea: saturation
encodes confidence in the classification — desaturated when the
debugger isn't certain about the mutability call).

## 6. Implementation order

Recommended landing sequence — each item is independently shippable:

1. **§5.1** — extend sharing-mode glyph to `RwLock` and `RefCell`.
   ✅ Landed on `giles-variables-view` (bugstalker). Adds a `LockState`
   enum (`Free` / `Exclusive` / `Shared(N)`), decodes RwLock's futex
   state per libstd's `MASK = (1 << 30) - 1` for accurate reader
   counts, and reads `RefCell::borrow: Cell<BorrowFlag>` to surface
   the same glyph vocabulary. `🔑` / `🔒` / `👥N` (saturating at
   `👥9+`) plus `☠️` poison trailer. 8 new unit tests + the existing
   `test_read_mutex_rwlock` integration test now asserts the
   `Shared(1)` case for `rwl.read()`. macOS pthread probe still
   handles Mutex only; RwLock on macOS reports `Free` (TODO).

2. **§5.4** — `Statics` + `Thread-locals` as new DAP scopes.
   ✅ Landed on `giles-variables-view`. Parser now classifies each
   `DW_TAG_variable` as file-scope or local via a `subprogram_offsets`
   set + `parent_index` ancestor walk; TLS internals
   (`__KEY` / `VAL` / `__RUST_STD_INTERNAL_VAL`) are unconditionally
   file-scope even when rustc nests them under closures.
   `DqeExecutor::query_file_scope(kind, filter)` enumerates with
   `FileScopeKind` (Statics vs ThreadLocals) and `FileScopeFilter`
   (`CurrentCrate` / `CurrentUnit` / `All`).
   `Debugger::read_static_variables` and `read_thread_local_variables`
   expose the API; DAP `handle_scopes` adds `Statics` and
   `Thread-locals` to its scope vector. Default filter is
   `CurrentCrate` (crate-root namespace match) so the pane doesn't
   flood with std / dep statics. 1 new unit test + 2 integration tests
   (`test_bulk_enumerate_statics`, `test_bulk_enumerate_thread_locals`)
   exercise the path on a live debuggee. Known limit: TLS internals
   whose runtime slot isn't initialised on the current thread are
   silently dropped at value-parse time — see §7.

6. **§5.6** — payload/padding HSL split.
   ✅ Landed on `giles-variables-view`. Bugstalker side only —
   the HSL rendering itself is vscode-extension work (see §7).
   * New `LayoutBreakdown { total, payload, padding }` struct on
     `QueryResult` with `padding_pct()` that saturates at 100%.
   * `QueryResult::layout()` shallow-classifies struct types:
     `total = type.byte_size`, `payload = Σ(members.byte_size)`,
     `padding = total − payload`. Returns `None` for non-struct
     types (primitives / arrays / pointers / enums — enums
     deferred per the shallow-only v0 scope).
   * DAP emits `bugstalker.layout = { totalBytes, payloadBytes,
     paddingBytes }` per top-level row, but **only** when
     `padding_pct ≥ 5` (the `showThresholdPct` from §4) — below
     that the HSL split is visual noise and gets suppressed.
   * 4 unit tests for the `padding_pct` arithmetic + 1 live-
     debuggee integration test pinning the structural identity
     `payload + padding == total` and `payload == 20` for the
     fixture's `Foo { i32, [i32;2], &i32 }` (sum is rustc-
     reorder-invariant, total isn't).

5. **§5.5** — stack health pill + per-local size column.
   ✅ Landed on `giles-variables-view`. v0 ships three of four
   intended signals; the fourth (per-frame size from CFA) is
   deferred:
   * **Per-local byte size** — new `QueryResult::byte_size()`
     resolves `DW_AT_byte_size` via the existing
     `ComplexType::type_size_in_bytes` path. Emitted as
     `bugstalker.byte_size` per top-level row. Drives the amber
     (>1 KB) / red (>16 KB) tinting in the extension's trailing
     size column.
   * **Thread stack budget** — new `DwarfRegistry::containing_range(addr)`
     returns the mapping containing an address. The new
     `stack_health` module reads the SP register, looks up the
     containing mapping (the thread stack — `[stack]` for main,
     anon-rw for spawned threads), and computes `total` /
     `used` / `used_pct`. `used_pct` saturates at 100 so
     guard-page-mid-overflow shows as 100%.
   * **Recursion count** — per-frame tag `bugstalker.recursionCount`
     when the function name appears ≥ 2 times in the backtrace;
     aggregated `max_recursion` in the stack-health snapshot.
   * **DAP emit** — `handle_stack_trace` attaches
     `bugstalker.stackHealth` (frameCount / maxRecursion /
     threadStackSize / threadStackUsed / threadStackUsedPct) to
     the response body alongside the existing `stackFrames`.
     Computed once per stack-trace request from the already-
     unwound backtrace + one register read + one proc_maps lookup.
   * 4 unit tests for the `used_pct` arithmetic (saturation,
     div-by-zero, none-propagation) + 1 live-debuggee integration
     test asserting `thread_stack_size`/`used` invariants and
     pinning `a: i32 → byte_size == Some(4)`.
   * **Deferred for v0**: per-frame size from CFA (needs
     extending `FrameSpan` with `cfa` and computing
     `CFA(this) − CFA(parent)` for each frame, plus
     `CFA(top) − current_sp` for the topmost). Mentioned in §7.

4. **§5.3** — storage-class glyph (landed after §5.2 below).
   ✅ Landed on `giles-variables-view`. Two enrichments to existing
   infrastructure plus a new classifier module:
   * `DwarfRegistry` now also indexes mapping **kind** (`Static`,
     `Stack`, `Heap`, `AnonRw`, `Other`) alongside writability,
     populated from `proc_maps` filenames (`[stack]` / `[heap]` /
     real paths / `None`). Lookup is the same O(log N) binary
     search.
   * New `variable::storage` module: `StorageClass` enum
     (Stack / Register / StaticReadOnly / StaticReadWrite /
     ThreadLocal / OptimizedAway / Unknown) plus `classify(...)`
     that walks the raw `DW_AT_location` expression. Source classifier
     is the first significant opcode (DW_OP_fbreg / reg / addr / TLS /
     implicit_value); for Address-producing opcodes it cross-
     references the evaluated address with the segment-kind +
     segment-writability indexes to split Static into RO/RW.
   * Heap overlay (`value_points_to_heap`) extracts the pointee
     address from Box/Rc/Arc/NonNull/Weak/raw-pointer values and
     checks whether it lands in `[heap]` or anon-RW.
   * New `FatDieRef<Variable>::location_expression(pc)` + `unit_encoding()`
     accessors expose the raw expression to the classifier without
     leaking the `DwarfLocation` wrapper.
   * `QueryResult` carries `Option<StorageClass>`; `apply_select_die`
     and `query_file_scope` populate it during enumeration.
   * DAP `handle_variables` emits `bugstalker.storage = "stack" |
     "register" | "static_ro" | "static_rw" | "tls" | "optimized" |
     "unknown"` and `bugstalker.points_to_heap = true` per top-level
     row (the latter omitted when false to keep JSON tight).
   * 2 unit tests for string vocab stability + 1 live-debuggee
     integration test pinning `static GLOB_2` → StaticReadOnly,
     `box_d` binding → Stack. Heap-overlay assertion is currently
     lenient — see §7 known limit on index-refresh.

3. **§5.2** — mutability background hue.
   ✅ Landed on `giles-variables-view`. Two-tier classifier:
   * `DwarfRegistry::address_writability(addr)` performs O(log N)
     binary search over a sorted PT_LOAD index (built from
     `proc_maps` in `update_mappings`) — the loader's PF_W bit is
     ground truth for statics + TLS. Index covers every mapping
     (incl. `[heap]` / `[stack]` / anon-mmap) so §5.3 can reuse it.
   * `mutability::classify_by_type` inspects the DWARF type for
     locals/args: `&T` → ReadOnly (with `CModifier::Const` on the
     reference target as the distinguishing signal), `&mut T` → ReadWrite,
     any type containing `UnsafeCell` → ReadWrite (recursive scan
     with cycle-breaking visited set), owned default → ReadWrite
     (per the rustc let-mut DWARF gap settled in §8).
   * DAP emits both the standard `presentationHint.attributes =
     ["readOnly"]` (so stock clients italicise) AND the custom
     `bugstalker.mutability = "ro" | "rw"` field for the
     vscode-extension to read.
   * 6 segment-index unit tests + 2 classifier unit tests +
     1 live-debuggee integration test (`test_mutability_classifier_runs_on_live_variables`,
     pinning `static GLOB_2: i32 = 2` to ReadOnly via the segment
     lookup since it's always `.rodata`).
   * Child-row mutability hint is currently `None` (only top-level
     rows get the hint). Per-field inheritance from parent is a
     follow-up.
2. **§5.4** — `Statics` + `Thread-locals` scopes. Adds the new DWARF
   walk and gives users new things to look at in the pane.
3. **§5.2** — mutability stripe. Builds the segment-index helper that
   §5.3 also reuses.
4. **§5.3** — storage-class glyph. Reuses the segment index and the
   already-evaluated location expressions.
5. **§5.5** — stack health pill + per-local size column. Independent
   of the others; arguably the highest practical value for users
   debugging perf-sensitive code.
6. **§5.6** — payload/padding split. Last because it's the most
   visually invasive and needs all the prior pieces in place to
   show its layered effect.

The **Stack hotspots** panel and the **deep padding** mode are
deferred follow-ups.

## 7. Known limitations

Things that work *less well* than the design promises but are
acknowledged + survivable. Each one is a candidate for follow-up
work after the main plan lands. Tagged with the feature they
belong to.

### Platform / backend gaps

- **[§5.1, Win7] SRWLOCK lock state.** `Mutex` / `RwLock` on Win7
  always report `LockState::Free` regardless of actual state. The
  SRWLOCK internal state field is undocumented; reading it would
  need reverse-engineered offsets. Deferred.
- **[§5.1, macOS] RwLock state.** macOS pthread RwLock probe is
  not implemented (only `pthread_mutex_t` has the owner-field
  probe). Falls through to `LockState::Free`. The
  `test_read_mutex_rwlock` integration test is platform-gated so
  the Linux assertion of `Shared(1)` still holds and macOS
  asserts `Free` instead.

### Value-parse fallthroughs

- **[§5.4] TLS bulk-enumeration value-parse drop.** Non-const-init
  `thread_local!`s have a runtime-allocated slot that isn't
  initialised until the owning thread first touches it. When the
  `Thread-locals` scope is opened on a thread that hasn't yet
  initialised a given TLS, `root_from_die` returns `None` and the
  entry is silently dropped. Const-init thread_locals
  (`thread_local! { static FOO: i32 = const { 42 }; }`) always
  parse. **Follow-up:** surface unreadable entries with an
  `<unavailable: TLS not yet initialised on this thread>`
  placeholder so the name still appears.

### Deferred features

- **[§5.6] vscode-extension HSL row-background renderer.** The
  bugstalker DAP server emits `bugstalker.layout = { totalBytes,
  payloadBytes, paddingBytes }` per top-level row when padding
  ≥ 5%. The actual two-shade HSL background rendering (payload
  at base lightness, padding at `base ± Δ`) is vscode-extension
  CSS work — not yet started. Same situation as the §5.2 row-
  background renderer.
- **[§5.6] enum layout breakdown.** v0 of `QueryResult::layout()`
  only classifies `TypeDeclaration::Structure`. Enum padding
  (total − active_variant_size − discriminant) is the design's
  promised second case; deferred because rustc niche-encoded
  enums need their active variant identified at runtime, which
  needs a couple of extra DWARF traversal steps. Structs alone
  cover most "is this row wasting space" cases.

- **[§5.5] per-frame size from CFA.** The intended fourth signal
  in the stack-health pill (alongside thread budget, recursion,
  per-local size) is per-frame size: `CFA(this) − CFA(parent)`
  for each frame, with `CFA(top) − current_sp` for the topmost.
  Data is in the unwinder (each `UnwindContext` carries a `cfa`);
  exposing it needs extending `FrameSpan` with `Option<cfa>` and
  populating from the loop. Deferred for v0 — the other three
  signals already give the user actionable information.

### Index-refresh gaps

- ~~**[§5.3] segment-writability index is startup-only.**~~ ✅
  Resolved on `giles-variables-view`. New
  `DwarfRegistry::refresh_segment_index()` re-reads `proc_maps`
  and rebuilds only the segment index (separate from the heavier
  `update_mappings` which also touches per-file DWARF mappings).
  Wired into DAP `handle_scopes` so it runs before every
  variables-pane query — cheap (one /proc/PID/maps read), and
  catches post-startup `Box::new` / `Vec::with_capacity` /
  spawned-thread stacks. The integration test
  `test_storage_classifier_runs_on_live_variables` now asserts
  `box_d.value points to heap` hard (was lenient pre-fix).

### Deferred plumbing

- **[§5.4] launch-args scope filter.** `variablesView.statics.scope`
  is hardcoded to `CurrentCrate` in the DAP `read_statics` /
  `read_thread_locals` helpers. The internal
  `DqeExecutor::query_file_scope` API already accepts all three
  modes (`crate` / `unit` / `all`); only the DAP-side glue that
  reads the client config is missing.
- **[§5.2] vscode-extension row-background renderer.** The
  bugstalker DAP server emits `bugstalker.mutability = "ro" | "rw"`
  per top-level variable. Stock DAP clients render the standard
  `presentationHint.attributes = ["readOnly"]` as italics, but the
  full grey/orange HSL row background (per §1.2 + §1.5) requires
  the vscode-extension to read the custom field and apply CSS.
  Extension-side work not yet started.
- **[§5.2] Child-row mutability propagation.** Per-field expansion
  rows (struct members, array indices, deref targets) currently
  emit no mutability hint — only top-level rows do. A field of a
  `ro` parent is conceptually `ro` for the user's purposes; the
  classifier needs to thread the parent's classification through
  `value_children`.

### Render-format choices to revisit

- **[§5.6] Hard L-step vs soft gradient.** The payload/padding
  split is rendered as a `linear-gradient` with a hard stop at the
  payload boundary so the proportion is readable at a glance. A
  soft gradient (10% transition band) would feel more organic but
  smudges the percentage signal. Going with hard for v0; revisit
  if it looks harsh in practice.

## 8. Open questions

Things we don't yet know the answer to. Once answered, they
either flip to a Known limitation or get incorporated into the
design.

- ~~**`let mut` preservation in DWARF.**~~ ✅ Settled 2026-05-28 by
  `objdump --dwarf=info` on a probe binary with both `let a: i64` and
  `let mut b: i64`. rustc emits identical DIEs for the two — no
  `DW_AT_mutable`, no const-qualifier, nothing. The borrow checker
  erases. Owned locals will default to orange (writable) in §5.2 as
  the conservative answer.
- **[§5.2] Dark vs light theme HSL palette.** Need tested HSL
  triples for the two mutability hues (grey, orange) in both
  themes, plus the lightness delta `Δ` per theme. Light-theme
  orange wants higher L (~70%) so the padding-side shift towards
  white still reads as "less"; dark-theme orange wants lower L
  (~28%) so the padding-side shift towards black doesn't become
  unreadable. Colour-blind-safe variants for grey/orange to be
  picked from existing VSCode palette tokens. Stack-health amber
  and red (per-local size colouring) live in a separate palette
  and don't interact with the mutability hues.
- **[§5.3] TLS storage glyph 🟣.** Thread-locals are surfaced as
  their own scope (§5.4 ✅), so the leading-row 🟣 glyph is
  arguably redundant — the scope label already says "Thread-locals".
  Decide whether to keep it (consistent vocabulary, useful when a
  TLS shows up in DAP custom views that flatten scopes) or drop
  it (no longer carries new info on top of the scope).
- **[§5.5] `Stack hotspots` cost on deep stacks.** Sustained CFI
  lookups across ~100s of frames — probably fine since CFI is
  already parsed for unwinding, but measure before exposing the
  panel as always-on.
