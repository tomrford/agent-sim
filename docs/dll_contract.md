# DLL Contract Notes

Primary contract: `include/sim_api.h`.

Key points for runtime implementers:

- Use `sim_get_tick_duration_us` to determine simulation quantum. Do not assume fixed value.
- Discover signal IDs/types at runtime (`sim_get_signal_count` + `sim_get_signals`).
- Never hardcode signal IDs across builds unless project explicitly freezes them.
- Treat `SimCtx*` as opaque handle; one ctx per simulated device instance.
- Serialize calls per ctx (no concurrent use of same ctx).
- Use explicit status-code handling on every call.

Recommended host loop:

1. Load DLL, bind symbols.
2. Query tick duration + signal catalog.
3. Create one or more contexts.
4. For each simulation step:
   - write inputs (zero or more)
   - `sim_tick`
   - read outputs (zero or more)
5. Free contexts, unload DLL.
