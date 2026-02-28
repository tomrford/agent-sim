## Cursor Cloud specific instructions

### Overview

**agent-sim** — firmware simulation runtime (Rust) + Zig DLL template sharing a stable C ABI (`include/sim_api.h`).

Two sub-projects:
- `runtime/` — Rust CLI (Cargo, edition 2024). Currently a scaffold.
- `template/` — Zig 0.15.2 shared-library template that builds `libsim_template.so`.

### Toolchain requirements

- **Rust stable ≥ 1.85** (edition 2024). Update via `rustup update stable && rustup default stable`.
- **Zig 0.15.2** exactly. The project pins this version in `flake.nix` and `build.zig.zon`.

### Build / test / lint

| Task | Command |
|---|---|
| Build Zig DLL | `cd template && zig build` |
| Test Zig template | `cd template && zig build test` |
| Check Zig formatting | `cd template && zig fmt --check src/ build.zig project.zig` |
| Build Rust runtime | `cd runtime && cargo build` |
| Run Rust runtime | `cd runtime && cargo run` |
| Test Rust runtime | `cd runtime && cargo test` |
| Clippy (Rust lint) | `cd runtime && cargo clippy` |
| Rustfmt check | `cd runtime && cargo fmt --check` |

### Notes

- The repo uses **Nix flakes** + direnv locally (`flake.nix`, `.envrc`). Cloud VMs install toolchains directly instead.
- The Zig template DLL output lands in `template/zig-out/lib/libsim_template.so`.
- No external services, databases, or Docker required.
- `include/sim_api.h` is the shared C ABI contract between runtime and template — referenced by both sub-projects.
