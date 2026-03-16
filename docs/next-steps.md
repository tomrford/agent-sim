# Next Steps

Exploration items for agent-sim runtime improvements. Each section is independent unless noted.

---

## 1. ipc.rs → interprocess

Replace hand-rolled `ipc.rs` (Unix sockets + Windows named pipes) with the `interprocess` crate.

**Why**: `ipc.rs` currently owns a lot of platform plumbing that is not product-specific:

- Tokio Unix socket binding/connecting
- Windows named-pipe listener rotation after each accept
- Windows busy-pipe retry logic
- endpoint-name dispatch across platforms

`interprocess` gives us a maintained Tokio-ready local-socket layer. We still need a small compatibility wrapper around endpoint naming, stale-endpoint cleanup, and marker-file behavior because those are part of our runtime lifecycle contract, not just transport mechanics.

**Current code snapshot**:

- `runtime/src/ipc.rs` is about 311 LoC total, about 242 LoC before tests
- about 140-160 LoC are Windows-specific pipe setup/rotation/retry/hash plumbing that `interprocess` should replace directly
- the remaining code is mostly the compatibility layer we still want to keep: endpoint cleanup, marker files, bind retry policy, and tests

**What we actually gain**:

- fewer platform edge-cases owned in-tree, especially Windows named-pipe lifecycle details
- one maintained abstraction for Tokio local sockets instead of separate Unix/Windows implementations
- simpler future maintenance if `interprocess` improves platform support or bug fixes
- less custom retry/rotation code to reason about when debugging connection failures

**Expected code-size effect**:

- expected post-migration `ipc.rs` size: roughly 140-180 LoC total including compatibility helpers/tests
- expected net reduction: roughly 80-120 LoC in `ipc.rs` alone
- this is not a dramatic shrink; the real gain is deleting bespoke platform behavior we currently have to trust and maintain ourselves

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

**Recommended implementation shape**:

1. Add `interprocess` and migrate `LocalListener`/`connect()` to Tokio local sockets created through `ListenerOptions` and the Tokio stream type.
2. Keep `ipc.rs` as the public wrapper module. Callers should continue to use:
   - `bind_listener(endpoint)`
   - `connect(endpoint)`
   - `cleanup_endpoint(endpoint)`
   - `create_endpoint_marker(endpoint)`
3. Add one helper that converts runtime endpoint `Path` -> transport name:
   - Unix: use `GenericFilePath` with the real socket path
   - Windows: preserve current stable hash mapping from endpoint path to `\\.\pipe\agent-sim-...`, then hand that full pipe name to `interprocess`
4. Preserve current bind behavior:
   - first bind attempt as normal
   - on bind conflict, try connecting
   - if connect fails, treat it as stale and clean up the endpoint marker/socket path
   - bind again
5. Keep crate overwrite behavior conservative:
   - on Unix, `reclaim_name` on drop is fine
   - do not rely on crate-level overwrite behavior to replace our stale-endpoint policy
   - on Windows, continue to own lifecycle semantics via the marker file and stable name mapping
6. Leave `BoxedLocalStream` and the JSON-lines framing unchanged so daemon/envd/worker code does not notice the swap

**Risks**:

- `GenericNamespaced` is not a drop-in replacement for current `socket_path()` semantics
- Preserve current stale-endpoint cleanup behavior; do not blindly replace it with crate defaults
- Preserve Windows marker-file/listing behavior unless lifecycle code is rewritten at the same time
- Check that `interprocess` builds cleanly under `nix develop`

**Recommendation**: use `interprocess` for the transport implementation, but retain a thin wrapper in `ipc.rs`. Full migration to raw crate types is not worth the semantic churn.

**Validation**:

- existing integration tests (`cli_*.rs`) should pass with zero protocol changes
- env bootstrap/worker tests that fake instance connections should pass unchanged
- manual smoke: load instance, send repeated requests, start env, close env, restart same instance/env names without leftover endpoint failures

---

## 2. Env signal catalog + grouped read API

Add a first-class env-level signal catalog and grouped read path. This is the substrate for env tracing and env breakpoints, and it is also the right place to introduce optional batch signal reads in the DLL ABI.

**Why**: the instance daemon already has the pieces we need:

- `InstanceAction::Signals` returns per-instance signal metadata
- `InstanceAction::Get` / `Sample` return per-instance values
- `Project` already owns the signal catalog and read path

But the env daemon currently has none of that:

- env bootstrap fetches instance `Info` and worker CAN buses, but not signal metadata
- `EnvState` stores no signal catalog
- `EnvAction` has no signal-related actions
- `ResponseData` has no env-qualified signal payloads
- `Project` only exposes per-signal `read()`, not grouped reads

So tracing and triggers need this as a real feature, not just a small prerequisite.

**Goals**:

- envd can resolve selectors against all instance signal catalogs
- envd can read values across many instances in one logical operation
- later tracing/breakpoint code can reuse one post-tick grouped-read hook
- instance CLI semantics stay intact; env adds a qualified layer on top

**Recommended public interface**:

```sh
agent-sim env signals my-env
agent-sim env signals my-env upstairs:* *:*.state
agent-sim env get my-env upstairs:hvac.current_temp downstairs:hvac.state
agent-sim env get my-env *
```

**Selector model**:

- instance-level selectors stay as they are today:
  - exact name: `hvac.current_temp`
  - glob: `hvac.*`
  - all: `*`
  - local id: `#12`
- env-level selectors are qualified:
  - exact: `upstairs:hvac.current_temp`
  - instance glob + signal glob: `up*:hvac.*`
  - all instances, suffix-ish glob: `*:*.state`
  - all signals everywhere: `*`
- public env selectors should **not** support `instance:#12`
  - ids are instance-local and not stable across builds/configs
  - they are fine as an internal optimization after envd resolves names once
- bare instance names without a colon should be rejected to keep parsing unambiguous

**Qualification rule**:

- canonical env signal name is `instance:signal`
- reserve `:` in runtime signal names; env bootstrap should fail if any instance signal name contains `:`
- this same qualified name becomes the CSV header/trigger target later

**Output shape**:

- do not reuse plain `SignalData` / `SignalValueData` for env responses as-is
- local signal ids collide across instances, so env responses need env-specific payloads
- recommended fields:
  - `instance`
  - `local_id`
  - `name` (canonical qualified name, e.g. `upstairs:hvac.current_temp`)
  - `signal_type`
  - `units`
  - `value` for read responses

**Internal runtime shape**:

1. During env bootstrap, fetch `InstanceAction::Signals` from every instance after `Info`.
2. Store a per-instance catalog in `EnvState`.
3. Build a resolved env catalog view that can answer:
   - exact qualified name lookup
   - instance glob + signal glob matching
   - per-instance grouping for later reads
4. Move selector resolution into a shared signal-selector module instead of keeping it buried inside `daemon/server.rs`.
5. Keep instance selector semantics identical; env selector resolution should layer on top of them, not redefine them.

**Grouped read path**:

- envd should resolve public selectors once into `(instance_name, signal_id, qualified_name, metadata)` records
- then group by instance and issue one internal read request per instance
- do not route env tracing/breakpoint sampling through repeated string selectors if we can avoid it

**Recommended internal worker API**:

- keep `InstanceAction::Signals` for bootstrap/catalog fetch
- add a dedicated worker read action for env-owned grouped reads, e.g.:
  - `WorkerAction::ReadSignals { ids: Vec<u32> }`
- response should be lean and ordered:
  - either `(id, value)` pairs
  - or just values in input order if we want a tighter internal contract
- avoid returning names/units on every tick; envd already owns the catalog after bootstrap

**Project-layer API**:

- add `Project::read_many(ids: &[u32]) -> Result<Vec<...>, SimError>`
- implementation:
  - use the batch read ABI when present
  - otherwise fall back to repeated single-signal reads during the transition
- this gives one stable runtime call-site for:
  - env `get`
  - tracing
  - breakpoints
  - any future bulk-read feature

**Read ABI direction**:

- treat batch reading as an extension of the main signal-read surface, not as a tracing-only side API
- the runtime should conceptually move to one logical read operation over `N` signal ids; a single-element read is just the `N=1` case
- because the runtime currently requires an exact ABI version match, a hard replacement of `sim_read_val` would force a coordinated ABI bump and break old DLLs immediately
- safest path:
  - add batch read now
  - switch runtime internals to prefer batch reads everywhere
  - keep single-value read only as compatibility fallback / transition support
- once all supported templates/examples export the batch form, we can decide whether to deprecate the single-value export in a later ABI revision

**Recommended ABI extension**:

```c
SimStatus sim_read_vals(const SignalId *ids, SimValue *out, uint32_t count);
```

**Recommended semantics**:

- `count == 0` is valid and returns `SIM_OK`
- `ids[0..count)` and `out[0..count)` are dense arrays with matching length
- output order exactly matches input order
- repeated ids are allowed and produce repeated values
- if any id is invalid, return `SIM_ERR_INVALID_SIGNAL`; host discards partial outputs
- this remains an optional export; runtime falls back when the symbol is absent

**Header / runtime compatibility note**:

- do not add a separate tracing-specific bulk-read API
- extend the existing read surface with `sim_read_vals`
- keep `sim_read_val` in the header during the transition because current runtime/template/example code all assume it exists
- runtime implementation should route all higher-level reads through `Project::read_many()`
- `Project::read()` can become a thin one-element wrapper over `read_many()` internally

**Implementation slices**:

1. Extract shared selector resolution helpers.
2. Add env bootstrap signal-catalog fetch and `EnvState` storage.
3. Add env-qualified response structs and public `env signals` / `env get`.
4. Add `Project::read_many`.
5. Add optional `sim_read_vals` symbol loading and fallback logic.
6. Add dedicated worker grouped-read action and switch envd reads to it.

**Risks**:

- current selector behavior is full glob matching, not suffix shorthand; examples/docs must reflect that precisely
- env responses cannot treat local `id` as globally unique
- adding `sim_read_vals` to `sim_api.h` is only low-risk if kept optional and fallback remains correct
- selector parsing must remain strict enough that instance and env paths do not drift semantically

**Recommendation**:

- do item 2 as both a public env API and an internal grouped-read refactor in the same pass
- keep the public surface small: `env signals` and `env get`
- use ids internally after resolution, but keep names as the public contract

**Validation**:

- instance behavior for `signals` / `get` remains unchanged
- env bootstrap fails cleanly if any signal name contains `:`
- env `signals` / `get` work for exact selectors, globs, and `*`
- grouped reads return stable ordering and correctly qualified names
- batch-read fallback and optional-symbol paths are both covered by tests

---

## 3. Signal tracing — CSV capture to file

Runtime command to record signal values to a CSV file, sampled on a requested simulated-time period. Replaces the existing `watch` command.

**CLI surface**:

```
agent-sim -i test trace start out.csv 1ms          # trace all signals every 1ms
agent-sim -i test trace stop                       # stop writing; file is complete
agent-sim -i test trace clear                      # discard file, reset state
agent-sim -i test trace status                     # show active file, row count, signals
```

**Behaviour**:

- `trace start` requires a sampling period as a positional arg, e.g. `1ms`.
- Sampling is time-based, not implicitly one-row-per-tick.
- `trace start` begins sampling immediately. Rows are flushed as they're written — the file is always valid/readable, `stop` just stops appending.
- V1: always traces **all** signals. If per-signal filtering becomes necessary for performance, add it later as optional positional args.
- Signal set is locked for the duration of a trace. No hot-swap. `stop` then `start` with a new file to change.
- CSV only for now. One header row, then one row per sample event: `tick,time_us,signal_a,signal_b,...`
- Not a recipe step — runtime CLI command only. Recipes can still `step`/`set` while tracing is active; the trace captures whatever the simulation does.
- If the requested sampling period is not an exact multiple of tick duration, sampling rounds up to the next tick that reaches or exceeds the requested simulated-time boundary. Never sample faster than the model advances.
- Do not add `tick` shorthand in v1. Keep one portable time-based model for both instance and env tracing.

**Deprecation**: The existing `watch` command (CLI-side polling loop) is superseded by tracing. Remove `watch` and `WatchSamples` only after trace covers its JSON/test contract.

**Implementation note**:

- this should reuse the grouped-read path from section 2, not invent a second sampling path
- tracing performance depends on batching at two layers:
  - batch FFI reads inside each instance (`sim_read_vals` / `Project::read_many`)
  - batch worker reads per instance inside envd (`WorkerAction::ReadSignals`)

**Env-level tracing**: When tracing through an env, signal names are qualified with the instance name using colon notation: `upstairs:hvac.current_temp`, `downstairs:hvac.state`. The env daemon coordinates sampling after all instances complete their tick. Single output file per env.

```
agent-sim env trace start my-env out.csv 1ms
agent-sim env trace stop my-env
```

**Insertion point**:

- instance: post-`sim_tick()` in `advance_single_project_tick()`, after tick but before CAN/shared TX processing
- env: after all workers complete their tick, before env time advances

**State note**: tracing is daemon/env-owned state, not a CLI polling loop.

---

## 4. Semantic breakpoints — signal triggers

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

**Env-level triggers**: Same colon-qualified signal names as tracing. The env daemon evaluates the trigger after all instances complete their tick, sampling the target signal from the relevant instance.

```
agent-sim env trigger set my-env upstairs:hvac.current_temp gt 25.0
agent-sim env trigger show my-env
```

**Insertion point**: Same post-tick sampling hook as tracing. Tracing (if active) should capture the firing tick before the simulation pauses.

---

## 5. Virtual CAN transport

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

## 6. iceoryx2 — optional env data/event plane

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

## Priority order

1. **ipc.rs → interprocess** — lowest risk; keep a thin wrapper for naming/cleanup semantics
2. **Env signal catalog + grouped read API** — selector refactor, env-qualified catalog, grouped reads, optional batch-read ABI
3. **Signal tracing** — CSV capture built on the grouped-read path; `watch` removal afterwards
4. **Semantic breakpoints** — same post-tick sampling hook as tracing
5. **Virtual CAN transport** — add backend-selectable virtual bus behind `CanSocket`
6. **iceoryx2 spike, if still needed** — only after the simpler virtual/backend work has been measured
