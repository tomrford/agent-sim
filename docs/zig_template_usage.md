# Zig Template Usage

Template root: `template/`.

Files to edit per project:

- `src/adapter.zig`: project init/reset/tick and signal mapping.
- `project.zig`: output library name + include paths + source modules.
- `include/wrapper.h`: host-side stubs/macros needed by firmware C build.

Keep stable across projects:

- `src/root.zig` (ABI exports + generic argument/status handling).
- `src/sim_types.zig` (ABI type mirror).

Tick duration:

- Template exposes tick via `adapter.TickDurationUs`.
- This is surfaced through `sim_get_tick_duration_us`.
- Runtime must call API; do not infer from project docs only.
