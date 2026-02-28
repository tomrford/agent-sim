## Cursor Cloud specific instructions

### Overview

**agent-sim** — firmware simulation runtime (Rust) + Zig DLL template sharing a stable C ABI (`include/sim_api.h`).

Two sub-projects:
- `runtime/` — Rust CLI (Cargo, edition 2024). Currently a scaffold.
- `template/` — Zig 0.15.2 shared-library template that builds `libsim_template.so`.

### Toolchains via Nix

All toolchains (Rust, Zig, clippy, rustfmt, rust-analyzer) are provided by `flake.nix`. Prefix commands with `nix develop -c` to use the pinned versions:

```sh
nix develop -c cargo build
nix develop -c zig build
```

The nix daemon must be running (`sudo /nix/var/nix/profiles/default/bin/nix-daemon &`) and PATH must include `/nix/var/nix/profiles/default/bin`.

### Build / test / lint

All commands run from the workspace root via `nix develop -c`:

| Task | Command |
|---|---|
| Build Zig DLL | `nix develop -c bash -c 'cd template && zig build'` |
| Test Zig template | `nix develop -c bash -c 'cd template && zig build test'` |
| Check Zig formatting | `nix develop -c bash -c 'cd template && zig fmt --check src/ build.zig project.zig'` |
| Build Rust runtime | `nix develop -c bash -c 'cd runtime && cargo build'` |
| Run Rust runtime | `nix develop -c bash -c 'cd runtime && cargo run'` |
| Test Rust runtime | `nix develop -c bash -c 'cd runtime && cargo test'` |
| Clippy (Rust lint) | `nix develop -c bash -c 'cd runtime && cargo clippy'` |
| Rustfmt check | `nix develop -c bash -c 'cd runtime && cargo fmt --check'` |

### Notes

- The Zig template DLL output lands in `template/zig-out/lib/libsim_template.so`.
- No external services, databases, or Docker required.
- `include/sim_api.h` is the shared C ABI contract between runtime and template — referenced by both sub-projects.
- `flake.nix` line 28 references `./Cargo.toml` (root) but the file lives at `runtime/Cargo.toml`. The `cargoToml` binding is unused by the devShell so it doesn't error — but it would break if any output actually consumed it.
