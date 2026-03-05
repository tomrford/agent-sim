# PRD: Flash Memory Initialization & Device Configuration

Status: draft

Related:

- `docs/env-daemon-prd.md`
- `docs/terminology-and-migration-prd.md`
- `docs/template-guide.md`

## Context

`agent-sim` models firmware as shared libraries loaded into per-device runtime processes. Today, startup is driven by the `load` path in the Rust runtime:

1. the CLI builds `Action::Load { libpath, env_tag }`
2. `runtime/src/connection.rs` bootstraps a session daemon for that library
3. `runtime/src/daemon/mod.rs` calls `Project::load(libpath)`
4. `runtime/src/sim/project.rs` binds symbols, reads metadata, then calls `sim_init()`

There is currently no mechanism for the host runtime to push pre-existing non-volatile data into a library before `sim_init()` runs. Adapter authors must hardcode calibration/configuration data in Zig source or leave it zeroed.

In real embedded systems, firmware reads static data from flash at fixed addresses. When porting such firmware into `agent-sim`, the adapter author needs a way to declare a byte-addressable flash region and have the runtime populate it from standard file formats before boot, just like flashing an MCU before power-on.

This feature also introduces a `device` abstraction in config: a named bundle of a library path plus flash configuration. Env definitions then reference devices instead of repeating raw library paths inline.

Terminology note:

- This document prefers `instance` for the running thing and `device` for the reusable definition, following `docs/terminology-and-migration-prd.md`.
- When referring to current code surfaces, it still uses the repo's existing names such as `session`, `sessions`, and `env_tag`.

### Non-Goals

- The runtime does not interpret flash contents. It only writes raw bytes.
- No host-side flash read-back in V1.
- No runtime memory-mapped I/O or address-space emulation.
- No env-owned persistent flash state; flashing remains part of device/project load, not transport orchestration.
- No `mint` integration in V1.

---

## Decisions

- **ABI shape**: one optional export, `sim_flash_write(base_addr, data, len)`, called once per resolved region.
- **Optional export pattern**: independent optional symbol, similar in spirit to CAN/shared optional exports but without an all-or-nothing group because there is only one write API.
- **Call ordering**: flash writes happen before `sim_init()`.
- **Load-path integration**: flash must be threaded through the initial load/bootstrap path, not added as a separate post-load RPC. In current code, the daemon is already bootstrapped and `Project::load(...)` has already run before `Action::Load` reaches the request router.
- **Ownership boundary**: flash is device-local state. The env daemon work should route a richer load spec to per-device workers, not take ownership of flash as env-scoped runtime state.
- **Config model**: `[device.<name>]` holds `lib` plus `flash`; `[env.<name>]` references devices for each instance/session entry.
- **File formats (V1)**: Intel HEX, Motorola S-record, and raw binary with explicit base address.
- **Inline values (V1)**: typed scalar values at explicit addresses, serialized directly from TOML.
- **DLL/library-side responsibility**: the adapter author owns flash layout inside `Ctx` and implements the address-to-storage mapping in `sim_flash_write`.

---

## Phase 1: Flash ABI Extension

### Problem

Libraries that model firmware reading from fixed flash addresses have no host-provided preload path. That blocks realistic calibration tables, lookup tables, device config blobs, and A/B test images.

### ABI

New optional export in `include/sim_api.h`:

```c
/**
 * @brief Write a block of data to the library's flash region.
 *
 * Called by the host before sim_init() to populate non-volatile state.
 * May be called multiple times.
 *
 * The library owns its memory layout.
 * Overlapping writes are applied in order (last write wins).
 * Out-of-range writes should return SIM_ERR_INVALID_ARG.
 */
SimStatus sim_flash_write(uint32_t base_addr, const uint8_t *data, uint32_t len);
```

### Capability Detection

In `Project::load`, after binding the required core symbols:

```rust
let sim_flash_write: Option<SimFlashWriteFn> =
    unsafe { library.get(b"sim_flash_write\0") }.ok().map(|symbol| *symbol);
```

Rules:

- If flash is configured for a device and the library does not export `sim_flash_write`, loading fails clearly.
- If the library exports `sim_flash_write` but no flash is configured, nothing special happens.

### Load Sequence Change

Current `Project::load(...)` shape:

```text
dlopen -> bind symbols -> read tick/signal metadata -> sim_init() -> enumerate CAN/shared
```

Target shape:

```text
dlopen -> bind symbols -> read tick/signal metadata -> sim_flash_write() x N -> sim_init() -> enumerate CAN/shared
```

Reading tick duration and signal metadata before flash remains fine because those surfaces are library metadata, not instance state.

### Bootstrap Plumbing

The current load path is important here:

- `runtime/src/cli/commands.rs` builds `Action::Load { libpath, env_tag }`
- `runtime/src/connection.rs` uses that action to call `bootstrap_daemon(...)`
- `runtime/src/daemon/mod.rs` immediately calls `Project::load(libpath)`

Because of that, a separate later action like `FlashWrite { ... }` is the wrong starting point. Flash support should instead introduce a richer load spec that can flow through bootstrap:

```rust
pub struct LoadSpec {
    pub libpath: String,
    pub env_tag: Option<String>,
    pub flash: Vec<ResolvedFlashRegion>,
}
```

Recommended direction:

- extend `Action::Load`
- extend `bootstrap_daemon(...)`
- extend `daemon::run(...)`
- extend `Project::load(...)`

This same load spec can later be routed by the env daemon without changing the core flash semantics.

### Zig Template

Library-side pattern in `adapter.zig`:

```zig
const sim_types = @import("sim_types.zig");
pub const SimStatus = sim_types.SimStatus;

const FLASH_BASE: u32 = 0x0800_0000;
const FLASH_SIZE: u32 = 256 * 1024;

pub const Ctx = struct {
    flash: [FLASH_SIZE]u8 = [_]u8{0xFF} ** FLASH_SIZE,

    // Other volatile runtime state goes here.
};

pub fn flashWrite(ctx: *Ctx, base_addr: u32, data: [*]const u8, len: u32) SimStatus {
    if (base_addr < FLASH_BASE) return .INVALID_ARG;
    const offset = base_addr - FLASH_BASE;
    if (offset + len > FLASH_SIZE) return .INVALID_ARG;
    @memcpy(ctx.flash[offset..][0..len], data[0..len]);
    return .OK;
}
```

Export wrapper in `root.zig`:

```zig
pub export fn sim_flash_write(base_addr: u32, data: ?[*]const u8, len: u32) SimStatus {
    const d = data orelse return .INVALID_ARG;
    if (len == 0) return .OK;
    return adapter.flashWrite(&g_ctx, base_addr, d, len);
}
```

Important template caveat:

- The current template in `template/src/root.zig` does `g_ctx = .{}` inside `sim_init()`.
- The current template in `template/src/adapter.zig` also does `ctx.* = .{}` in `init()` and `reset()`.
- A flash-capable template must change that behavior. Preserving flash cannot be done only by adding `sim_flash_write`; either `root.zig` or the adapter must explicitly preserve non-volatile fields while reinitializing volatile state.

### Deliverables

- [ ] `sim_flash_write` added to `include/sim_api.h`
- [ ] Optional symbol binding in `Project::load`
- [ ] Flash writes injected before `sim_init()`
- [ ] Load/bootstrap path extended to carry flash data/spec
- [ ] Zig template updated with a safe flash-preserving pattern
- [ ] `docs/template-guide.md` updated with flash-specific init/reset guidance
- [ ] Error if flash is configured but `sim_flash_write` is missing
- [ ] Unit test: flash write -> init -> read signal derived from flash data

---

## Phase 2: File Format Parsers

### Problem

Flash inputs come from standard embedded file formats. The runtime needs to convert them into resolved `(base_addr, bytes)` regions.

### Supported Formats

**Intel HEX** (`.hex`, `.ihex`)

- Record types 00, 01, 02, 04
- Per-record checksum validation
- Output: zero or more resolved regions

**Motorola S-record** (`.mot`, `.srec`, `.s19`, `.s28`, `.s37`)

- S1/S2/S3 data records, plus standard header/count/end records
- Per-record checksum validation
- Output: zero or more resolved regions

**Raw binary** (`.bin`)

- Flat byte array
- Requires explicit `base` in config
- Output: one resolved region

### Parser Design

```rust
pub struct ResolvedFlashRegion {
    pub base_addr: u32,
    pub data: Vec<u8>,
}

pub fn parse_intel_hex(content: &str) -> Result<Vec<ResolvedFlashRegion>, FlashParseError>;
pub fn parse_srec(content: &str) -> Result<Vec<ResolvedFlashRegion>, FlashParseError>;
pub fn parse_raw_binary(bytes: &[u8], base: u32) -> ResolvedFlashRegion;
```

Format detection:

- explicit `format = "ihex" | "srec" | "bin"` wins
- otherwise infer from extension
- ambiguous or unsupported cases fail clearly

This parser layer should be reusable from both standalone `load` flows and env/device startup flows.

### Deliverables

- [ ] Intel HEX parser with checksum validation
- [ ] Motorola S-record parser with checksum validation
- [ ] Raw binary loader with explicit base address
- [ ] Format detection from config or file extension
- [ ] Unit tests for valid inputs, corrupt checksums, overlap handling, and address overflow

---

## Phase 3: Load/Bootstrap Plumbing and Device Config

### Problem

Today, `runtime/src/config/recipe.rs` models env membership as:

```rust
pub struct EnvSession {
    pub name: String,
    pub lib: String,
}
```

That is too narrow for flash-aware devices and forces env definitions to mix:

- instance identity
- device identity
- library path
- future device-local boot inputs

Device definitions give that data a stable home and keep env topology focused on wiring.

### Config Schema

```toml
# -- Devices ----------------------------------------------------------------

[device.ecu1]
lib = "./zig-out/lib/libecu1.dylib"
flash = [
    { file = "./calibration.hex", format = "ihex" },
    { file = "./lookup_tables.mot" },
    { file = "./raw_params.bin", format = "bin", base = "0x08040000" },
    { u32 = 42, addr = "0x08060000" },
]

[device.ecu2]
lib = "./zig-out/lib/libecu2.dylib"

# -- Environments -----------------------------------------------------------

[env.bench]
# Current code uses `sessions`; docs use "instance" in prose.
sessions = [
    { name = "ecu1-a", device = "ecu1" },
    { name = "ecu2-a", device = "ecu2" },
]

[env.bench.can.chassis]
members = ["ecu1-a", "ecu2-a"]
vcan = "vcan0"
dbc = "./chassis.dbc"
```

### Inline Values

```toml
{ u32 = 42, addr = "0x08060000" }
{ i32 = -1, addr = "0x08060004" }
{ f32 = 3.14, addr = "0x08060008" }
{ bool = true, addr = "0x0806000C" }
```

V1 serializes inline values as little-endian bytes.

### Backwards Compatibility

- Existing env entries with direct `lib = "..."` should keep working.
- `device` and `lib` on the same entry are mutually exclusive.
- `sessions` remains the concrete parser field unless the terminology work adds an `instances` alias in the same stream.

Still-valid direct form:

```toml
[env.simple]
sessions = [
    { name = "default", lib = "./my_lib.dylib" },
]
```

### Rust Config Shapes

Target delta from the current config model:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceDef {
    pub lib: String,
    #[serde(default)]
    pub flash: Vec<FlashBlockDef>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum FlashBlockDef {
    File {
        file: String,
        format: Option<String>,
        base: Option<String>,
    },
    InlineU32 { u32: u32, addr: String },
    InlineI32 { i32: i32, addr: String },
    InlineF32 { f32: f64, addr: String },
    InlineBool { bool: bool, addr: String },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvSession {
    pub name: String,
    pub lib: Option<String>,
    pub device: Option<String>,
}
```

`FileConfig` also gains:

```rust
#[serde(default)]
pub device: BTreeMap<String, DeviceDef>,
```

### Path Resolution

Flash file paths should follow the same behavior `env start` already uses for DBC/shared-library paths:

- relative to the config file directory when a config file path is known
- otherwise relative to the current working directory
- canonicalized to an absolute path before daemon startup

### Env Integration

Current `runtime/src/cli/env.rs` does CLI-side fan-out. The env-daemon PRD changes who owns orchestration, but it should not change the flashing boundary:

1. resolve the instance entry
2. resolve `device.<name>` if present
3. resolve/canonicalize flash sources
4. parse inline values and files into `ResolvedFlashRegion`s
5. build a `LoadSpec`
6. hand that load spec to the current bootstrap path or the future env daemon
7. let per-device load perform `sim_flash_write(...)` before `sim_init()`
8. continue with CAN/shared/env wiring

Key point: flashing belongs inside device load. It should not become an env-owned long-lived control-plane subsystem.

### Deliverables

- [ ] `[device.<name>]` config section with `lib` and `flash`
- [ ] `device` map added to config model
- [ ] `FlashBlockDef` enum for file and inline variants
- [ ] Inline value serialization to little-endian bytes
- [ ] `EnvSession` updated to allow `device` or `lib`
- [ ] Validation for `device`/`lib` mutual exclusivity and missing device references
- [ ] Flash file paths resolved relative to config source path
- [ ] Env startup builds a load spec instead of sending post-load flash RPCs

---

## Phase 4: Standalone Flash Support

### Problem

Single-instance workflows should also be able to preload flash without defining an env.

### Design

Extend `[defaults.load]` and the `load` CLI so they both produce the same richer load spec used by env startup.

Config:

```toml
[defaults.load]
lib = "./my_lib.dylib"
flash = [
    { file = "./cal.hex" },
]
```

CLI:

```text
agent-sim load ./my_lib.dylib --flash ./cal.hex --flash ./tables.bin:0x08040000
```

The repeatable `--flash` flag accepts:

- `path` for HEX/S-record files
- `path:base_addr` for raw binary

### Deliverables

- [ ] `flash` field added to `[defaults.load]`
- [ ] `load` CLI extended with repeatable `--flash`
- [ ] CLI/defaults resolved into the same load-spec plumbing used by env/device startup
- [ ] Flash parsed and written before `sim_init()` in standalone mode

---

## Open Questions

1. **Template init/reset preservation**: should the template teach a split between non-volatile and volatile state, or should `root.zig` preserve flash fields automatically around `init()`/`reset()`? Recommendation: make the template pattern explicit and safe by default, rather than relying on every adapter author to remember it.

2. **Reset semantics**: in real hardware, reset preserves flash. That matches the env-daemon PRD's device-local reset framing. Recommendation: `sim_reset()` should not cause the runtime to re-flash; adapters/templates should preserve flash and only reset volatile state.

3. **Address width**: `uint32_t base_addr` caps V1 at a 4 GB address space. Recommendation: keep it for V1.

4. **Inline endianness**: V1 assumes little-endian serialization. Recommendation: keep fixed little-endian behavior in V1.

5. **`sessions` vs `instances` config spelling**: should config aliasing land here or in separate terminology work? Recommendation: do not block flash/device work on aliasing; keep `sessions` initially unless aliasing is low-risk.

---

## Phase Ordering

```text
Phase 1: Flash ABI extension
    |
    v
Phase 2: Parser + resolved region model
    |
    v
Phase 3: Load/bootstrap plumbing + device config
    |
    v
Phase 4: Standalone load UX
```

Notes:

- Phases 1 and 2 are mostly independent.
- Phase 3 is the key repo-specific step because of the current daemon bootstrap design and the config/model changes that hang off it.
- Phase 4 should stay compatible with the env-daemon work by treating flash as part of per-device load, not env-owned transport orchestration.
