# Zig DLL Template Guide

How to create a new firmware sim DLL using the `template/` scaffold.

## Contract

The shared C ABI is defined in `include/sim_api.h`. Every DLL must export:

| Symbol                     | Purpose                      |
| -------------------------- | ---------------------------- |
| `sim_new`                  | Allocate a new `SimCtx*`     |
| `sim_free`                 | Destroy a context            |
| `sim_reset`                | Reset context to defaults    |
| `sim_tick`                 | Advance one simulation step  |
| `sim_read_val`             | Read a signal value          |
| `sim_write_val`            | Write a signal value         |
| `sim_get_signal_count`     | Number of signals            |
| `sim_get_signals`          | Fill signal descriptor array |
| `sim_get_tick_duration_us` | Tick quantum in microseconds |

Key rules:

- `SimCtx*` is opaque. One context = one simulated device.
- Serialize all calls per context (not thread-safe).
- Signal IDs/types are discovered at runtime — never hardcode across builds.
- Use `sim_get_tick_duration_us` for the tick quantum; don't assume a fixed value.

## Files to Edit

| File              | What to change                                        |
| ----------------- | ----------------------------------------------------- |
| `src/adapter.zig` | Init/reset/tick logic, signal catalog, read/write map |
| `project.zig`     | Library name, include paths                           |

## Files to Keep Stable

| File                | Why                                           |
| ------------------- | --------------------------------------------- |
| `src/root.zig`      | ABI exports, argument/status plumbing         |
| `src/sim_types.zig` | Zig mirror of `sim_api.h` types               |
| `build.zig`         | Generic build; reads `project.zig` for config |

## Tick Duration

Set `pub const TickDurationUs` in `adapter.zig`. The runtime reads this via `sim_get_tick_duration_us` and converts user-facing durations (e.g. `1s`) to tick counts automatically.

## Host Loop (runtime perspective)

1. `dlopen` + bind symbols
2. Query tick duration + signal catalog
3. Create one or more contexts (`sim_new`)
4. Per step: write inputs → `sim_tick` → read outputs
5. `sim_free` contexts, `dlclose`

## Example

See `examples/hvac/` for a complete thermostat state machine (11 signals, 6 states).
