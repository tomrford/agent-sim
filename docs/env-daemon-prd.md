# PRD: Persistent Env Daemon and Env-Owned Transport Orchestration

Status: draft

Terminology note:

- the product now uses `instance` as the preferred user-facing term
- this PRD uses `instance daemon` for the per-device worker process

Supersedes parts of `docs/prd.md`:

- `Orchestrator model`
- `Multi-instance tick synchronisation`
- the assumption that CAN attachment and timing live only in per-instance daemons

## Summary

Introduce a persistent env daemon as the control plane for multi-device simulation.

In env mode:

- the env daemon owns logical time, topology, and env-level transport state
- instance daemons continue to own per-device simulation state
- CAN is managed as an env-owned transport, not as a separate binary and not as a signal abstraction
- `agent-sim` remains the only user-facing binary; `agent-sim can ...` is part of the same CLI
- CAN is exposed only through message/bus-oriented scaffolding, not through `get`/`set` signal selectors

This keeps the architecture deterministic enough for reproducible SIL tests while still allowing external tools to monitor or inject VCAN traffic when the user chooses to do so.

## Problem

The current branch has three constraints that become more painful as multi-device work grows:

1. `env start` is CLI-side fan-out, not a persistent controller.
   It starts instances, attaches CAN/shared resources, then exits.

2. Each instance daemon owns its own `TimeEngine`.
   That is fine for standalone single-device use, but it does not provide env-level pause, step, or speed authority.

3. CAN stimulus needs persistent runtime state.
   One-off frame injection is straightforward, but cyclic traffic, scheduled jobs, last-send bookkeeping, and deterministic replay all need a persistent owner across CLI calls.

The current CAN DBC-as-signal overlay is not the right abstraction for the next phase. Message scheduling is a bus concern, not a scalar signal concern. The new direction is to drop CAN read/write through the signal namespace and support CAN only through dedicated CAN message/bus scaffolding.

## Goals

- Keep a single user-facing binary: `agent-sim`
- Add a persistent env daemon for multi-device orchestration
- Make env time authoritative in env mode
- Keep per-device sim state in instance daemons
- Move CAN bus ownership into env-level orchestration
- Drop CAN-as-signals from the product direction
- Support persistent CAN state across CLI calls:
  - cyclic jobs
  - one-shot sends
  - frame history / latest frame cache
  - optional DBC codec/cache state
- Preserve external VCAN interoperability for tools like `candump`, SavvyCAN, and python-can
- Leave room for future transports without committing to one daemon per thing
- Keep CAN code strongly isolated so it can later be extracted into a shared crate or standalone CLI if that becomes useful

## Non-Goals

- Creating or deleting VCAN interfaces
- Preventing users from attaching outside tools to VCAN
- Adding a separate `agent-can` binary
- One daemon per transport instance as the default design
- Treating flashing as a runtime orchestration concern
- Solving every future transport in this PRD; this document sets the control-plane direction

## Current State

Today:

- the CLI talks directly to instance daemons over Unix sockets
- `env start` is a stateless orchestration command in the CLI
- each instance daemon owns:
  - loaded DLL/project state
  - time state
  - attached CAN sockets
  - DBC overlays
  - shared-state attachments
- env membership is just an `env_tag` attached to each instance

This model is good for:

- standalone usage
- low-complexity instance grouping
- simple VCAN attachment

It is weak for:

- deterministic multi-instance stepping
- pause/resume semantics across a full env
- persistent bus scheduling
- future transport growth

## Proposed Architecture

### High-Level Model

There are three runtime roles:

1. CLI client
2. env daemon
3. instance daemons

The CLI remains stateless.

The env daemon becomes the owner of:

- env membership
- logical time
- env topology
- env-scoped transport managers
- command routing for env-scoped operations

Instance daemons remain the owner of:

- device/project state
- per-device FFI interactions
- signal I/O against the loaded DLL
- device-local state that must persist across CLI calls

### Modes

#### Standalone instance mode

Existing single-device workflows remain valid:

- an instance daemon can still run independently
- the instance daemon owns its own time in this mode
- direct `load`, `get`, `set`, `time`, `close` continue to work as today

#### Env mode

When an instance belongs to a running env:

- env-level time becomes authoritative
- instance daemons act as stepped workers
- env-scoped transports are managed by the env daemon

This avoids trying to keep multiple independently-running clocks in sync by convention.

## Time Ownership

### Decision

In env mode, the env daemon owns logical time.

That means:

- current env time state lives in the env daemon
- env-wide `start`, `pause`, `speed`, and `step` are coordinated there
- instance daemons do not independently advance time in env mode unless explicitly instructed by the env daemon

### Why

Centralized time authority is required for:

- deterministic step-by-step SIL tests
- coherent pause/resume semantics
- bus scheduling tied to simulation time
- future cross-transport coordination

Centralized time authority does not require centralized simulation state. Instance daemons still keep their own device state.

### Expected Semantics

In env mode:

- `pause` means the env daemon stops advancing logical time
- `step N` means the env daemon advances the env by `N` ticks using a controlled barrier
- `speed 2x` means the env daemon drives env progression at that rate in realtime mode
- the env-level time surface should mirror the current instance-level surface:
  - `start`
  - `pause`
  - `step`
  - `speed`
  - `status`

Per-instance `time ...` commands in env mode should be rejected with a clear error. The user should be told to use env-level time commands instead.

## CAN Ownership

### Decision

CAN should be env-owned first, not split into a separate binary and not split into separate per-bus daemons by default.

In env mode, the env daemon:

- attaches to all VCAN interfaces declared by the env
- owns CAN transport state
- keeps persistent CAN scheduling state across CLI calls
- steps CAN behavior according to env time

### Why

This provides the cleanest path for:

- deterministic cyclic messages
- bus-level scheduling
- env-scoped topology awareness
- fewer daemons to manage
- one CLI surface

It also avoids prematurely committing to a daemon-per-bus process model before there is evidence that process isolation is worth the operational complexity.

### CAN State in Env

The env daemon should be able to persist:

- bus registry
- VCAN attachment metadata
- latest observed frames per bus/arbitration ID
- cyclic transmit jobs
- `last_sent_tick`
- `next_due_tick`
- optional DBC metadata/cache
- tracing/recording buffers later

This is the persistent state needed for reproducible SIL bus behavior.

### CAN Interface Shape

CAN should be modeled only as:

- bus-level commands
- message/frame-level commands
- schedule/job-level commands

It should not be modeled as:

- `get can.<bus>.<signal>`
- `set can.<bus>.<signal>`
- any other CAN-through-signal-namespace interface

DBC may still exist later as a codec/helper for CAN tooling, but not as a projected signal surface in the main runtime interface.

### External Traffic

External VCAN traffic is allowed.

This is an explicit tradeoff:

- users can monitor traffic with external GUIs/tools
- users can inject traffic from outside `agent-sim`
- this may reduce determinism if used irresponsibly

That is acceptable. The product should document the tradeoff rather than trying to forbid it.

## Instance Daemon Restrictions in Env Mode

When an instance is attached to a running env, commands should be split into two groups.

### Still allowed directly against an instance

- `info`
- `signals`
- native signal `get`
- native signal `set`
- `reset`
- other clearly device-local inspection commands

These remain useful as debug probes into one device and do not conflict with env ownership of time or transport topology.

### Rejected and redirected to env-level control

- instance-local `time ...`
- instance-local CAN lifecycle/transport commands
- transport attach/detach style commands owned by env topology
- instance close/remove operations that would mutate env membership behind the env daemon's back

The default behavior should be a clear error that explains which env-level command family to use instead.

### Candidate commands that may move to env control later

- power-state style controls
- any future command that changes topology, timing, or transport ownership

`reset` is intentionally allowed at both the env level and the instance level for the near term. This supports convenience and simple power-cycle-like flows while the broader env orchestration model is still being introduced. It can be revisited later if stricter env ownership proves necessary.

## Transport Placement Model

The system should not assume that every transport looks like CAN.

### Bus-like transports

Examples:

- CAN
- possibly LIN later

These fit naturally as env-owned transport managers because they are:

- shared by multiple participants
- schedule-sensitive
- easy to observe centrally

### Point-to-point transports

Examples:

- UART
- direct socket-style IPC between two instances

These may not require a dedicated transport daemon.

Preferred direction:

- the env daemon owns control plane concerns:
  - topology
  - timing contracts
  - configuration
  - recipe integration
- the data plane may be implemented directly between instance daemons where appropriate

This matches the intended future approach for peer-to-peer IPC and keeps the design flexible.

### Shared fabric / snapshot transports

Examples:

- shared memory channels
- backplane-style env state

These fit better as env-owned facilities, because they are topology-aware and usually coordination-heavy.

## Why Not Separate CAN Daemons First

### Option A: one CAN daemon per bus

Strengths:

- clear process boundary
- fault isolation
- conceptually neat for CAN specifically

Weaknesses:

- another layer of process lifecycle to manage
- another internal RPC boundary
- still requires an env-level time authority above it
- does not generalize well to UART or point-to-point IPC

This option remains a valid later refactor if:

- the env daemon becomes too heavy
- CAN throughput grows enough to justify isolation
- OS-specific integration around CAN becomes significantly more complex

It should not be the starting point.

### Option B: env daemon with internal CAN manager

Strengths:

- one owner for logical time
- one owner for env topology
- one place for bus scheduling
- fewer daemons
- cleaner CLI and recipe model

Weaknesses:

- env daemon gets broader responsibility
- weaker fault isolation unless internal module boundaries are kept clean

This is the preferred starting point.

## Future Features and Fit

### Near-Term Features This Architecture Supports Well

- env-level pause / resume / step / speed
- deterministic cyclic CAN jobs
- one-shot CAN injection from CLI
- CAN traces and replay later
- env-scoped bus monitoring
- recipe-driven transport orchestration

### UART / Serial

Likely future needs:

- virtual UART links between instances
- optional host-side bridge for console/debug access
- later PTY integration if needed

Fit:

- env daemon owns topology and timing
- direct instance-to-instance link data path is acceptable
- optional helper process can be introduced later for OS-heavy PTY bridging if required

### Direct IPC / Socket Links

Likely future needs:

- peer-to-peer links between specific instances
- simple transport without a sniffable shared bus

Fit:

- env daemon owns registration, policy, and orchestration
- data flow may be direct between instances
- observability can be added via explicit endpoint telemetry rather than by forcing a shared daemon

### Recording / Replay

A persistent env daemon is the right place to coordinate:

- env timeline
- transport events
- cross-device trace capture
- replay later

### Fault Injection

An env-owned transport layer can later support:

- drops
- delays
- jitter
- bus disconnection
- message corruption

This is much easier with an env control plane than with loose per-instance clocks.

## CLI Direction

There should remain only one user-facing CLI:

```sh
agent-sim ...
agent-sim can ...
agent-sim env ...
```

No separate `agent-can` binary is planned.

Env-level time commands should mirror the current instance-level time surface as closely as possible so users do not need to learn two different timing models.

Reset should be available at both levels for the near term:

- env-level reset for broad orchestration
- instance-level reset for convenience and power-cycle-like testing flows

In env mode, CAN commands should target env-owned resources rather than talking directly to a device instance unless the command is explicitly device-local.

The CAN subcommand should be implemented with strong internal isolation:

- transport manager logic separated from the rest of env orchestration
- message/schedule model separated from CLI presentation
- minimal coupling to unrelated instance logic

This preserves the option of later extracting the CAN subsystem into a dedicated crate or standalone CLI wrapper without making that a product requirement now.

Examples of likely env-owned CAN operations:

- `can load-dbc`
- `can list`
- `can send`
- `can inspect`
- `can schedule add`
- `can schedule update`
- `can schedule remove`
- `can schedule list`
- `can schedule stop`
- `can buses`
- `can trace` later

Exact syntax is intentionally deferred.

## Migration Direction

### Phase A: Introduce env daemon

- add persistent env daemon lifecycle
- move env membership out of CLI-only fan-out mode
- keep existing instance daemons

### Phase B: Move env time authority into env daemon

- env daemon owns pause/start/step/speed
- instance daemons become stepped workers in env mode

### Phase C: Move CAN ownership into env daemon

- env daemon attaches env-declared VCAN interfaces
- instance daemons stop owning env-mode CAN sockets directly
- one-shot CAN injection routes through env

### Phase D: Add persistent CAN scheduling

- cyclic jobs
- message timers
- last/next send tick bookkeeping
- recipe integration

### Phase E: Remove CAN signal projection from planned interface

- remove `get can.<bus>.<signal>` and `set can.<bus>.<signal>` from the target architecture
- keep CAN interactions on the dedicated CAN command surface only
- if DBC survives, it does so only as a message codec/helper, not as a signal interface

### Phase F: Update migration/docs/demo coverage

- update the migration guidance/spec to match the env-daemon and CAN-scaffolding direction
- expand the HVAC example so it can demonstrate env configuration rather than only single-instance signal control
- land HVAC demo expansion inline with the feature work that introduces the new env/CAN capabilities, not as a follow-up cleanup phase
- add demo/test scenarios that exercise:
  - env startup
  - env-level time control
  - env-owned CAN interaction
  - cyclic message setup/teardown
  - multi-instance behavior under one env

## Risks

- env daemon can become too broad if internal boundaries are weak
- env-mode and standalone-mode semantics must be very explicit
- external VCAN traffic can reduce determinism
- migration from current per-instance CAN ownership must be staged carefully

## Mitigations

- keep transport managers modular inside the env daemon
- define env mode vs standalone mode explicitly
- document external traffic tradeoffs
- only split transport managers into subprocesses if there is a real operational reason

## Deliverables

- [ ] Persistent env daemon lifecycle and protocol
- [ ] Env-owned logical time in env mode
- [ ] Instance daemons as stepped workers in env mode
- [ ] Env-owned CAN manager attached to all env-declared VCANs
- [ ] Persistent CAN schedule/job state across CLI calls
- [ ] `agent-sim can ...` routed through env-owned CAN control in env mode
- [ ] Clear env-mode vs standalone-mode command semantics
- [ ] Migration plan from current stateless `env start` fan-out model
- [ ] Update migration guidance/spec to reflect the new env/CAN model
- [ ] Expand the HVAC example to demonstrate env configuration and env-owned transport behavior
- [ ] Land HVAC demo/test expansion alongside the feature changes that require it
- [ ] Add tests that cover env startup, env time control, and cyclic CAN lifecycle

## Open Questions

1. Should env-mode instance-local `time ...` commands hard fail, or proxy through env control?
2. For future UART links, do we want the first implementation to be fully env-owned or env-controlled with direct worker-to-worker data flow?
3. At what threshold would CAN warrant extraction into a dedicated worker process later:
   - throughput
   - reliability isolation
   - OS bridge complexity
   - trace volume
