# PRD: Cleanup, Simplification, and Refactor Backlog

Status: living backlog

## Purpose

This document is the working list of cleanup, simplification, contract clarification,
and refactor items for `agent-sim`.

It is broader than a feature PRD:

- some items are correctness or contract issues
- some are architectural simplifications
- some are performance concerns
- some are small maintainability or DX cleanups

The goal is to keep one ordered backlog of engineering debt and refactor ideas that
can be worked through in focused batches.

## Guiding Assumptions

### ABI / DLL model

`agent-sim` is an open-source host tool. A user installs `agent-sim`, builds their own
firmware simulation as a shared library with an `agent-sim`-compliant ABI, and then
uses the runtime/CLI to drive that library.

This implies:

- the DLL boundary is **trusted in intent**
- `agent-sim` is **not** trying to sandbox hostile plugins
- users should **not** need to modify `agent-sim` source to integrate their own DLLs

At the same time, the host should avoid introducing its own ambiguity:

- if a DLL violates the ABI, `agent-sim` should not silently invent meaning on top of
  bad metadata or raw payloads
- the focus is **boundary honesty and deterministic host behavior**, not adversarial
  hardening

### Environment scope

This PRD only tracks work that is realistically fixable inside the repo.

It does **not** track cloud-host limitations that require changes outside the repo
(for example kernel features not exposed by the Cursor host environment).

## How to Use This Doc

- Treat each item as a problem statement, not as a committed design.
- When starting implementation work, pull one chunk or a small related subset into a
  focused task/PR.
- Prefer fixing root causes over local patches.
- Add new items here as they are discovered.
- Mark items done or remove them once the underlying problem is genuinely resolved.

## Priority Labels

- `P0` - correctness, contract, or misleading host-behavior risk
- `P1` - important architectural, performance, or portability issue
- `P2` - maintainability, ergonomics, examples, or cleanup

## Recommended Implementation Order

1. Boundary honesty and contract clarity
2. Small cleanup and developer/agent QoL
3. Envd architecture and scaling
4. Transport portability
5. ABI/example polish

This order is intentional:

- first remove ambiguity at the host boundary
- then take easy cleanup wins
- then do the larger envd structural work
- then add new transport backends on top of a cleaner transport/control model
- then finish with lower-urgency ABI/example polish

## Chunk 1 - Boundary honesty and contract clarity

### Why this chunk comes first

These items are the highest-value correctness work, but they should be understood as
"make the host behavior honest" rather than "treat user DLLs as hostile".

The key question is:

> when a DLL is wrong or ambiguous, does `agent-sim` fail clearly, or does it create
> plausible-looking behavior of its own?

### Items

| ID | Priority | Problem | Why it matters | Possible direction |
| --- | --- | --- | --- | --- |
| R2 | P0 | DLL metadata is not fully validated on load. Duplicate signal names/IDs, duplicate CAN/shared names, and ambiguous metadata are accepted. | Later lookups become order-dependent and the host can behave arbitrarily on top of a broken catalog. | Add a single load-time validation pass for uniqueness and stronger contract checks before the project becomes usable. |
| R3 | P0 | Shared-state semantics are underspecified, especially around `BUFFER_TOO_SMALL`, dense vs sparse snapshots, and empty/default slots. | The runtime and template currently rely on implied behavior that is not written into `sim_api.h`. | Tighten the ABI contract in `include/sim_api.h`, then align runtime, template, and tests with the chosen semantics. |
| R5 | P1 | Coverage is strong for happy-path CLI/runtime flows, but weaker for malformed DLL metadata, duplicate IDs/names, and bad shared-state payloads. | Risky boundary code is under-tested relative to its failure modes. | Add focused negative tests around ABI contract violations and loader validation. |
| R1 | P1 | `SimSharedSlot::from_raw()` coerces the nested `SimValue` tag instead of treating the raw payload as either valid or invalid. | This risks turning broken ABI data into plausible host-side values, which is misleading even in a trusted-plugin model. | Split raw ABI decoding from domain conversion and reject mismatched shared slot tags instead of coercing them. |

## Chunk 2 - Small cleanup and developer/agent QoL

### Why this chunk comes second

These are mostly low-risk cleanup wins. They reduce friction, keep the code easier to
work with, and improve the intended workflow where a user installs `agent-sim` and has
an agent build and test a compliant DLL for them.

### Items

| ID | Priority | Problem | Why it matters | Possible direction |
| --- | --- | --- | --- | --- |
| C1 | P2 | Clippy currently fails on `items_after_test_module` in `runtime/src/daemon/lifecycle.rs`. | The full Rust gate is not clean, even though builds/tests pass. | Reorder items so test modules come last and keep the clippy gate green. |
| A6 | P2 | `list_sessions()` returns an unnamed 4-tuple that is destructured repeatedly. | This is easy to misuse and hides intent. | Replace the tuple with a named struct such as `SessionInfo`. |
| C2 | P2 | CAN hex parsing exists in both the instance daemon path and the env daemon path. | Small duplicated helpers tend to drift over time. | Pull shared parsing/validation helpers into one module. |
| A5 | P2 | `CliArgs` mixes public CLI flags with internal daemon/bootstrap flags used only for subprocess re-exec. | The user-facing surface and internal process control surface are tangled together. | Separate external CLI parsing from internal bootstrap argument parsing. |
| A8 | P2 | `template/src/sim_types.zig` and `examples/hvac/src/sim_types.zig` are copy-pasted ABI mirrors. | ABI drift can happen silently as the header evolves. | Share one Zig ABI mirror or generate/verify the mirrors from `include/sim_api.h`. |
| C3 | P2 | Zig signal catalogs and read/write dispatch are hand-maintained in multiple places. | Metadata and behavior can drift out of sync over time. | Explore a more table-driven or enum-backed pattern for signals in the template and examples. |
| D1 | P2 | Agent-consumable guidance is currently scattered across docs only. There is no dedicated "build a compliant DLL" / "use agent-sim effectively" skill or equivalent lightweight in-repo guide optimized for autonomous agents. | The intended workflow is "install agent-sim, then have an agent build and test a DLL against it". Better agent-facing guidance would reduce friction and repeated context loading. | Add one or more `skill.md`-style guides and/or a lightweight CLI-discoverable docs surface for DLL authoring and runtime usage. |

## Chunk 3 - Envd architecture and scaling

### Why this chunk comes third

This is the biggest structural refactor area. The env daemon works, but its current
shape is closer to "feature assembled successfully" than "clean long-term control plane".

These should be taken one at a time, not as a single rewrite.

### Suggested internal order

1. `A3` - split the god module
2. `A4` - reduce lock-centric coordination
3. `A1` - improve per-tick IPC shape
4. `A7` - revisit serial bootstrap/step execution
5. `A2` - protocol role cleanup if still worthwhile after the earlier refactors

### Items

| ID | Priority | Problem | Why it matters | Possible direction |
| --- | --- | --- | --- | --- |
| A3 | P1 | `runtime/src/envd/server.rs` is a large god module containing bootstrap, dispatch, ticking, CAN logic, scheduling, lifecycle helpers, and shared-channel orchestration. | The file is hard to navigate, hard to review, and likely to resist future refactors. | Split envd into submodules similar to the instance daemon layout (`dispatch`, `tick`, `can`, `lifecycle`, `shared`, etc.). |
| A4 | P1 | Envd uses `Arc<Mutex<EnvState>>` where the instance daemon already uses a cleaner action-channel pattern. | Request handling and ticking can contend on the same lock, and state is held across async work more easily. | Move envd toward message-passing / actor-style coordination, or at minimum stop holding global state across awaited I/O. |
| A1 | P1 | Envd opens a fresh Unix socket to each instance on every step. At higher tick rates and instance counts this becomes a large connect/disconnect churn. | This creates avoidable syscall overhead and makes env stepping more expensive than necessary. | Consider persistent per-instance connections or a worker channel model for env-owned stepping. |
| A7 | P1 | Env instance bootstrap and per-tick stepping are done serially even where the work is independent. | This limits scalability as env size grows. | Explore concurrent bootstrap/step fan-out while keeping deterministic env semantics. |
| A2 | P1 | One `Action` enum is shared by both instance daemons and env daemons, even though each side rejects a large subset of variants. | Invalid states are representable and dispatch logic becomes less self-documenting. | Split protocol surfaces by role, or introduce stronger typed sub-enums with shared serialization helpers. |

## Chunk 4 - Transport portability

### Why this chunk comes after envd cleanup

Transport portability will be easier if envd and the transport/control boundaries are
cleaner first. This is where Windows CAN support via Peak CAN fits.

### Items

| ID | Priority | Problem | Why it matters | Possible direction |
| --- | --- | --- | --- | --- |
| T1 | P1 | CAN transport is currently Linux SocketCAN-only, which blocks native Windows CAN usage. One requested direction is support for Peak CAN on Windows via a maintained wrapper crate. | Cross-platform support is a stated product goal, but practical CAN support is Linux-only today. | Introduce a transport abstraction above `CanSocket` and add a Windows backend for Peak CAN while preserving SocketCAN on Linux. |

## Chunk 5 - ABI and example polish

### Why this chunk comes last

These are worth doing, but they are lower urgency than boundary honesty, cleanup, and
envd architecture work.

### Items

| ID | Priority | Problem | Why it matters | Possible direction |
| --- | --- | --- | --- | --- |
| R4 | P2 | `sim_api.h` uses plain C enums for `SimStatus` and `SimType`, while Rust and Zig assume `u32` layout. | The ABI is less portable and less explicitly stable than it appears, but this is lower urgency if users typically build against a coordinated toolchain. | Make enum widths explicit in the C ABI or add a clearer compatibility story. |
| C4 | P2 | `SIM_API_VERSION_MAJOR/MINOR` exist in the header but are not actively negotiated by the runtime. | Version drift would fail late rather than early and explicitly, but this is lower urgency than the ambiguity issues above. | Add an exported version check or compatibility handshake during project load. |
| R6 | P2 | HVAC example semantics are slightly inconsistent: fault `error_code` appears to survive a power cycle, and invalid `mode` values are stored/read raw but executed as `auto`. | The example becomes a de facto spec and can mislead future feature work or tests. | Tighten example semantics and add regression tests for power-cycle and invalid-mode behavior. |

## Notes

- This backlog intentionally mixes big and small items.
- Some items overlap; that is fine for now.
- If implementation changes the problem statement materially, update this doc rather
  than preserving stale wording.
