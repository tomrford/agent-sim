# Agent Prompt: Migrate a Legacy Simulation to Current agent-sim Features

Use this when you want another coding agent to migrate an older simulation DLL to the current `agent-sim` capabilities (flash preload, env wiring, env-owned CAN transport, and shared-state channels).

## Copy/Paste Prompt for the Agent

You are migrating an existing simulation project to current `agent-sim` conventions.

### Goal

Upgrade the legacy simulation so it works with:

1. Base simulation ABI (`sim_*` core exports)
2. Optional flash preload ABI (`sim_flash_write`)
3. Optional CAN ABI (`sim_can_get_buses`, `sim_can_rx`, `sim_can_tx`)
4. Optional shared-state ABI (`sim_shared_get_channels`, `sim_shared_read`, `sim_shared_write`)
5. Runtime env wiring via `agent-sim.toml` (`env start`)
6. Env-owned CAN control via `agent-sim env can ...`

### Constraints

- Follow `include/sim_api.h` exactly.
- Keep deterministic behavior for `sim_init` and `sim_reset`.
- Preserve non-volatile flash contents across `sim_init` / `sim_reset`.
- Do not rename existing externally-consumed signal names unless necessary.
- Do not use signal names prefixed with `can.` (reserved namespace).
- Preserve current simulation behavior unless migration requires explicit adjustment.

### Work Items

1. Inspect the legacy adapter and identify missing exports and data structures.
2. Add/upgrade flash support when the firmware expects preloaded non-volatile data:
   - define flash-backed storage in the adapter context
   - implement `sim_flash_write`
   - keep init/reset from wiping non-volatile state
3. Add/upgrade CAN support:
   - Define `SimCanBusDesc` array (`name`, `id`, bitrates, FD flag).
   - Implement `canRx` and `canTx` behavior (at minimum deterministic stubs that compile and behave correctly).
   - Ensure `canTx` handles `BUFFER_TOO_SMALL` semantics if queueing is used.
4. Add/upgrade shared-state support:
   - Define `SimSharedDesc` channels.
   - Implement `sharedRead`/`sharedWrite` with stable slot IDs and type-safe values.
5. Keep/read/write signal catalog correct:
   - IDs unique and stable.
   - `read`/`write` enforce type checks.
6. Add or update `agent-sim.toml` device/env wiring:
   - `[device.<name>] lib = "..."`
   - optional `flash = [...]`
   - `[env.<name>] instances = [...]`
   - `[env.<name>.can.<bus>] members = [...], vcan = "..."`
   - Optional `dbc = "..."` per bus so `env start` auto-loads DBC.
   - Optional `[env.<name>.shared.<channel>]` with `members` + `writer`.
7. Validate end-to-end with targeted commands and tests.
8. Provide a concise migration report:
   - what changed,
   - what remains stubbed,
   - exact commands used to verify.

### Required Verification Commands

Run from repo root with Nix toolchain:

- `nix develop -c bash -c 'cd runtime && cargo fmt --check'`
- `nix develop -c bash -c 'cd runtime && cargo clippy'`
- `nix develop -c bash -c 'cd runtime && cargo test'`
- `nix develop -c bash -c 'cd template && zig fmt --check src/ build.zig project.zig'`
- `nix develop -c bash -c 'cd template && zig build test'`
- `nix develop -c bash -c 'cd examples/hvac && zig build test'`

If behavior changed, add/adjust focused tests.

---

## Example A: Legacy vs Migrated Adapter Shape

### Legacy (minimal; no CAN/shared)

- Only core `sim_*` behavior.
- No declared CAN buses.
- No shared channels.

### Migrated (shape)

- Add `can_buses`:
  - e.g. `internal` (classic), `external` (FD-capable).
- Add `shared_channels`:
  - e.g. `sensor_feed` with fixed `slot_count`.
- Add deterministic no-op handlers first, then real behavior:
  - `canRx(...)` consumes inbound frames.
  - `canTx(...)` returns pending frames.
  - `sharedRead(...)` applies inbound snapshot.
  - `sharedWrite(...)` publishes outbound snapshot.

---

## Example B: Env Config with VCAN + Auto-DBC Loading

```toml
[env.cluster]
instances = [
  { name = "ecu1", lib = "./zig-out/lib/libecu1.so" },
  { name = "ecu2", lib = "./zig-out/lib/libecu2.so" },
]

[env.cluster.can.internal]
members = ["ecu1:internal", "ecu2:internal"]
vcan = "vcan_internal"
dbc = "./dbc/internal.dbc"

[env.cluster.can.external]
members = ["ecu1:external"]
vcan = "vcan_external"
dbc = "./dbc/external.dbc"

[env.cluster.shared.sensor_feed]
members = ["ecu1:sensor_feed", "ecu2:sensor_feed"]
writer = "ecu1"
```

Notes:

- `dbc` is optional per CAN bus.
- CAN is env-owned; do not model DBC messages as normal `get`/`set` signals.

---

## Example C: Smoke Test Flow

1. Start env:
   - `agent-sim --config ./agent-sim.toml env start cluster`
2. Step env time:
   - `agent-sim env time cluster step 100ms`
3. Confirm env-owned buses:
   - `agent-sim env can cluster buses`
4. Inject a CAN frame:
   - `agent-sim env can cluster send internal 0x123 01020304`
5. Validate shared channel:
   - `agent-sim --instance ecu2 shared get sensor_feed.*`

---

## Handoff Template (Agent Output)

Use this output format:

1. **Summary of migration changes**
2. **Files modified**
3. **Known limitations/stubs left intentionally**
4. **Verification commands + pass/fail**
5. **Follow-up recommendations**
