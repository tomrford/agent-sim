# agent-sim Runtime — Product Requirements Document (V1)

## 1. Problem Statement

Firmware SIL (Software-in-the-Loop) testing requires running compiled firmware logic against controlled inputs and observing outputs. Today this is done with hardware-in-the-loop rigs, MATLAB/Simulink, or ad-hoc test harnesses. None of these are ergonomic for AI agents or CI pipelines.

**agent-sim** provides a stateful CLI that loads firmware compiled as a shared library (via a stable C ABI), lets users (human or AI agent) manipulate signals, control time, and script deterministic test sequences — all from the terminal.

## 2. Design Philosophy

### 2.1 Agent Experience (AX) First

The CLI is designed for programmatic consumption by AI coding agents. Every command produces minimal, predictable output. `--json` mode emits machine-readable JSON. Human mode uses clean tables (comfy-table). No chatty banners, no prompts, no ambiguous output.

Reference: [agent-browser](https://github.com/vercel-labs/agent-browser) — same stateful-CLI-over-IPC pattern.

### 2.2 Determinism by Default

Simulation state advances only when explicitly stepped or when a real-time clock is running. No implicit side effects. Recipe scripts produce identical results on every run.

### 2.3 Single Binary

The CLI and daemon are the same binary (`agent-sim` vs `agent-sim --daemon`). No external runtime dependencies (no Node.js, no Python). Cross-platform (Linux, macOS, Windows).

## 3. Architecture

```
┌──────────────┐     Unix socket / named pipe     ┌──────────────────────┐
│  agent-sim   │  ◄──────── JSON lines ────────►  │  agent-sim --daemon  │
│  (CLI client)│                                   │                      │
└──────────────┘                                   │  ┌────────────────┐  │
                                                   │  │  Project       │  │
                                                   │  │  (dlopen DLL)  │  │
                                                   │  │  sim_api.h ABI │  │
                                                   │  └───────┬────────┘  │
                                                   │          │           │
                                                   │  ┌───────▼────────┐  │
                                                   │  │  Instance 0    │  │
                                                   │  │  (SimCtx*)     │  │
                                                   │  ├────────────────┤  │
                                                   │  │  Instance 1    │  │
                                                   │  │  (SimCtx*)     │  │
                                                   │  └────────────────┘  │
                                                   │                      │
                                                   │  ┌────────────────┐  │
                                                   │  │  Time Engine   │  │
                                                   │  │  (tick loop)   │  │
                                                   │  └────────────────┘  │
                                                   └──────────────────────┘
```

### 3.1 Client–Daemon Model

The daemon is a long-lived process that holds the loaded project and all instances. The CLI is a thin client that sends commands over IPC and prints responses.

**Why:** Library loading and `SimCtx` state are expensive to set up. A daemon keeps them alive between CLI invocations, enabling the stateful workflow agents need.

**IPC transport:**

- Unix: `~/.agent-sim/{session}.sock`
- Windows: named pipe `\\.\pipe\agent-sim-{session}`
- Protocol: newline-delimited JSON (one request line → one response line)
- Read timeout: 30s. Write timeout: 5s. Retry transient errors up to 5× with 200ms backoff.

### 3.2 Daemon Lifecycle

```
agent-sim load ./libsim.so
  ├─ Is daemon running for session? (check socket/PID file)
  │   ├─ No → spawn daemon (same binary, --daemon flag, detached)
  │   │       poll socket until ready (≤5s)
  │   └─ Yes → send "load" command
  └─ Daemon: dlopen, bind symbols, verify ABI version, cache signal catalog

agent-sim close
  └─ Daemon: free all instances, dlclose, remove socket + PID, exit
```

### 3.3 Abstraction Layers

```
Project (shared library)
 └─ 1 loaded at a time per session (V1)
 └─ owns the dlopen handle + signal catalog metadata
 └─ produces Instance objects via sim_new()

Instance (= 1 SimCtx*)
 └─ owns all mutable simulation state for one simulated device
 └─ can be created, reset, freed independently
 └─ 1 Project can produce N Instances

Time Engine
 └─ drives sim_tick() across all instances in the session
 └─ states: paused, running
 └─ user-facing time unit: duration (seconds, milliseconds, microseconds)
 └─ engine converts duration → tick count per instance using tick_duration_us
 └─ speed: wallclock multiplier (1.0 = realtime)
```

## 4. Concepts & Terminology

| Term         | Definition                                                                                                                              |
| ------------ | --------------------------------------------------------------------------------------------------------------------------------------- |
| **Project**  | The loaded shared library (`.so`/`.dylib`/`.dll`) implementing `sim_api.h`. One per session (V1). Owns signal catalog and tick quantum. |
| **Instance** | A single `SimCtx*` handle. Represents one simulated device. Multiple instances can exist per project.                                   |
| **Signal**   | A named, typed value exposed by the project (discovered at runtime). Addressable by name, `#id`, or glob pattern.                       |
| **Tick**     | One simulation quantum. Duration reported by `sim_get_tick_duration_us()`. Internal unit — users interact via time durations.           |
| **Session**  | An isolated daemon process with its own socket, project, instances, and time engine.                                                    |
| **Recipe**   | A named sequence of commands defined in `agent-sim.toml`.                                                                               |

## 5. Command Reference

### 5.1 Project & Instance Lifecycle

```
agent-sim load <libpath>              # Load project, discover signals, create instance 0
agent-sim unload                      # Free all instances, unload project
agent-sim info                        # Show loaded project path, tick quantum, signal count, instance count

agent-sim instance new                # Create new instance (returns instance index)
agent-sim instance list               # List instances with index
agent-sim instance select <index>     # Set active instance for subsequent commands
agent-sim instance reset [index]      # sim_reset (active instance if index omitted)
agent-sim instance free <index>       # sim_free + remove instance
```

### 5.2 Time Control

Time has two states:

```
          start              pause
  ┌────────┐ ──────────► ┌─────────┐
  │ PAUSED │              │ RUNNING │
  └────────┘ ◄──────────  └─────────┘
                 pause
```

Initial state after `load` is PAUSED.

```
agent-sim time start              # Begin real-time tick loop (PAUSED → RUNNING)
agent-sim time pause              # Pause tick loop (RUNNING → PAUSED)
agent-sim time step <duration>    # Advance by duration (e.g. "1s", "100ms", "500us"). Only valid when PAUSED.
agent-sim time speed [multiplier] # Get/set speed. 1.0 = realtime. 0.5 = half speed. 10.0 = 10x.
agent-sim time status             # Show state, elapsed ticks, elapsed sim-time, speed
```

**Duration format:** `<number><unit>` where unit is `s`, `ms`, or `us`. Examples: `1s`, `100ms`, `500us`, `0.5s`.

**Time → tick conversion:** The engine converts the requested duration to a tick count using `tick_duration_us` from the loaded project: `ticks = floor(duration_us / tick_duration_us)`. If the duration is not an exact multiple, the engine advances the floored tick count and reports actual time advanced. This allows V2 multi-project sessions (different tick rates) to advance each instance by the correct number of ticks for the same wall-of-time request.

**Semantics:**

- `step` is always deterministic — no wallclock involvement. Ticks all instances sequentially per step. Command blocks until all ticks complete and returns actual time advanced.
- `start` drives `sim_tick()` on all instances at `speed × tick_duration_us` wallclock intervals.
- `speed` only affects RUNNING state. `step` always executes as fast as possible.
- When RUNNING, if the host can't keep up (speed too high), it ticks as fast as possible and logs a warning.
- All signal reads/writes are valid in any time state. Signals are read/written to the current `SimCtx` state; `sim_tick()` advances that state.
- `step` while RUNNING is an error — pause first.

**V2 considerations (out of scope, design for):**

- Per-instance time coupling (lock-step vs free-running).
- External clock sources (vCAN, MATLAB/Simulink co-simulation).
- Time synchronization across sessions (multi-device IPC).
- Instances from different projects with different tick rates coexisting in one session.

### 5.3 Signal I/O

```
agent-sim signals                         # List all signals (table: name, type, units)
agent-sim get <signal> [<signal> ...]     # Read one or more signals from active instance
agent-sim get *                           # Read all signals (snapshot)
agent-sim set <signal> <value>            # Write single signal to active instance
agent-sim set <sig1>=<val1> <sig2>=<val2> # Batch write (multiple signals, one call)
agent-sim watch <signal> [interval_ms]    # Stream signal value (human: live, json: NDJSON)
```

**Signal addressing:**

- By name: `agent-sim get motor_rpm`
- By ID: `agent-sim get #3`
- Glob: `agent-sim get "motor_*"` (matches all signals starting with `motor_`)
- Wildcard: `agent-sim get *` (all signals — replaces the concept of "snapshot")

### 5.4 Recipe Execution

```
agent-sim run <recipe-name>               # Execute named recipe from agent-sim.toml
agent-sim run <recipe-name> --dry-run     # Print commands without executing
```

### 5.5 Session Management

```
agent-sim session                         # Show current session name + status
agent-sim session list                    # List all active sessions
agent-sim close                           # Shut down daemon for current session
```

Default session is `default`. Override with `--session <name>` or `AGENT_SIM_SESSION`.

### 5.6 Global Flags

| Flag                 | Env var              | Description                               |
| -------------------- | -------------------- | ----------------------------------------- |
| `--json`             | `AGENT_SIM_JSON`     | JSON output mode                          |
| `--session <name>`   | `AGENT_SIM_SESSION`  | Named session                             |
| `--instance <index>` | `AGENT_SIM_INSTANCE` | Override active instance for this command |
| `--config <path>`    | `AGENT_SIM_CONFIG`   | Config file path                          |

## 6. Configuration

### 6.1 Layering (lowest → highest priority)

1. `~/.config/agent-sim/config.toml` — user defaults
2. `./agent-sim.toml` — project config + recipes
3. `AGENT_SIM_*` env vars
4. CLI flags

### 6.2 Config File Format

```toml
[defaults]
json = false
speed = 1.0

[defaults.load]
lib = "./target/libsim_template.so"
```

### 6.3 Recipes

Recipes are ordered lists of commands. Available recipe commands:

| Command           | Description                                           |
| ----------------- | ----------------------------------------------------- |
| `set`             | Write one or more signals                             |
| `step`            | Advance by duration (e.g. `"1s"`, `"100ms"`)          |
| `print`           | Print specific signal(s), or `"*"` for all            |
| `speed`           | Set simulation speed                                  |
| `reset`           | Reset active instance                                 |
| `instance_new`    | Create a new instance                                 |
| `instance_select` | Switch active instance                                |
| `sleep`           | Wall-clock sleep (ms)                                 |
| `for`             | Loop a signal over a range, executing `each` per step |

```toml
[recipe.cold-start]
description = "Initialize for cold-start test"
steps = [
  { set = { ignition = true, ambient_temp = -20.0, fuel_level = 0.8 } },
  { step = "1s" },
  { print = "*" },
]

[recipe.rpm-sweep]
description = "Sweep target RPM and print actual RPM at each step"
steps = [
  { set = { ignition = true } },
  { step = "500ms" },
  { for = { signal = "target_rpm", from = 800.0, to = 6000.0, by = 200.0, each = [
      { step = "1s" },
      { print = ["target_rpm", "actual_rpm", "engine_load"] },
  ] } },
  { print = "*" },
]
```

## 7. Output Modes

### 7.1 Human (default)

Clean tables, minimal chrome:

```
$ agent-sim signals
 Name            Type   Units
 motor_rpm       f32    rpm
 throttle_pos    f32    %
 ignition        bool   -
 coolant_temp    f32    °C

$ agent-sim get motor_rpm
1523.5

$ agent-sim get *
 Name            Type   Value       Units
 motor_rpm       f32    1523.5      rpm
 throttle_pos    f32    0.82        %
 ignition        bool   true        -
 coolant_temp    f32    87.3        °C

$ agent-sim time status
 State: PAUSED  Ticks: 4200  Sim-time: 42.000s  Speed: 1.0x
```

### 7.2 JSON (`--json`)

Every command returns a single JSON object:

```json
{
  "success": true,
  "data": {
    "signals": [
      {
        "id": 0,
        "name": "motor_rpm",
        "type": "f32",
        "value": 1523.5,
        "units": "rpm"
      },
      {
        "id": 1,
        "name": "throttle_pos",
        "type": "f32",
        "value": 0.82,
        "units": "%"
      }
    ]
  }
}
```

Errors:

```json
{ "success": false, "error": "signal not found: 'nonexistent'" }
```

### 7.3 NDJSON (streaming)

`watch` emits one JSON line per sample:

```json
{"tick":4200,"time_us":42000000,"name":"motor_rpm","value":1523.5}
{"tick":4201,"time_us":42010000,"name":"motor_rpm","value":1524.1}
```

## 8. IPC Protocol

### 8.1 Request

```json
{
  "id": "a1b2c3",
  "action": "set",
  "instance": 0,
  "signals": { "motor_rpm": 1500.0, "throttle_pos": 0.8 }
}
```

### 8.2 Response

```json
{
  "id": "a1b2c3",
  "success": true
}
```

### 8.3 Error Response

```json
{
  "id": "a1b2c3",
  "success": false,
  "error": "type mismatch: signal 'ignition' expects bool, got f32"
}
```

### 8.4 Command Processing

Commands are queued and processed sequentially. `SimCtx*` is not thread-safe, and sequential processing matches the "write inputs → tick → read outputs" contract. The real-time tick loop runs on a separate tokio task; signal reads/writes are interleaved between ticks (never concurrent with a tick on the same instance).

## 9. Error Handling

### 9.1 Simulation Errors

`SimStatus` codes are mapped to typed Rust errors:

| SimStatus                  | Rust variant               | CLI message                                                         |
| -------------------------- | -------------------------- | ------------------------------------------------------------------- |
| `SIM_ERR_INVALID_CTX`      | `SimError::InvalidCtx`     | `"invalid instance context (freed or corrupted)"`                   |
| `SIM_ERR_INVALID_ARG`      | `SimError::InvalidArg`     | `"invalid argument: {detail}"`                                      |
| `SIM_ERR_INVALID_SIGNAL`   | `SimError::InvalidSignal`  | `"signal not found: '{name}'"`                                      |
| `SIM_ERR_TYPE_MISMATCH`    | `SimError::TypeMismatch`   | `"type mismatch: signal '{name}' expects {expected}, got {actual}"` |
| `SIM_ERR_BUFFER_TOO_SMALL` | `SimError::BufferTooSmall` | (internal, retried with larger buffer)                              |
| `SIM_ERR_INTERNAL`         | `SimError::Internal`       | `"internal simulation error"`                                       |

### 9.2 Error Hierarchy (thiserror)

```
AgentSimError (top-level)
├── ProjectError     (dlopen, symbol binding, ABI version mismatch)
├── SimError         (SimStatus → Rust, per above)
├── InstanceError    (no active instance, index out of range)
├── TimeError        (invalid state transition, e.g. step while RUNNING)
├── ConfigError      (TOML parse, missing recipe)
├── ProtocolError    (IPC serialization, malformed request)
├── ConnectionError  (socket not found, timeout, daemon not running)
└── IoError          (transparent from std::io::Error)
```

Each subsystem gets its own `error.rs` with `#[error(transparent)]` + `#[from]` roll-up into the parent, per existing project conventions.

## 10. Crate Organization

```
runtime/
├── Cargo.toml
└── src/
    ├── main.rs                 # Entry: --daemon → daemon::run(), else cli::run()
    ├── lib.rs                  # pub mod declarations
    │
    ├── cli/
    │   ├── mod.rs              # run() → ExitCode
    │   ├── args.rs             # clap derive, top-level Args + flattened sub-args
    │   ├── commands.rs         # Args → protocol::Request dispatch
    │   ├── output.rs           # Human (comfy-table) + JSON formatting
    │   └── error.rs            # CliError
    │
    ├── daemon/
    │   ├── mod.rs              # run() — tokio runtime, socket listener
    │   ├── server.rs           # Accept loop, command queue, request dispatch
    │   ├── lifecycle.rs        # Spawn (client-side), ready check, shutdown
    │   └── error.rs            # DaemonError
    │
    ├── sim/
    │   ├── mod.rs
    │   ├── project.rs          # dlopen, symbol binding, ABI version check, signal catalog
    │   ├── instance.rs         # Instance wrapper (SimCtx* + index)
    │   ├── instance_manager.rs # InstanceManager: create/free/select/list instances
    │   ├── types.rs            # Rust repr of SimValue, SimSignalDesc, SimType, SimStatus
    │   ├── time.rs             # TimeEngine: state machine, tick loop, speed control, duration→tick conversion
    │   └── error.rs            # SimError, ProjectError, InstanceError, TimeError
    │
    ├── config/
    │   ├── mod.rs              # Config loading, layering, resolution
    │   ├── recipe.rs           # Recipe parsing + execution
    │   └── error.rs            # ConfigError
    │
    ├── protocol.rs             # Request/Response serde types, action enum
    └── connection.rs           # Client-side IPC: connect, send, retry logic
```

## 11. Dependencies

| Crate                         | Purpose                                                |
| ----------------------------- | ------------------------------------------------------ |
| `clap` (derive)               | CLI arg parsing                                        |
| `serde` + `serde_json`        | Protocol + output serialization                        |
| `toml`                        | Config/recipe files                                    |
| `comfy-table`                 | Human-readable table output                            |
| `thiserror`                   | Error type derivation                                  |
| `libloading`                  | Cross-platform dlopen/dlsym                            |
| `tokio` (rt, net, sync, time) | Async runtime for daemon (socket listener + tick loop) |
| `uuid` or `nanoid`            | Request IDs                                            |

**Not used:** `nix` (Unix-only), `anyhow` (prefer typed errors), `rayon` (no parallelism needed).

## 12. Cross-Platform Strategy

| Concern       | Unix                                             | Windows                                     |
| ------------- | ------------------------------------------------ | ------------------------------------------- |
| IPC           | Unix domain socket                               | Named pipe (`\\.\pipe\agent-sim-{session}`) |
| DLL extension | `.so` / `.dylib`                                 | `.dll`                                      |
| Daemon spawn  | `fork`-like via `std::process::Command` + detach | `CREATE_NEW_PROCESS_GROUP`                  |
| Socket dir    | `~/.agent-sim/` or `XDG_RUNTIME_DIR`             | `%LOCALAPPDATA%\agent-sim\`                 |
| PID file      | `{session}.pid`                                  | Same                                        |

`libloading` and `tokio` handle platform differences. No platform-specific crates needed.

## 13. Implementation Phases

| Phase                       | Scope                                                                            | Commands                                                                   |
| --------------------------- | -------------------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| **P0: Foundation**          | Crate structure, clap args, error types, protocol types, config loading          | — (scaffold only)                                                          |
| **P1: Project + Instances** | `dlopen`, symbol binding, ABI check, signal catalog, instance create/free/reset  | `load`, `unload`, `info`, `signals`, `instance new/list/select/reset/free` |
| **P2: Signal I/O**          | Read/write through FFI, batch ops, glob/wildcard, value parsing/formatting       | `get`, `set` (single + batch + wildcard)                                   |
| **P3: Daemon IPC**          | Socket listener, client connect, spawn, command queue, request/response loop     | `close`, `session`, `session list`                                         |
| **P4: Time Engine**         | State machine, duration→tick conversion, deterministic step, real-time tick loop | `time start/pause/step/speed/status`                                       |
| **P5: Output Polish**       | comfy-table formatting, `--json`, NDJSON streaming                               | `watch`, all commands get pretty output                                    |
| **P6: Recipes**             | TOML recipe parsing, execution engine, `for` loops, `print`                      | `run <recipe>`                                                             |

## 14. Out of Scope (V2+)

- Multi-project loading per session (multiple DLLs with different tick rates)
- Inter-session IPC (instance-to-instance communication across sessions)
- vCAN interface for human experimentation
- External simulator coupling (MATLAB/Simulink, PLECS)
- Per-instance independent time (free-running vs lock-step)
- Recipe assertions / expected-value checking
- Context serialization (save/load instance state to file)
- Signal groups / structured signals (nested structs)

## 15. Open Design Notes

- **Time coupling (V2 prep):** The `TimeEngine` is designed as a separate component from `InstanceManager`. The duration-based `step` command (not tick-count-based) means V2 can introduce instances with different tick rates and the user-facing API stays the same — the engine just converts the same duration to different tick counts per instance. V1's single-project case is the degenerate form where all instances share one tick quantum.
- **Context save/load:** The ABI doesn't currently expose context serialization. When needed, this requires either a new ABI call (`sim_serialize_ctx`) or the runtime snapshotting all signal values and replaying writes + ticks. Design the `Instance` wrapper so either approach can be added without breaking the command surface.
- **Signal namespacing:** V1 has one project, so signal names are globally unique. V2 multi-project will need `project_name.signal_name` or similar. The signal addressing layer should be prepared for a namespace prefix.
