# PRD: Terminology Standardization and Migration Documentation

Status: draft

Related:

- `docs/env-daemon-prd.md`
- `docs/prd.md`
- existing migration guidance/specs

## Summary

This PRD covers two adjacent product concerns:

1. standardizing the user-facing term `instance`
2. expanding migration documentation so it stays aligned with the env daemon and CAN scaffolding direction

The goal is to improve product language, reduce conceptual confusion, and make migration guidance strong enough to support the current architecture.

## Problem

### Terminology

`instance` is the right user-facing term for a running simulated device:

- one firmware/device can have multiple instances
- an env contains multiple instances
- an instance is a running simulated node with its own state

### Migration docs

The migration story needs to be more explicit as the product direction changes.

Upcoming changes now touch:

- env orchestration
- CAN scaffolding
- DBC usage expectations
- terminology standardization
- demo and test expectations

If migration guidance lags behind implementation, users and future coding agents will follow stale patterns.

## Goals

- Use `instance` consistently in user-facing product language
- Land a clean, repo-wide cutover to `instance`
- Keep migration guidance aligned with the shipped surface
- Expand migration docs to reflect the env daemon and dedicated CAN scaffolding direction
- Require demo/doc/test updates to land alongside the feature work they describe

## Non-Goals

- Renaming every internal symbol immediately
- Rewriting history across all existing docs in one pass
- Solving every naming question in one document

## Terminology Model

Preferred product vocabulary:

- `device`: a device definition or firmware identity
- `instance`: a running simulated instance of a device
- `env`: a coordinated collection of instances plus topology/transports

## Recommendation

Adopt `instance` as the single user-facing term.

### User-facing direction

New docs, PRDs, examples, and help text should prefer:

- `instance`
- `instance list`
- `--instance`
- envs containing `instances`

### Cutover direction

Land a single preferred surface with no compatibility aliases:

- `--instance`
- `instance list`
- `instances = [...]`
- `instance = "..."` in recipe/default targeting

Remove older terminology rather than preserving it as an alias.

### Internal code direction

Internal symbols do not need to be renamed immediately.

Recommended order:

1. rename docs/help text first
2. land the final CLI/config cutover
3. rename internal code opportunistically when touching relevant areas

This keeps churn controlled.

## Suggested Surface-by-Surface Plan

### Phase 1: Documentation-first standardization

Change new and actively maintained docs to prefer `instance`.

Examples:

- architecture docs
- PRDs
- README concepts section
- examples and walkthroughs

### Phase 2: CLI/config cutover

Rename the user-facing surface directly:

- `--instance`
- `instance list`
- `instances = [...]`
- `instance = "..."` in recipe/default targeting

No compatibility aliases.

### Phase 3: Internal renaming over time

Only rename internal identifiers when:

- the area is already being refactored
- the rename reduces confusion materially
- the churn is worth it

No repo-wide rename for its own sake.

## Migration Documentation Requirements

The migration guidance needs to evolve from "what symbols exist" to "how to participate in the new orchestration model".

It should cover:

- preferred terminology (`instance`)
- env daemon model vs old CLI fan-out assumptions
- dedicated CAN scaffolding instead of CAN-through-signal projection
- DBC as CAN helper/codec rather than primary signal interface
- expectations for examples, env wiring, and tests

## Migration Doc Deliverables

At minimum, migration documentation should include:

- a current migration guide for DLL/runtime adopters
- a coding-agent-oriented migration prompt/spec that reflects current direction
- explicit notes about deprecated or discouraged patterns
- examples for env-based setups, not only single-instance setups

## Demo and Test Policy

Demo and test expansion should not trail the feature work that makes them necessary.

Specific requirement:

- HVAC demo expansion must land inline with the feature changes that introduce the new env/CAN behavior

This means:

- if env daemon support lands, the demo/examples should demonstrate env usage in the same stream of work
- if CAN scaffolding lands, the demo/tests should exercise it in the same stream of work
- migration docs should be updated alongside the same work, not later as cleanup

## What Migration Guidance Should Explain

For users:

- how to think about `instance` vs `device` vs `env`
- how env-level control differs from direct instance control
- how CAN is now modeled

For DLL authors:

- what the runtime expects from env-aware integrations
- how CAN support should be exposed
- what example/demo quality bar is expected

For future coding agents:

- which old assumptions are no longer valid
- what terms to use in new docs and code comments
- what follow-on docs/examples/tests must be updated with feature changes

## Risks

- temporary mixed terminology during implementation
- migration docs can still drift if ownership is vague

## Mitigations

- make `instance` the required wording everywhere user-facing
- finish with one clean terminology pass
- require doc/demo/test updates in the same workstream as feature delivery

## Deliverables

- [ ] Preferred terminology documented: `instance`
- [ ] Final CLI/config terminology documented
- [ ] Migration guidance updated to match env daemon and CAN scaffolding direction
- [ ] Coding-agent migration prompt/spec refreshed
- [ ] README and active docs updated to use preferred terminology where practical
- [ ] HVAC env/demo coverage updated inline with feature delivery, not afterward

## Open Questions

1. How much internal renaming is worth doing once user-facing terminology is cleaned up?
