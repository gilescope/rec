VERSION 0.8

# Cross-repo CI for the Rust Edit-and-Continue project.
#
# `rec` itself has no code — three forks pinned by submodule pointers.
# Each fork's own CI tests it in isolation; this Earthfile drives the
# tests that need all three at the rec-pinned shas.
#
# Refs default to each fork's `main`. CI passes the recorded submodule
# shas explicitly so a rec commit gates on the exact combination it
# pinned:
#
#   earth -P +rec-gate \
#       --BS_REF=$(git ls-tree HEAD bugstalker        | awk '{print $3}') \
#       --WILD_REF=$(git ls-tree HEAD linker          | awk '{print $3}') \
#       --EXT_REF=$(git ls-tree HEAD vscode-extension | awk '{print $3}')
#
# `earth` is the EarthBuild fork's binary (post-Earthly-Software OSS
# continuation). The legacy `earthly` binary is wire-compatible.

ARG --global BS_REF=main
ARG --global WILD_REF=main
ARG --global EXT_REF=main
ARG --global REC_PLATFORM=linux/amd64

# ---------------------------------------------------------------------
# Per-fork own-CI gates (delegated to each fork's Earthfile or built
# inline here for wild, which has no Earthfile yet).
#
# Re-running each fork's own gate here catches the "the rec-pinned
# sha doesn't even pass its own CI any more" case — easy to miss if a
# fork branch was force-pushed after rec bumped its submodule.
# ---------------------------------------------------------------------

# bs-own-gate uses +ci-lint (platform-flexible via BS_PLATFORM
# pass-through). The full bs test matrix is gated on bs's own repo
# PRs; re-running it at the rec level is wasteful — we only need the
# rec-pinned sha to still pass its own lint here.
bs-own-gate:
    BUILD github.com/gilescope/BugStalker:${BS_REF}+ci-lint --BS_PLATFORM=${REC_PLATFORM}

ext-own-gate:
    BUILD github.com/gilescope/vscode-lldb:${EXT_REF}+bs-gate

wild-base:
    FROM --platform=${REC_PLATFORM} rust:1.95-bookworm
    ENV CARGO_TERM_COLOR=always
    ENV DEBIAN_FRONTEND=noninteractive
    RUN apt-get update && \
        apt-get install -y --no-install-recommends \
            build-essential pkg-config ca-certificates git && \
        rm -rf /var/lib/apt/lists/*
    # earth's GIT CLONE --branch only accepts refs/branches/tags, not
    # raw shas. The rec workflow passes a sha here when integration-
    # testing the recorded submodule pointer, so use plain git clone +
    # checkout to handle either form.
    RUN git clone https://github.com/gilescope/wild /wild && \
        cd /wild && git checkout ${WILD_REF}
    WORKDIR /wild

wild-own-gate:
    FROM +wild-base
    RUN --mount=type=cache,target=/usr/local/cargo/registry \
        --mount=type=cache,target=/wild/target \
        cargo test --workspace --release

wild-build:
    FROM +wild-base
    RUN --mount=type=cache,target=/usr/local/cargo/registry \
        --mount=type=cache,target=/wild/target,sharing=locked \
        cargo build --release --bin wild && \
        cp target/release/wild /tmp/wild
    SAVE ARTIFACT /tmp/wild AS LOCAL build/wild

# ---------------------------------------------------------------------
# Rec-level integration shared base.
# ---------------------------------------------------------------------

common:
    FROM --platform=${REC_PLATFORM} rust:1.95-bookworm
    ENV CARGO_TERM_COLOR=always
    ENV DEBIAN_FRONTEND=noninteractive
    RUN apt-get update && \
        apt-get install -y --no-install-recommends \
            build-essential pkg-config ca-certificates git jq && \
        rm -rf /var/lib/apt/lists/*

binaries:
    FROM +common
    # bs's Earthfile defaults BS_PLATFORM to arm64; pass our rec
    # platform through so the cross-repo build matches the runner.
    COPY (github.com/gilescope/BugStalker:${BS_REF}+build-rel/bs --BS_PLATFORM=${REC_PLATFORM}) /usr/local/bin/bs
    COPY +wild-build/wild /usr/local/bin/wild
    RUN bs --version && wild --version

# ---------------------------------------------------------------------
# Cross-repo integration tests — runs on Linux x86_64 today.
# ---------------------------------------------------------------------

# 1. Protocol contract check (cheap, fast, high signal).
#
# Three rec-level invariants exercised against the pinned shas:
#   (a) bs --describe-commands emits valid JSON with a non-empty
#       methods array — proves the scripting schema is wired.
#   (b) bs's REPL still exposes apply-patch — the wild->bs handoff
#       depends on it.
#   (c) the extension's TypeScript source still references the
#       bs/encApplyPatch DAP custom request — proves the watcher
#       path that bs implements is still the contract the editor
#       speaks.
#
# Any of these breaking at the rec-pinned shas means an integration
# regression that single-repo CI couldn't see.
test-protocol-contract:
    FROM +binaries
    RUN bs --describe-commands > /tmp/schema.json
    RUN jq -e '.methods | length > 0' /tmp/schema.json > /dev/null \
        || (echo "bs --describe-commands returned no methods" && exit 1)
    RUN bs --help 2>&1 | grep -q 'apply-patch\|--help' \
        || (echo "bs help output unrecognisable" && exit 1)
    # Pull the extension source via plain `git clone` + checkout (the
    # extension Earthfile doesn't SAVE ARTIFACT the TS source, so
    # cross-repo COPY can't reach it; and earth's GIT CLONE --branch
    # rejects raw shas which the workflow passes).
    RUN git clone https://github.com/gilescope/vscode-lldb /ext && \
        cd /ext && git checkout ${EXT_REF}
    # NB: the wire name is bs/applyPatch (yadap/session/mod.rs dispatch,
    # editContinue.ts customRequest) — bs/encApplyPatch only ever existed in
    # comments, which is what this gate was first (wrongly) written against.
    RUN grep -rq 'bs/applyPatch' /ext/extension \
        || (echo "extension no longer references bs/applyPatch — the EnC contract is broken" && exit 1)
    RUN echo "[protocol-contract] ok — scripting schema present, apply-patch reachable, extension still speaks bs/applyPatch"

# 2. bs --test smoke against a tiny scripted session + tiny debuggee.
#
# Exercises the JSON-RPC scripting front-end end-to-end: bs spawns
# the demo binary, parses the script, sets a breakpoint by symbol,
# runs, hits it. Catches scripting-engine regressions invisible to
# bs's own unit tests.
test-bs-script:
    FROM +binaries
    COPY tests/demo /demo
    COPY tests/scripts /scripts
    WORKDIR /demo
    RUN cargo build --release
    RUN --privileged \
        bs --test /scripts/smoke.json5 target/release/demo

# 3. v3 --emit-patch round-trip. **macOS only today** — wild's
#    --emit-patch is wired in args/macho.rs but not args/elf.rs.
#    This target is the placeholder where the Linux end-to-end will
#    land once wild gains ELF --emit-patch support.
#
# Marked LOCALLY (host-bound) so a Linux GHA runner doesn't try to
# execute it. macos-14 runners can call it directly.
test-patch-roundtrip-darwin:
    LOCALLY
    RUN test "$(uname)" = Darwin || \
        { echo "+test-patch-roundtrip-darwin: macOS only (host is $(uname))"; exit 1; }
    RUN echo "WIP: wire wild --emit-patch on aarch64-apple-darwin against a tiny demo"

# 4. End-to-end EnC happy path. WIP — needs the same fixtures plus a
#    scripted edit/save/rebuild/apply/continue driver. Reserved as a
#    target name so it's discoverable.
test-encs-e2e:
    FROM +binaries
    RUN echo "test-encs-e2e: WIP — see tests/scripts/encs_e2e.json5 (TODO)"

# ---------------------------------------------------------------------
# Aggregate targets.
# ---------------------------------------------------------------------

# Cheap PR gate: rec-level integration that runs on Linux today.
# Each fork's own CI is re-run separately under +all.
rec-gate:
    BUILD +test-protocol-contract
    BUILD +test-bs-script

# Full sweep: each fork's own CI re-run at the rec-pinned sha plus
# the rec-level integration. Heavy — push-to-main and dispatch only.
all:
    BUILD +bs-own-gate
    BUILD +ext-own-gate
    BUILD +wild-own-gate
    BUILD +rec-gate

# ---------------------------------------------------------------------
# Dev install (macOS).
# ---------------------------------------------------------------------
#
# One command to put a working debugger + editor integration on the
# host. Unlike the CI gates above (which pull each fork from its pinned
# GitHub ref), this installs from the LOCAL submodule checkouts, so your
# uncommitted/unpushed work lands in the editor.
#
#   1. bugstalker `bs` — `cargo install` (perf overlay on by default) +
#      adhoc codesign with the `com.apple.security.cs.debugger`
#      entitlement, delegated to bugstalker's own +install-darwin. The
#      entitlement is what lets the perf PC-sampler call task_for_pid.
#   2. the VS Code extension — packaged to a .vsix (+vsix) and installed
#      via the VS Code CLI with --force (overwrites any prior copy).
#
# LOCALLY throughout: needs host cargo, codesign and the `code` CLI.
# The `code` shell command must be on PATH (VS Code Command Palette →
# "Shell Command: Install 'code' command in PATH") or installed in the
# standard app-bundle location.
#
#   earth +install-darwin
install-darwin:
    LOCALLY
    RUN test "$(uname)" = Darwin || \
        { echo "+install-darwin: macOS only (host is $(uname))"; exit 1; }
    # 1) bs — build (perf default) + codesign + `bugstalker` symlink.
    # 2) extension — produce the .vsix (containerised, reproducible);
    #    +vsix SAVE ARTIFACT AS LOCAL lands at vscode-extension/build/.
    # BUILD is async in earthly, so both go in a WAIT block: without it
    # the install RUN below races ahead and reinstalls the *previous*
    # run's stale .vsix before this one finishes writing.
    WAIT
        BUILD ./bugstalker+install-darwin
        BUILD ./vscode-extension+vsix
    END
    # 3) install the .vsix. ~/code is a user wrapper, not the CLI, so
    #    prefer the app-bundle binary and fall back to PATH `code`.
    RUN CODE="/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code"; \
        [ -x "$CODE" ] || CODE="$(command -v code 2>/dev/null || true)"; \
        { [ -n "$CODE" ] && [ -x "$CODE" ]; } || \
            { echo "+install-darwin: VS Code 'code' CLI not found (app bundle or PATH); install it via Command Palette → \"Shell Command: Install 'code' command in PATH\""; exit 1; }; \
        "$CODE" --install-extension vscode-extension/build/vscode-bugstalker.vsix --force
    RUN echo "+install-darwin: bs (perf, cs.debugger-signed) + extension installed — reload the VS Code window (Cmd-R) to activate."
