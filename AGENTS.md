# agent-sim

CLI tool for programmatic SIL testing of firmware. Zig compiles firmware as a shared library; a Rust runtime CLI loads and interacts with the simulation over IPC.

## Toolchain

Nix manages all toolchains (`flake.nix`). Prefix commands with `nix develop -c`.

```sh
nix develop -c cargo build   # Rust
nix develop -c zig build     # Zig
```

## Sub-projects

- `runtime/` — Rust CLI + daemon (Cargo, edition 2024).
- `template/` — Zig 0.15.2 shared-library scaffold.
- `examples/hvac/` — HVAC thermostat example DLL (11 signals, 6 states). Has `agent-sim.toml` with example recipes.
- `include/sim_api.h` — shared C ABI contract.

## Build / test / lint

All from workspace root:

| Task               | Command                                                                              |
| ------------------ | ------------------------------------------------------------------------------------ |
| Build Rust runtime | `nix develop -c bash -c 'cd runtime && cargo build'`                                 |
| Test Rust runtime  | `nix develop -c bash -c 'cd runtime && cargo test'`                                  |
| Clippy             | `nix develop -c bash -c 'cd runtime && cargo clippy'`                                |
| Rustfmt check      | `nix develop -c bash -c 'cd runtime && cargo fmt --check'`                           |
| Build Zig template | `nix develop -c bash -c 'cd template && zig build'`                                  |
| Test Zig template  | `nix develop -c bash -c 'cd template && zig build test'`                             |
| Zig fmt check      | `nix develop -c bash -c 'cd template && zig fmt --check src/ build.zig project.zig'` |
| Build HVAC example | `nix develop -c bash -c 'cd examples/hvac && zig build'`                             |
| Test HVAC example  | `nix develop -c bash -c 'cd examples/hvac && zig build test'`                        |
| Build nix package  | `nix build`                                                                          |

## Output paths

- Template DLL: `template/zig-out/lib/libsim_template.{so,dylib}`
- HVAC example DLL: `examples/hvac/zig-out/lib/libsim_hvac_example.{so,dylib}`
- Runtime binary: `runtime/target/debug/agent-sim` (dev) or `nix build` → `result/bin/agent-sim`

## Key docs

- `docs/template-guide.md` — how to create a new DLL from the template.
- `examples/hvac/agent-sim.toml` — recipe reference for the HVAC example.

## Notes

- No external services, databases, or Docker required.
- `include/sim_api.h` is the C ABI contract referenced by both Rust and Zig.
- The flake exposes `packages.default` = `agent-sim` (Rust CLI built via `buildRustPackage`).
