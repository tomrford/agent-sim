# Next Steps

Exploration items for agent-sim runtime improvements. Each section is independent unless noted.

---

## 1. ipc.rs → interprocess

Replace hand-rolled `ipc.rs` (Unix sockets + Windows named pipes) with the `interprocess` crate.

**Why**: `ipc.rs` reimplements cross-platform local sockets with manual retry logic, Windows pipe rotation, custom Windows name hashing, and stale-endpoint cleanup. `interprocess` gives us a maintained Tokio-ready local-socket layer, but we still need a small compatibility wrapper around naming and cleanup semantics.

**Scope**:
- `LocalListener` / `LocalStream` in `ipc.rs` → `interprocess::local_socket::{tokio::*, ListenerOptions}`
- Keep a thin local wrapper for endpoint naming, stale-endpoint cleanup, and endpoint marker-file behavior
- Prefer `GenericFilePath` for Unix filesystem sockets
- Keep custom Windows endpoint mapping unless we intentionally migrate lifecycle semantics
- JSON-lines protocol on top is unchanged

**GenericNamespaced vs GenericFilePath**:
- `GenericFilePath`
  - Unix: uses a real filesystem path
  - Windows: only works for `\\.\pipe\...` names
  - Best fit if we want to preserve current Unix socket-file behavior
- `GenericNamespaced`
  - Windows: maps cleanly to named pipes
  - Linux: maps to abstract namespace, not a filesystem socket path
  - Other Unices: maps to a generated filesystem path under `/tmp`
  - This changes lifecycle semantics, removes path-level parity, and makes current marker-file/listing behavior more indirect

**Risks**:
- `GenericNamespaced` is not a drop-in replacement for current `socket_path()` semantics
- Preserve current stale-endpoint cleanup behavior; do not blindly replace it with crate defaults
- Preserve Windows marker-file/listing behavior unless lifecycle code is rewritten at the same time
- Check that `interprocess` builds cleanly under `nix develop`

**Recommendation**: use `interprocess` for the transport implementation, but retain a thin wrapper in `ipc.rs`. Full migration to raw crate types is not worth the semantic churn.

**Validation**: existing integration tests (`cli_*.rs`) should pass with zero protocol changes.

---

## 2. Virtual CAN transport

Add a transport backend that behaves like a local CAN bus without requiring Linux SocketCAN or Windows Peak CAN hardware. This is the portability step that unblocks macOS and any environment without `vcan`.

**Why**: current CAN abstraction already has a clean seam: `CanSocket` delegates to `backend::PlatformCanSocket`. Today we support Linux SocketCAN and Windows Peak CAN only. On other targets, CAN is explicitly unavailable.

**Design direction**:
- Keep the current `CanSocket` API: `open()`, `recv_all()`, `send()`
- Extend backend selection from platform-only to transport-kind selection
- Add a new virtual backend that can be used by:
  - env CAN buses
  - direct `can attach`
  - tests
- Keep frame validation and DBC logic above the backend layer

**Recommended shape**:
- Introduce a transport config string instead of overloading `vcan`
  - examples: `socketcan:vcan0`, `peak:usb1`, `virtual:demo-bus`
- Parse that into a small runtime enum
- Keep `SimCanFrame` as the wire type at the `CanSocket` boundary
- Implement the virtual backend first with process-local or host-local IPC semantics

**Agreed implementation approach**:
- keep the current `CanSocket` facade and move backend selection behind it
- stop selecting the CAN backend purely from `target_os`
- dispatch by explicit transport kind instead
- implement `virtual:<bus-name>` first as the simplest host-local named bus with `send()` / `recv_all()` semantics matching the current backends
- reuse existing frame validation, env ownership, and DBC logic above the backend seam
- evaluate `iceoryx2` only if the simple virtual backend proves insufficient for throughput or fan-out

**Backend options**:
- Minimal v1:
  - use a lock-free or mutexed shared queue per bus name
  - only needs `send()` + `recv_all()`
  - good enough for macOS, CI, and local env orchestration
- Shared-memory / pub-sub v2:
  - use `iceoryx2` only if we need higher throughput or multi-process fan-out guarantees
  - this is an optimization path, not the first portability step

**Risks**:
- Config churn: `EnvCanBus.vcan` is Linux-specific naming today
- Bus ownership rules must stay identical across kernel and virtual backends
- Need clear story for cross-process discovery and cleanup of named virtual buses

**Validation**:
- existing env CAN tests should pass on virtual transport
- add backend-agnostic tests that run against both kernel-backed and virtual-backed buses where possible

---

## 3. iceoryx2 — optional env data/event plane

Use `iceoryx2` only if we need a high-throughput zero-copy channel for a new env-level virtual data plane. **Complements** interprocess and the virtual CAN backend; does not replace current command transport.

**Why**: current code does **not** send CAN frames or shared snapshots over JSON request/response:
- CAN already goes through OS CAN transports
- shared-state channels already use mmap-backed snapshots

So the case for `iceoryx2` is narrower:
- virtual CAN bus implementation with zero-copy fan-out
- env-level signal/event streaming
- future high-rate env observability channel

**Layer separation**:
- **Command plane** (interprocess / ipc.rs): Load, Reset, Info, Set, Get, TimeStep — low-frequency, variable-size, JSON fine
- **Data plane** (optional iceoryx2): virtual CAN broadcast or signal/event streaming — high-frequency, fixed-size, zero-copy matters

**What to spike**:
- [ ] Does iceoryx2 build under `nix develop`? Any C/C++ deps or pure Rust?
- [ ] Pub/sub with `SimCanFrameRaw` or another raw payload type — verify `ZeroCopySend` derivation and API ergonomics
- [ ] Process lifecycle: choose service naming (`env/<env>/can/<bus>` or similar)
- [ ] Decide signal-handling policy explicitly if using `Node::wait()`; default crate signal handling should likely be disabled
- [ ] Benchmark against the simplest viable virtual-bus backend before committing to dependency weight

**Risks**:
- Dependency weight and toolchain sensitivity (`cc`, `bindgen`, POSIX-heavy internals)
- iceoryx2 is pre-1.0 (v0.8.1). API may shift. Pin to exact version.

**Recommendation**: do not make `iceoryx2` the first step. First build a backend-selectable virtual CAN transport behind `CanSocket`; only add `iceoryx2` if the simple backend is insufficient.

---

## 4. Signal tracing — CSV capture to file

Runtime command to record signal values to a CSV file, sampled every tick. Replaces the existing `watch` command.

**CLI surface**:
```
agent-sim -i test trace start out.csv              # trace all signals
agent-sim -i test trace stop                       # stop writing; file is complete
agent-sim -i test trace clear                      # discard file, reset state
agent-sim -i test trace status                     # show active file, row count, signals
```

**Behaviour**:
- `trace start` begins sampling immediately. Rows are flushed as they're written — the file is always valid/readable, `stop` just stops appending.
- V1: always traces **all** signals. If per-signal filtering becomes necessary for performance, add it later as optional positional args.
- Signal set is locked for the duration of a trace. No hot-swap. `stop` then `start` with a new file to change.
- CSV only for now. One header row, then one row per tick: `tick,time_us,signal_a,signal_b,...`
- Not a recipe step — runtime CLI command only. Recipes can still `step`/`set` while tracing is active; the trace captures whatever the simulation does.

**Deprecation**: The existing `watch` command (CLI-side polling loop) is superseded by tracing. Remove `watch` and `WatchSamples` only after trace covers its JSON/test contract.

**ABI consideration**: Currently each signal read is a separate FFI call (`sim_read_val` per signal). At high signal counts + fast tick rates this becomes the bottleneck. Consider adding a batch read to `sim_api.h`:
```c
SimStatus sim_read_vals(const SignalId *ids, SimValue *out, uint32_t count);
```
DLL fills `out[0..count]` in one call. Optional export — runtime falls back to per-signal reads if symbol is missing. This benefits tracing and any future bulk-read path.

**Env-level tracing prerequisite**: envd currently has no signal catalog/read API. Add that first.

**Env-level tracing**: When tracing through an env, signal names are qualified with the instance name using colon notation: `upstairs:hvac.current_temp`, `downstairs:hvac.state`. The env daemon coordinates sampling after all instances complete their tick. Single output file per env.

```
agent-sim env trace start my-env out.csv
agent-sim env trace stop my-env
```

**Insertion point**:
- instance: post-`sim_tick()` in `advance_single_project_tick()`, after tick but before CAN/shared TX processing
- env: after all workers complete their tick, before env time advances

**State note**: tracing is daemon/env-owned state, not a CLI polling loop.

---

## 5. Semantic breakpoints — signal triggers

Pause simulation when a signal meets a condition. Single trigger slot (v1), set/arm/disarm via CLI.

**CLI surface**:
```
agent-sim -i test trigger set <signal> <condition> [args]
agent-sim -i test trigger arm
agent-sim -i test trigger disarm
agent-sim -i test trigger show
agent-sim -i test trigger clear
```

**Examples**:
```
agent-sim -i test trigger set hvac.current_temp gt 25.0
agent-sim -i test trigger set hvac.state eq 5
agent-sim -i test trigger set hvac.current_temp rising 22.0
agent-sim -i test trigger set hvac.heater eq false
```

**Condition types**:
| Condition | Args | Fires when |
|-----------|------|------------|
| `eq` | `<value>` | value == threshold |
| `gt` / `lt` | `<value>` | value > / < threshold |
| `gte` / `lte` | `<value>` | value >= / <= threshold |
| `rising` | `<level>` | was ≤ level last tick, now > level |
| `falling` | `<level>` | was ≥ level last tick, now < level |
| `outside` | `<lo> <hi>` | value < lo or value > hi |
| `inside` | `<lo> <hi>` | lo ≤ value ≤ hi |

Edge conditions (`rising`/`falling`) require storing previous-tick value internally.

**Behaviour**:
- Only outcome is **pause** — simulation halts, user inspects state, then resumes.
- `trigger set` replaces any existing trigger and auto-arms. One trigger slot for v1.
- `trigger arm` / `trigger disarm` toggles whether the trigger is evaluated each tick. Useful when re-running a simulation — disarm before reset to avoid re-firing during setup, then arm when ready.
- `trigger show` prints current signal, condition, armed state, and whether it has fired.
- `trigger clear` removes the trigger entirely.
- When a trigger fires during realtime mode, the time engine pauses. During a manual `time step`, the step should return partial progress as a normal result, not as an error path.
- Not a recipe step — CLI command only.

**Env-level trigger prerequisite**: same env signal catalog/read API required by tracing.

**Env-level triggers**: Same colon-qualified signal names as tracing. The env daemon evaluates the trigger after all instances complete their tick, sampling the target signal from the relevant instance.

```
agent-sim env trigger set my-env upstairs:hvac.current_temp gt 25.0
agent-sim env trigger show my-env
```

**Insertion point**: Same post-tick sampling hook as tracing. Tracing (if active) should capture the firing tick before the simulation pauses.

---

## Env-level signal qualification

Both tracing and triggers need to address signals across instances in a multi-device env. Convention:

- **Instance-level**: existing project signal name — `hvac.current_temp`, `hvac.state`
- **Env-level**: `instance:signal` — `upstairs:hvac.current_temp`, `downstairs:hvac.state`
- Glob patterns apply after the colon: `upstairs:*`, `*:state`

This notation extends naturally to any future env-level signal access (get/set/assert).

**Compatibility note**: reserve `:` in runtime signal names so env-qualified selectors remain unambiguous.

---

## Priority order

1. **ipc.rs → interprocess** — lowest risk; keep a thin wrapper for naming/cleanup semantics
2. **Virtual CAN transport** — add backend-selectable virtual bus behind `CanSocket`
3. **Env signal catalog + grouped read API** — prerequisite for env tracing/triggers
4. **Signal tracing** — CSV capture, optional batch-read ABI addition, `watch` removal
5. **Semantic breakpoints** — builds on the same post-tick sampling path as tracing
6. **iceoryx2 spike, if still needed** — only after the virtual backend exists and a simpler implementation has been measured
