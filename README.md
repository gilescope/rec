# Rust Edit-and-Continue

This repository is a coordination checkout for the Rust edit-and-continue
prototype. It keeps the active forks and debugger/editor pieces together as Git
submodules, with the design notes in [edit_and_continue.md](edit_and_continue.md).

## Repositories

| Path | Repository | Purpose | Branch |
| --- | --- | --- | --- |
| `rustc_codegen_cranelift` | `git@github.com:gilescope/rustc_codegen_cranelift.git` | cg_clif edit-and-continue JIT integration | `main` |
| `wasmtime` | `git@github.com:gilescope/wasmtime.git` | `cranelift-hotswap` implementation | `giles-cranelift-hotswap` |
| `linker` | `https://github.com/gilescope/wild` | Wild linker AOT patch emission | `giles-mac` |
| `bugstalker` | `https://github.com/gilescope/BugStalker` | Debugger patch application and reverse/debugger work | `giles-rust-visuals` |
| `vscode-extension` | `https://github.com/gilescope/vscode-lldb.git` | VSCode debugger extension experiments | pinned commit |

`enc-patcher` was an early same-process patching spike and is intentionally not
included as a submodule here.

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
