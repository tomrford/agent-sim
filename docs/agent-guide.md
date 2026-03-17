# Agent Guide

Use this when an autonomous agent needs to build, wire, and verify a DLL against the current `agent-sim` runtime.

## Fast mental model

- One loaded DLL = one running **instance**.
- The ABI contract lives in `include/sim_api.h`.
- The shared Zig ABI mirror lives in `include/sim_types.zig`.
- The runtime requires an exact `sim_get_api_version()` match before loading a DLL.
- CAN and shared-state are optional, but if you export any symbol from an optional surface you must export the full surface.
- Shared-state channels are **dense snapshots**, not sparse maps.
- Batch signal reads are supported via optional `sim_read_vals`; runtime falls back to single-value reads when absent.

## Files that usually matter

- `include/sim_api.h` — C ABI contract
- `include/sim_types.zig` — Zig ABI mirror
- `template/src/adapter.zig` — example adapter implementation
- `template/src/root.zig` — exported ABI entry points
- `examples/hvac/` — complete worked example
- `examples/hvac/agent-sim.toml` — current config/recipe reference

## Current shared-state rules

For each shared channel:

- `slot_count` is the full snapshot size.
- Valid slot ids are exactly `0 .. slot_count-1`.
- `SimSharedSlot.type` must equal `SimSharedSlot.value.type`.
- `sim_shared_read` receives exactly `slot_count` slots in ascending slot order.
- `sim_shared_write` must return exactly `slot_count` slots in ascending slot order.
- If `sim_shared_write` gets a buffer smaller than `slot_count`, it may write the dense prefix, return `SIM_ERR_BUFFER_TOO_SMALL`, and the host will retry with the full buffer size.

Do not rely on “unset”, “default”, or sparse shared slots. They are not part of the current contract.

## Recommended implementation flow

1. Start from `template/`.
2. Define one enum-backed signal catalog in `src/adapter.zig`.
3. Keep `read`/`write` logic aligned with that catalog.
4. If flash is needed:
   - implement `sim_flash_write`
   - keep non-volatile state separate from reset/init logic
5. If CAN is needed:
   - declare buses
   - validate frame behavior against `sim_api.h`
6. If shared-state is needed:
   - declare channels
   - implement dense snapshot read/write behavior
7. Add/update `agent-sim.toml` wiring.
8. Run the focused verification commands below.

## Verification commands

Run from repo root.

### Runtime

```sh
nix develop -c bash -c 'cd runtime && cargo fmt --check'
nix develop -c bash -c 'cd runtime && cargo clippy'
nix develop -c bash -c 'cd runtime && cargo test'
```

### Template / example DLLs

```sh
nix develop -c bash -c 'cd template && zig build'
nix develop -c bash -c 'cd template && zig build test'
nix develop -c bash -c 'cd template && zig fmt --check src/ build.zig project.zig'

nix develop -c bash -c 'cd examples/hvac && zig build'
nix develop -c bash -c 'cd examples/hvac && zig build test'
```

### Packaged binary

```sh
nix build
```

## Transport notes

- Linux CAN transport uses SocketCAN interface names.
- Windows CAN transport uses Peak CAN channel names such as `usb1` or `pci1`.
- The current Windows Peak CAN backend includes common classic/FDCAN bitrate profiles and fails early on unsupported bitrate pairs instead of guessing.

## Useful smoke tests

```sh
# Load a standalone instance
agent-sim --instance demo load ./template/zig-out/lib/libsim_template.so
agent-sim --instance demo signals
agent-sim --instance demo set demo.input 4.0
agent-sim --instance demo time step 20us
agent-sim --instance demo get demo.output

# Start an env from config
agent-sim --config ./examples/hvac/agent-sim.toml env start single-node
agent-sim env status single-node
agent-sim env time single-node step 100ms
agent-sim close --env single-node
```

## Pitfalls to avoid

- Do not invent compatibility shims for old ABI behavior.
- Do not use signal names prefixed with `can.`.
- Do not let `sim_init` / `sim_reset` erase flash-backed state.
- Do not hand-maintain multiple Zig ABI mirrors.
- Do not return ambiguous shared-state payloads; return a full dense snapshot.
