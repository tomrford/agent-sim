# PRD: Terminology Refresh and Migration Documentation

Status: draft

Related:

- `docs/env-daemon-prd.md`
- `docs/prd.md`
- existing migration guidance/specs

## Summary

This PRD covers two adjacent product concerns:

1. shifting the preferred user-facing term from `session` to `instance`
2. expanding migration documentation so it stays aligned with the new env daemon and CAN scaffolding direction

The goal is not a risky big-bang rename. The goal is to improve product language, reduce conceptual confusion, and make migration guidance strong enough to support the upcoming architecture work.

## Problem

### Terminology

`session` works technically, but it is not the best user-facing term for a running simulated device.

Problems with `session`:

- it sounds transient and CLI-connection-oriented rather than device-oriented
- it does not read naturally in multi-device envs
- it competes conceptually with `env`, `device`, and future transport concepts
- it makes docs more abstract than necessary for users who think in terms of running instances of firmware

`instance` is a better fit for the likely product language:

- one firmware/device can have multiple instances
- an env contains multiple instances
- an instance is a running simulated node with its own state

### Migration docs

The migration story needs to be more explicit as the product direction changes.

Upcoming changes now touch:

- env orchestration
- CAN scaffolding
- DBC usage expectations
- potential terminology changes
- demo and test expectations

If migration guidance lags behind implementation, users and future coding agents will follow stale patterns.

## Goals

- Prefer `instance` over `session` in user-facing product language
- Avoid a flag day rename that creates unnecessary churn
- Preserve compatibility where practical during transition
- Clearly define which surfaces move first and which stay stable longer
- Expand migration docs to reflect the env daemon and dedicated CAN scaffolding direction
- Require demo/doc/test updates to land alongside the feature work they describe

## Non-Goals

- Renaming every internal symbol immediately
- Rewriting history across all existing docs in one pass
- Breaking existing CLI/scripts without a compatibility window
- Solving every naming question in one document

## Terminology Model

Preferred product vocabulary:

- `device`: a device definition or firmware identity
- `instance`: a running simulated instance of a device
- `env`: a coordinated collection of instances plus topology/transports

Avoid using `session` in new user-facing docs unless needed for backward compatibility context.

## Recommendation

Adopt `instance` as the preferred user-facing term, but stage the rollout.

### User-facing direction

New docs, PRDs, examples, and help text should prefer:

- `instance`
- `instance list`
- `--instance`
- envs containing `instances`

### Compatibility direction

During migration, support old terminology where needed:

- `--session` remains accepted as an alias
- `session list` can remain as an alias if command naming changes
- config may temporarily accept both `sessions` and `instances`

The product should guide users toward `instance`, not force everyone to rewrite automation immediately.

### Internal code direction

Internal symbols do not need to be renamed immediately.

Recommended order:

1. rename docs/help text first
2. add CLI/config aliases
3. rename internal code opportunistically when touching relevant areas

This keeps churn controlled.

## Suggested Surface-by-Surface Plan

### Phase 1: Documentation-first rename

Change new and actively maintained docs to prefer `instance`.

Examples:

- architecture docs
- PRDs
- README concepts section
- examples and walkthroughs

When old terminology must be mentioned, use explicit compatibility wording such as:

- "instance (currently called `session` in some CLI surfaces)"

### Phase 2: CLI aliasing

Add user-facing aliases where worthwhile:

- `--instance` alias for `--session`
- `instance list` alias for `session list`
- error/help text should prefer `instance`

This phase is about ergonomics, not forced deprecation.

### Phase 3: Config aliasing

If env/config syntax is touched for the env daemon work, consider supporting:

- `instances = [...]`
- `sessions = [...]` as a compatibility alias

Do not do this unless the parser change is low-risk and justified. Config aliasing is useful, but it is also a source of ambiguity if done carelessly.

### Phase 4: Internal renaming over time

Only rename internal identifiers when:

- the area is already being refactored
- the rename reduces confusion materially
- the churn is worth it

No repo-wide rename for its own sake.

## Migration Documentation Requirements

The migration guidance needs to evolve from "what symbols exist" to "how to participate in the new orchestration model".

It should cover:

- preferred terminology (`instance`)
- current compatibility terms (`session`)
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
- which CLI terms are preferred vs compatibility aliases
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

- partial rename can create temporary inconsistency
- aliasing can create ambiguity if error/help text is unclear
- config dual-support can become permanent if not documented carefully
- migration docs can still drift if ownership is vague

## Mitigations

- make `instance` the preferred wording everywhere new
- explicitly label old terms as compatibility terms
- avoid unnecessary config aliasing unless it clearly helps
- require doc/demo/test updates in the same workstream as feature delivery

## Deliverables

- [ ] Preferred terminology documented: `instance` over `session`
- [ ] Compatibility strategy defined for CLI/config surfaces
- [ ] Migration guidance updated to match env daemon and CAN scaffolding direction
- [ ] Coding-agent migration prompt/spec refreshed
- [ ] README and active docs updated to use preferred terminology where practical
- [ ] HVAC env/demo coverage updated inline with feature delivery, not afterward

## Open Questions

1. Should the CLI eventually make `instance` the canonical command/flag and treat `session` purely as an alias?
2. Should env config eventually standardize on `instances`, or keep `sessions` for stability?
3. How much internal renaming is worth doing once user-facing terminology is cleaned up?
