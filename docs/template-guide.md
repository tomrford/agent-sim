# Zig DLL Template Guide

How to create a new firmware sim DLL using the `template/` scaffold.

## Contract

The shared C ABI is defined in `include/sim_api.h`. Every DLL must export:

| Symbol                     | Purpose                      |
| -------------------------- | ---------------------------- |
| `sim_init`                 | Initialize deterministic startup state |
| `sim_reset`                | Reset state to defaults      |
| `sim_tick`                 | Advance one simulation step  |
| `sim_read_val`             | Read a signal value          |
| `sim_write_val`            | Write a signal value         |
| `sim_get_signal_count`     | Number of signals            |
| `sim_get_signals`          | Fill signal descriptor array |
| `sim_get_tick_duration_us` | Tick quantum in microseconds |

Key rules:

- State is singleton per loaded DLL process (no exported context handles).
- Serialize calls into a loaded DLL (not thread-safe).
- Signal IDs/types are discovered at runtime — never hardcode across builds.
- Use `sim_get_tick_duration_us` for the tick quantum; don't assume a fixed value.
- Shared-state channels are dense snapshots:
  - `slot_count` is the full snapshot size
  - valid slot ids are exactly `0 .. slot_count-1`
  - `SimSharedSlot.type` must match `SimSharedSlot.value.type`
  - `sim_shared_read` receives exactly `slot_count` slots in ascending slot order
  - `sim_shared_write` must return exactly `slot_count` slots in ascending slot order

## Optional Flash Export

The template now includes an optional flash preload hook:

- `sim_flash_write`

The runtime calls `sim_flash_write(base_addr, data, len)` before `sim_init()` when
flash regions are configured from CLI or TOML.

### Flash-preserving init/reset rule

Treat flash as non-volatile state:

- `sim_flash_write` updates non-volatile storage
- `sim_init` should reset volatile runtime state only
- `sim_reset` should reset volatile runtime state only

Do **not** zero the whole context inside `sim_init` or `sim_reset` after adding
flash-backed storage, or you'll erase flashed data on boot/reset.

## Files to Edit

| File              | What to change                                        |
| ----------------- | ----------------------------------------------------- |
| `src/adapter.zig` | Init/reset/tick logic, signal catalog, read/write map |
| `project.zig`     | Library name, include paths                           |

## Optional CAN Exports

The template now includes optional CAN hooks:

- `sim_can_get_buses`
- `sim_can_rx`
- `sim_can_tx`

By default, `src/adapter.zig` declares two example buses (`internal`, `external`) and
stub RX/TX handlers. Keep or adapt this pattern if your firmware model needs CAN.
If you don't need CAN, remove or ignore the bus declarations and keep TX empty.

## Optional Shared-State Exports

The template also includes optional shared-state hooks:

- `sim_shared_get_channels`
- `sim_shared_read`
- `sim_shared_write`

The default adapter exposes one channel (`sensor_feed`) with two slots to
demonstrate snapshot-style sharing between instances.

## Files to Keep Stable

| File                | Why                                           |
| ------------------- | --------------------------------------------- |
| `src/root.zig`      | ABI exports, argument/status plumbing         |
| `include/sim_types.zig` | Shared Zig mirror of `sim_api.h` types |
| `build.zig`         | Generic build; reads `project.zig` for config |

## Tick Duration

Set `pub const TickDurationUs` in `adapter.zig`. The runtime reads this via `sim_get_tick_duration_us` and converts user-facing durations (e.g. `1s`) to tick counts automatically.

## Host Loop (runtime perspective)

1. `dlopen` + bind symbols
2. Query tick duration + signal catalog
3. Optional flash preload (`sim_flash_write`)
4. Initialize state (`sim_init`)
5. Per step: write inputs → `sim_tick` → read outputs
6. `dlclose`

## Example

See `examples/hvac/` for a complete thermostat state machine (11 signals, 6 states).
