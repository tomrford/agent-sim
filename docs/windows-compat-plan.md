# Windows Runtime Compatibility Plan

Plan for making the `agent-sim` runtime work on Windows without changing the DLL ABI or the JSON request protocol.

## Goal

Enable Windows support for:

- standalone instance flows: `load`, `info`, `signals`, `get`, `set`, `time`, `close`
- env flows that do not depend on Linux-only assumptions
- the existing Zig template and HVAC example DLLs

Keep Linux behavior intact. Keep the `sim_api.h` ABI unchanged.

## Non-goals

- redesign the CLI or JSON protocol
- redesign the DLL ABI
- full macOS parity
- changing the current Windows CAN backend unless it becomes necessary for runtime integration

## Current blockers

### 1. Core local IPC is Unix-only

The runtime control plane is built directly on Tokio Unix domain sockets.

- `runtime/src/connection.rs` uses `tokio::net::UnixStream`
- `runtime/src/daemon/server.rs` uses `UnixListener` and `UnixStream`
- `runtime/src/envd/server.rs` uses `UnixListener` and `UnixStream`
- `runtime/src/envd/server/instance_worker.rs` uses `UnixStream` and `tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf}`

This is the first Windows blocker. The runtime cannot be made portable by fixing `connection.rs` alone.

### 2. Daemon discovery is tied to `*.sock` files

Instance and env lifecycle code uses `.sock` paths both as:

- the actual transport endpoint
- the source of truth for "what is running"

Files involved:

- `runtime/src/daemon/lifecycle.rs`
- `runtime/src/envd/lifecycle.rs`

This works for Unix domain sockets but does not map cleanly to Windows named pipes.

### 3. Long-lived worker connections depend on Unix split halves

The env instance worker keeps a persistent connection to an instance daemon using Unix-specific owned halves. That makes the current worker implementation transport-specific instead of protocol-specific.

File involved:

- `runtime/src/envd/server/instance_worker.rs`

### 4. Forced process cleanup has a Unix-only fallback

`kill_pid()` falls back to `libc::kill(SIGKILL)` on Unix and returns "not supported on this platform" elsewhere.

Files involved:

- `runtime/src/daemon/lifecycle.rs`
- `runtime/src/envd/server.rs`

Windows needs an explicit terminate-by-pid implementation instead of a Unix-only fallback.

### 5. Tests and docs still assume Unix

Examples:

- tests shell out via `bash` or `sh`
- tests use `/tmp/...`
- docs still describe Unix sockets and `.so` examples in several places

Files involved:

- `runtime/tests/common/mod.rs`
- `runtime/tests/cli_project_instance.rs`
- `README.md`
- `docs/agent-guide.md`
- `docs/template-guide.md`

These are not the first runtime blocker, but they will block credible Windows validation and confuse users.

## What is already in good shape

These areas should not need architectural redesign for Windows:

- DLL ABI in `include/sim_api.h`
- dynamic library loading in `runtime/src/sim/project.rs`
- native library suffix resolution in `runtime/src/load/resolve.rs`
- shared-memory file mapping in `runtime/src/shared/mod.rs`
- Windows CAN backend wiring in `runtime/src/can/backend/mod.rs`

This is why the Windows plan should focus on the runtime control plane first.

## Recommended architecture

### 1. Keep the protocol, abstract the transport

Do not change the JSON-lines request/response protocol. It is already simple and portable.

Instead, introduce a local IPC layer under a new module such as:

- `runtime/src/ipc/mod.rs`
- `runtime/src/ipc/address.rs`
- `runtime/src/ipc/client.rs`
- `runtime/src/ipc/listener.rs`
- `runtime/src/ipc/registry.rs`
- `runtime/src/process/mod.rs`

The abstraction boundary should sit above the raw socket/pipe type and below request handling.

### Why this boundary

The current server/client code mostly needs three things:

- connect to a local endpoint
- accept local connections
- read and write newline-delimited JSON frames

If the new abstraction exposes "JSON transport" behavior instead of raw Unix types, most higher-level request handling can stay unchanged.

### 2. Use named pipes on Windows

Preferred transport split:

- Unix: Unix domain sockets
- Windows: named pipes

Why named pipes:

- local-only, like the current Unix socket design
- no dynamic TCP port allocation
- no firewall prompts
- closest semantic match to the existing architecture

Loopback TCP is a valid emergency fallback, but it should not be the target design unless named-pipe complexity proves unacceptable.

### 3. Replace raw transport types with protocol-oriented wrappers

Avoid exposing `UnixStream`, `UnixListener`, or `tokio::net::unix::*` outside the IPC module.

Recommended shape:

- `EndpointKind`: `Instance` or `Env`
- `EndpointAddress`: transport-specific address
- `EndpointMeta`: on-disk registry record
- `JsonClient`: one-shot request/response client
- `PersistentClient`: long-lived request/response client for env worker use
- `LocalListener`: bind + accept wrapper

Possible address model:

- Unix: filesystem socket path
- Windows: pipe name such as `\\.\pipe\agent-sim\instance\<name>`

The important point is that the rest of the runtime should not know which one it is using.

### 4. Prefer generic framing helpers over transport-specific split halves

Current worker code is tightly coupled to Unix-specific owned halves. Replace that with generic framing helpers over any `AsyncRead + AsyncWrite` transport.

Two reasonable options:

1. keep a stream intact and implement:
   - `read_request_line()`
   - `write_response_line()`
   - `send_request()`
2. split generically inside the IPC module and hand back a transport-agnostic framed connection

Prefer option 1 if possible. It keeps the transport wrapper simpler and avoids surfacing transport-specific half types.

### 5. Replace `.sock` discovery with registry metadata

Do not use endpoint filenames as the runtime registry.

Instead, store metadata files under `AGENT_SIM_HOME`, for example:

```text
.agent-sim/
  instances/
    <name>.json
  envs/
    <name>.json
  bootstrap/
```

Each metadata file should contain enough information to reconnect or declare the entry stale:

- logical name
- endpoint kind
- transport kind
- endpoint address
- daemon pid
- optional env tag
- format version

Benefits:

- Windows no longer depends on `.sock` files
- Unix and Windows share the same registry model
- listing and stale-entry cleanup become transport-agnostic

### 6. Add a platform process helper

Introduce a small process abstraction for forced termination:

- Unix: existing signal-based implementation
- Windows: terminate process by PID with a Windows-specific implementation

This should live in one place, rather than leaking `#[cfg(unix)]` logic through lifecycle code.

That keeps daemon/env shutdown logic platform-neutral.

## Suggested module responsibilities

### `ipc/address`

- endpoint name validation and sanitization
- deterministic endpoint address construction
- transport kind selection by platform

### `ipc/registry`

- read/write endpoint metadata
- list instances/envs
- stale entry detection
- metadata cleanup on shutdown

### `ipc/client`

- one-shot connect/send/read helpers for CLI requests
- persistent client support for env worker connections

### `ipc/listener`

- bind listener
- accept incoming connections
- hide Unix vs Windows listener differences

### `process`

- terminate by pid
- optional helpers for pid existence checks if stale cleanup needs them

## Migration plan

### Phase 0: document actual support

Before or alongside the port:

- document Linux as the current primary platform
- document Windows runtime support as planned, not complete
- stop describing the current architecture as universally cross-platform while Unix sockets are still hard-coded

This prevents the docs from over-promising during the migration.

### Phase 1: introduce the IPC abstraction without changing behavior

Goal:

- Linux still uses Unix sockets
- runtime code stops importing Unix transport types directly outside `ipc`

Refactor targets:

- `runtime/src/connection.rs`
- `runtime/src/daemon/server.rs`
- `runtime/src/envd/server.rs`
- `runtime/src/envd/server/instance_worker.rs`

Success criteria:

- Linux tests still pass
- all Unix transport imports are isolated to `ipc` platform modules

### Phase 2: add registry metadata and stop scanning `*.sock`

Goal:

- listing and health checks use metadata files
- transport endpoint files are no longer treated as the registry

Refactor targets:

- `runtime/src/daemon/lifecycle.rs`
- `runtime/src/envd/lifecycle.rs`

Success criteria:

- Linux behavior stays unchanged for users
- the registry is now transport-agnostic

### Phase 3: add Windows named-pipe transport

Goal:

- compile and run instance daemon flows on Windows

Initial validation scope:

- `load`
- `info`
- `signals`
- `get`
- `set`
- `time`
- `close`

Success criteria:

- standalone instance workflows run on Windows with the template DLL

### Phase 4: port env flows

Goal:

- env daemon and env worker support Windows

Validation scope:

- `env start`
- `env status`
- `env time`
- `env reset`
- `close`

Success criteria:

- the single-node HVAC env workflow runs on Windows without Unix transport code

### Phase 5: process cleanup and stale metadata handling

Goal:

- forced cleanup works on Windows
- dead daemon metadata can be detected and removed safely

This phase closes the lifecycle gaps that remain after transport is working.

### Phase 6: tests, CI, and docs cleanup

Goal:

- add real Windows validation
- remove Unix-only assumptions from tests and docs

Work items:

- replace `bash`/`sh` test helpers with direct process spawning
- replace hard-coded `/tmp` with tempdir-based paths
- use extensionless or native-suffix DLL examples in docs
- add a Windows CI job for runtime tests

## PR slicing

Keep the work reviewable. Suggested PR order:

1. docs/support-matrix cleanup
2. IPC abstraction on Unix only
3. metadata registry refactor
4. Windows transport for standalone instance flows
5. Windows env flows
6. process cleanup abstraction
7. Windows CI and remaining docs/tests cleanup

## Risks and decision notes

### Risk: named-pipe server model differs from Unix listeners

Tokio named pipes are not a drop-in `UnixListener` replacement. The IPC module must absorb that difference so server code still has a simple accept loop.

### Risk: stale metadata cleanup can kill the wrong process

If stale cleanup relies only on PID, it must be conservative. When in doubt, prefer "mark stale and require retry" over aggressive termination.

### Risk: mixing registry refactor with transport refactor can grow the diff

The safest path is:

1. isolate transport imports
2. introduce metadata registry
3. add Windows transport

### Decision: do not change the DLL side for Windows runtime support

The Zig template and example already cross-build cleanly. Windows runtime work should not expand into DLL ABI changes unless runtime integration proves a missing host assumption.

## Definition of done

Windows compatibility for the runtime is "done enough" when all of the following are true:

- `cargo check` succeeds for a Windows target
- standalone instance flows work on Windows
- env flows work on Windows for the HVAC example
- no runtime module outside `ipc` imports Unix transport types
- instance/env listing no longer depends on `.sock` file scans
- forced cleanup has a Windows implementation
- docs describe the support matrix accurately
- Windows CI covers the supported flows
