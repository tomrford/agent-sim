# Shared VCAN Env Simplification

## Summary

This document captures the intended redesign of env-mode CAN handling in `agent-sim`.

Today, env mode centralizes CAN ownership in the env daemon. The env daemon binds to the CAN interface, receives external traffic, forwards frame batches to instance daemons over RPC, collects instance TX over RPC, and writes those frames back to the bus.

The proposed direction is to simplify this model:

- each env member instance binds directly to the shared CAN interface itself
- the CAN interface remains the actual communication medium between instances
- the env daemon remains responsible for env-owned time control, agent-facing CAN commands, schedules, and bus inspection
- exact intra-step CAN visibility is not a primary goal
- realism, lower overhead, and a simpler external boundary are preferred over strict deterministic replay

This is intended to be an 80% SIL tool for fast agent-guided iteration before hardware testing, not a bit-exact plant or bus simulator.

## Problem Statement

The current env CAN design is more controlled than necessary for the product goal:

- all CAN traffic is funneled through the env daemon
- every env-managed CAN hop pays extra overhead for routing, buffering, and RPC
- instances in env mode do not behave like independent participants on a shared bus
- the user-facing system boundary is less natural than "all devices are on the same VCAN"

This complexity was introduced to preserve tighter env-controlled delivery semantics. Based on current product goals, that level of control is not worth the added complexity and overhead.

## Product Goals

- Make env-mode CAN feel like a real shared bus with separate simulated devices.
- Keep the external integration boundary simple: users and third-party tools should interact with the same CAN interface the sims use.
- Reduce per-tick CAN overhead and remove the env daemon as the CAN routing bottleneck.
- Preserve env-owned time control across multi-instance tests.
- Preserve enough reproducibility for fast firmware iteration:
  state machines should advance in roughly correct tick/time windows.
- Keep the mental model easy to explain to users and agents.

## Non-Goals

- Bit-exact replay of all multi-instance interactions.
- Exact intra-step CAN ordering guarantees across all instances.
- Protection against user-caused interference from external tools talking on the same CAN interface.
- Accurate modeling of real-world analog effects, control loop noise, transformer behavior, or bus timing jitter.

## Current Behavior

In env mode today:

- the env daemon opens the configured CAN interface
- instance daemons do not attach directly to that bus
- the env daemon drains bus traffic and builds per-instance `can_rx` batches
- the env daemon steps each instance through worker RPC
- each instance returns `can_tx` batches to the env daemon
- the env daemon writes those frames onto the CAN interface

This gives the env daemon strong control over what each instance sees within a step, but it makes env mode more complex and more expensive than necessary.

## Proposed Direction

### Core Decision

Adopt a shared-bus model for env mode:

- each env member instance binds directly to the env-configured CAN interface
- CAN traffic between instances flows over the actual CAN interface, not env RPC
- the env daemon no longer brokers instance-to-instance CAN traffic

### Env Daemon Responsibilities

The env daemon should continue to own:

- env lifecycle
- env-owned time coordination
- agent-facing `env can ...` commands
- CAN schedules
- bus inspection / DBC decode helpers

The env daemon should stop owning:

- per-frame routing between instances
- per-instance CAN delivery decisions during each step

### Instance Responsibilities

Each instance daemon should:

- attach directly to the mapped env CAN interface during env bootstrap
- drain incoming CAN traffic from its local socket at step time
- inject drained frames into the DLL via `sim_can_rx(...)`
- run `sim_tick()`
- collect outbound DLL frames via `sim_can_tx(...)`
- flush those frames to the CAN interface

## Why This Better Matches Product Intent

- It is closer to the real-world mental model: separate devices sharing a bus.
- It makes third-party integration natural: external tools can read/write the same interface the sims use.
- It reduces the amount of CAN-specific logic concentrated in the env daemon.
- It avoids paying serialization/RPC overhead for CAN frames that could have remained on the kernel/driver bus path.
- It matches the intended use of the tool as a fast first-pass firmware test harness rather than a high-fidelity deterministic simulator.

## Timing and Reproducibility Position

We explicitly do not require exact reproducibility at the level of:

- "if I reset and run exactly N ticks, every device must always end in the exact same state"
- "if device A transmits during a step, device B must or must not observe it in that same step in a perfectly repeatable way"

We do care about:

- approximate state-machine timing
- roughly correct tick-window behavior
- useful agent feedback before hardware testing

This means looser CAN visibility semantics are acceptable if they materially simplify the system and improve realism/performance.

## Simple vs Phased Design

There are two viable versions of the shared-VCAN design.

### Option A: Simple Direct Step

Each instance runs its normal local step when env asks it to advance:

1. drain local CAN RX queues
2. inject frames into the DLL with `sim_can_rx(...)`
3. run `sim_tick()`
4. collect DLL TX with `sim_can_tx(...)`
5. write TX frames to the CAN interface

#### Pros

- smallest change from the existing standalone instance path
- easiest implementation
- lowest conceptual overhead
- likely good enough for the intended use case

#### Cons

- if one instance flushes TX before another instance has drained RX, same-step visibility can vary based on scheduling
- slightly weaker reproducibility across multi-instance runs

### Option B: Phased Direct Step

Split each env step into barriers:

1. all instances `DrainInputs`
2. all instances `RunTick`
3. all instances `FlushOutputs`

This gives a cleaner semantic rule:

- frames already present on the bus before the step are eligible to be seen this step
- frames emitted during the step become visible on the next step

#### Pros

- preserves more step-to-step consistency without brokering CAN through env
- reduces same-step visibility variance across instances
- still keeps VCAN as the real transport

#### Cons

- requires staging buffers inside each instance daemon
- requires new worker actions or equivalent internal step phases
- requires env-side phase/barrier coordination
- increases implementation complexity relative to Option A

## Complexity Assessment

Option B is not dramatically harder than Option A, but it is meaningfully more complex.

### Option A Complexity

Moderate. This mostly reuses the existing standalone local CAN path in env-controlled stepping.

Expected changes:

- env bootstrap attaches each mapped instance bus directly to the env CAN interface
- env worker `Step` stops carrying CAN payloads
- env tick becomes "fan out step, wait for completion"
- env daemon keeps its own CAN socket only for send/schedule/inspect duties

### Option B Complexity

Moderate-plus. Still tractable, but no longer a minimal simplification.

Expected additional changes beyond Option A:

- add per-instance staged RX buffers
- add per-instance staged TX buffers
- split worker stepping into multiple phases or introduce a phased internal state machine
- define schedule/env-send visibility relative to phase boundaries
- keep phase behavior clear in docs and tests

### Recommendation

Start with Option A unless we have a strong concrete need for same-step visibility stability.

Option B should remain available as a follow-on refinement if:

- users notice same-step ordering variance
- replayability becomes more important than currently expected
- we want a cleaner step contract without reintroducing env-brokered CAN

## CAN Queue / Mailbox Model

The DLL does not poll the bus directly. The runtime owns bus I/O and pushes frames into the DLL.

The ABI contract is effectively:

1. host drains received frames from the CAN interface
2. host delivers them to the DLL via `sim_can_rx(...)`
3. host calls `sim_tick()`
4. host pulls queued transmit frames from the DLL via `sim_can_tx(...)`
5. host writes those frames to the CAN interface

This means:

- draining does not need to happen immediately at frame arrival time
- the host can drain at step boundaries
- the DLL-side firmware model can still maintain its own mailbox/FIFO/interrupt abstractions internally

For this project, draining at step boundaries is acceptable.

## Validation Position

Under the simplified design:

- each instance daemon remains the boundary validator for the frames it receives from its CAN attachment
- env-owned CAN sends and schedules should still be validated before transmission
- env inspect becomes observational rather than authoritative

We do not need env to be the universal CAN routing/validation chokepoint.

## External Tooling and Interference

External tools should be able to:

- write frames to the shared CAN interface
- observe frames emitted by env schedules and by instance daemons

This is desirable because it makes the system act more like separate real devices on a shared bus.

Interference from external writers is acceptable and considered user-managed behavior, especially for Linux VCAN use on the local machine.

## Proposed Implementation Shape

### Phase 1: Simplify to Direct Shared CAN

- attach each env member instance bus directly to the configured env CAN interface
- remove CAN frame payloads from worker `Step`
- reuse local instance `drain -> tick -> flush` behavior during env-controlled steps
- keep env daemon CAN socket for agent sends, schedules, and inspect
- keep shared-state handling as-is unless separate simplification is warranted

### Phase 2: Optional Phased Stepping

Only if needed:

- split worker stepping into `DrainInputs`, `RunTick`, and `FlushOutputs`
- add staging buffers per instance
- define and document next-step visibility for TX produced during the current step

## Open Questions

- Should env inspect report only frames observed on its own socket plus env-local sends, or should we also keep some explicit env-local frame state for better UX?
- Do we want to keep the redesign Linux-first for VCAN and adapt Windows/PEAK semantics later, or require both paths to move together?
- Should env startup reject certain mixed-bus capability combinations, or is per-instance validation enough?
- Should phased stepping remain a documented future option only, or should we build the worker API so phased stepping can be added without another protocol break?

## Decision Summary

Preferred direction:

- direct shared CAN attachments for env member instances
- env-owned time, send, schedule, and inspect
- looser CAN timing semantics
- realism and simplicity over exact replay

Preferred implementation starting point:

- Option A: simple direct step

Future refinement if justified:

- Option B: phased direct step
