# Runtime Expectations (for future runtime repo)

Required DLL symbol set:

- `sim_new`, `sim_free`
- `sim_reset`, `sim_tick`
- `sim_read_val`, `sim_write_val`
- `sim_get_signal_count`, `sim_get_signals`
- `sim_get_tick_duration_us`

Runtime responsibilities:

- Symbol binding + ABI safety checks.
- Context lifecycle management.
- Signal metadata cache per loaded DLL.
- Deterministic step scheduling from reported tick duration.
- Error conversion/status propagation.
