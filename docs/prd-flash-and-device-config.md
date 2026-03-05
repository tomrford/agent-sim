# PRD: Flash Memory Initialisation & Device Configuration

## Context

agent-sim models firmware as shared libraries (DLLs) with a signal-based I/O surface. State initialisation today is handled entirely inside the DLL's `sim_init()` — the adapter author zeros a Zig struct and sets sane defaults. There is no mechanism for the host runtime to push pre-existing data (calibration tables, lookup data, configuration blocks) into a DLL before it boots.

In real embedded systems, firmware reads static data from flash memory at fixed addresses. When porting such firmware into a SIL DLL, the adapter author needs a way to declare a byte-addressable memory region and have the runtime populate it from standard file formats (Intel HEX, Motorola S-record, raw binary) before `sim_init()` runs — exactly like flashing an MCU before power-on.

This feature also introduces a **device** abstraction in the TOML config: a named bundle of a DLL path plus its flash configuration. Environments then reference devices by name rather than embedding lib paths directly, creating a clean separation between "what a device is" and "how devices are wired together."

### Non-Goals

- The runtime does not interpret flash contents — it pushes raw bytes. Memory layout and access patterns are the DLL author's responsibility.
- No read-back of flash from the host side (write-only from runtime to DLL).
- No runtime memory-mapped I/O or address-space emulation — this is a data seeding mechanism, not an MCU memory controller.
- No `mint` tool integration in V1 (noted as future exploration for parametric hex generation from `../mint`).

---

## Decisions

- **ABI shape**: single `sim_flash_write(base_addr, data, len)` function, called once per region. The runtime calls it N times for N flash blocks. Simpler than a batch descriptor approach and maps naturally to how hex file records are structured.
- **Optional export pattern**: follows the CAN precedent — if `sim_flash_write` is present, the DLL supports flash. Single symbol; no all-or-nothing group since there's no read-back or enumeration needed from the host.
- **Call ordering**: flash writes happen **before** `sim_init()`. The DLL receives all flash data first, then `init()` runs with flash already populated — matching real hardware where flash is written before the MCU boots.
- **Config model**: `[device.<name>]` sections own lib path + flash blocks. `[env.<name>]` sessions reference devices by name. This is a clean split that doesn't over-abstract — no separate `[mem]` indirection for V1.
- **File formats (V1)**: Intel HEX (`.hex`/`.ihex`), Motorola S-record (`.mot`/`.srec`/`.s19`), raw binary with explicit base address.
- **Inline values (V1)**: typed scalar values at explicit addresses, specified directly in TOML. Useful for injecting a single calibration constant without creating a file.
- **DLL-side responsibility**: the adapter author declares a memory region in `Ctx` (e.g. `flash: [N]u8`) and implements `sim_flash_write` to copy data into it. How that memory is indexed and used during `tick()` is entirely up to the adapter.

---

## Phase 1: Flash ABI Extension

### Problem

DLLs that model firmware reading from fixed flash addresses have no way to receive that data from the host. The adapter author must either hardcode the data in Zig source or leave it zeroed, neither of which supports runtime-configurable simulation scenarios (different calibration sets, A/B testing of parameter tables, etc.).

### ABI

New optional DLL export:

```c
/**
 * @brief Write a block of data to the DLL's flash memory region.
 *
 * Called by the host before sim_init() to populate flash contents.
 * May be called multiple times (once per region/record).
 *
 * The DLL is responsible for managing its own memory layout.
 * Overlapping writes are applied in order (last write wins).
 * Writes to addresses outside the DLL's declared range should
 * return SIM_ERR_INVALID_ARG.
 *
 * @param base_addr  Start address for this write
 * @param data       Pointer to raw bytes
 * @param len        Number of bytes to write
 */
SimStatus sim_flash_write(uint32_t base_addr, const uint8_t *data, uint32_t len);
```

### Capability Detection

In `Project::load`, after symbol binding:

```rust
let sim_flash_write: Option<SimFlashWriteFn> =
    unsafe { library.get(b"sim_flash_write\0") }.ok().map(|s| *s);
```

If the config specifies flash blocks for a device whose DLL does not export `sim_flash_write`, loading fails with a clear error.

If the DLL exports `sim_flash_write` but no flash blocks are configured, nothing happens — the DLL boots with default/zeroed flash, same as today.

### Load Sequence Change

Current:
```
dlopen → bind symbols → read catalog → sim_init() → enumerate CAN/shared
```

New:
```
dlopen → bind symbols → read catalog → [sim_flash_write() × N] → sim_init() → enumerate CAN/shared
```

Flash writes are injected between symbol binding and `sim_init()`. The signal catalog and tick duration are read before flash (they don't depend on flash contents). CAN/shared enumeration stays after init.

### Zig Template

```zig
const sim_types = @import("sim_types.zig");
pub const SimStatus = sim_types.SimStatus;

// Example: 256 KB flash region starting at 0x0800_0000
const FLASH_BASE: u32 = 0x0800_0000;
const FLASH_SIZE: u32 = 256 * 1024;

pub const Ctx = struct {
    // Flash memory region
    flash: [FLASH_SIZE]u8 = [_]u8{0xFF} ** FLASH_SIZE,  // 0xFF = erased flash

    // ... existing signal state ...
};

pub fn flashWrite(ctx: *Ctx, base_addr: u32, data: [*]const u8, len: u32) SimStatus {
    if (base_addr < FLASH_BASE) return .INVALID_ARG;
    const offset = base_addr - FLASH_BASE;
    if (offset + len > FLASH_SIZE) return .INVALID_ARG;
    @memcpy(ctx.flash[offset..][0..len], data[0..len]);
    return .OK;
}
```

In `root.zig`, the export wrapper:

```zig
pub export fn sim_flash_write(base_addr: u32, data: ?[*]const u8, len: u32) SimStatus {
    const d = data orelse return .INVALID_ARG;
    if (len == 0) return .OK;
    return adapter.flashWrite(&g_ctx, base_addr, d, len);
}
```

Note: `sim_flash_write` is called before `sim_init()`, so it operates on `g_ctx` directly (not through `requireInitialized()`). The context is default-initialized at declaration (`var g_ctx: adapter.Ctx = .{}`), so flash writes land on a valid struct. `sim_init()` then runs without re-zeroing flash — the adapter author must ensure their `init()` does not overwrite the flash region.

### Deliverables

- [ ] `sim_flash_write` signature in `sim_api.h`
- [ ] Optional symbol binding in `Project::load`
- [ ] Flash write calls injected before `sim_init()` in load sequence
- [ ] Zig template with `flashWrite` pattern and `root.zig` export
- [ ] `sim_types.zig` updated if needed
- [ ] Error if config has flash blocks but DLL lacks `sim_flash_write`
- [ ] Unit test: flash write → init → read signal derived from flash data

---

## Phase 2: File Format Parsers

### Problem

Flash data comes in standard embedded file formats. The runtime needs to parse these into `(base_addr, data, len)` tuples for `sim_flash_write` calls.

### Supported Formats

**Intel HEX** (`.hex`, `.ihex`)
- Record types 00 (data), 01 (EOF), 02 (extended segment address), 04 (extended linear address)
- Produces a set of `(address, bytes)` regions
- Checksum validation on each record

**Motorola S-record** (`.mot`, `.srec`, `.s19`, `.s28`, `.s37`)
- S0 (header), S1/S2/S3 (data with 16/24/32-bit addresses), S5 (count), S7/S8/S9 (end)
- Produces a set of `(address, bytes)` regions
- Checksum validation on each record

**Raw binary** (`.bin`)
- Flat byte array, requires an explicit `base` address in config
- Single `(base, entire_file)` region

### Parser Design

```rust
pub struct FlashRegion {
    pub base_addr: u32,
    pub data: Vec<u8>,
}

pub fn parse_intel_hex(content: &str) -> Result<Vec<FlashRegion>, FlashParseError>;
pub fn parse_srec(content: &str) -> Result<Vec<FlashRegion>, FlashParseError>;
pub fn parse_raw_binary(bytes: &[u8], base: u32) -> FlashRegion;
```

Format detection: explicit in config (`format = "ihex"` / `"srec"` / `"bin"`). If omitted, inferred from file extension. Ambiguous cases fail with a clear error.

### Deliverables

- [ ] Intel HEX parser with checksum validation
- [ ] Motorola S-record parser with checksum validation
- [ ] Raw binary loader with explicit base address
- [ ] Format detection from config or file extension
- [ ] Unit tests for each format (valid files, corrupt checksums, address overflow)

---

## Phase 3: Device Configuration & TOML Schema

### Problem

The current `[env.<name>]` section embeds lib paths inline in sessions. Adding flash configuration alongside lib paths makes sessions unwieldy. A separate device abstraction provides a clean place to attach per-device configuration.

### Config Schema

```toml
# -- Devices ----------------------------------------------------------------

[device.ecu1]
lib = "./zig-out/lib/libecu1.dylib"
flash = [
    { file = "./calibration.hex", format = "ihex" },
    { file = "./lookup_tables.mot" },                          # format inferred from extension
    { file = "./raw_params.bin", format = "bin", base = "0x08040000" },
    { u32 = 42, addr = "0x08060000" },                         # inline scalar
]

[device.ecu2]
lib = "./zig-out/lib/libecu2.dylib"
# No flash — boots with default state

# -- Environments -----------------------------------------------------------

[env.bench]
sessions = [
    { name = "ecu1", device = "ecu1" },
    { name = "ecu2", device = "ecu2" },
]

[env.bench.can.chassis]
members = ["ecu1", "ecu2"]
vcan = "vcan0"
dbc = "./chassis.dbc"
```

**Inline values** supported in V1:

```toml
# Typed scalars at explicit addresses
{ u32 = 42, addr = "0x08060000" }
{ i32 = -1, addr = "0x08060004" }
{ f32 = 3.14, addr = "0x08060008" }
{ bool = true, addr = "0x0806000C" }     # 1 byte: 0x01
```

Values are serialised to little-endian bytes (matching ARM/most embedded targets). Byte order could become configurable in V2 if needed.

**Backwards compatibility**: `[env]` sessions can still use `lib = "..."` directly for environments that don't need flash. If both `device` and `lib` are present on a session, that's an error.

```toml
# Still works — no device, no flash
[env.simple]
sessions = [
    { name = "default", lib = "./my_dll.dylib" },
]
```

### Rust Config Types

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
        format: Option<String>,   // "ihex", "srec", "bin"
        base: Option<String>,     // required for "bin", hex string e.g. "0x08000000"
    },
    InlineU32 { u32: u32, addr: String },
    InlineI32 { i32: i32, addr: String },
    InlineF32 { f32: f64, addr: String },   // TOML floats are f64; narrowed to f32
    InlineBool { bool: bool, addr: String },
}

#[derive(Debug, Clone, Deserialize)]
pub struct EnvSession {
    pub name: String,
    pub lib: Option<String>,       // direct lib path (existing)
    pub device: Option<String>,    // reference to [device.<name>]
}
```

### `defaults.load` Interaction

The existing `[defaults.load]` section provides a default lib path for single-session `agent-sim load` commands. This is orthogonal to devices — devices are for env sessions. A device does not affect the `load` default.

### Env Start Changes

When `env start` encounters a session with `device = "ecu1"`:

1. Resolve `device.ecu1` from config
2. Spawn daemon with `device.ecu1.lib`
3. For each flash block in `device.ecu1.flash`:
   - Parse the file (or serialise the inline value)
   - Send flash write action(s) to the daemon
4. Send init action (or let the daemon's load sequence handle init after flash)
5. Continue with CAN/shared wiring as before

This means the daemon needs a new protocol action:

```
FlashWrite { base_addr: u32, data: Vec<u8> }
```

Or, flash writes could be pushed during `Project::load()` itself if the flash config is passed to the load action. This avoids a separate protocol round-trip.

### Deliverables

- [ ] `[device.<name>]` config section with `lib` and `flash` fields
- [ ] `FlashBlockDef` enum (file + inline variants)
- [ ] Inline value serialisation (little-endian bytes)
- [ ] `EnvSession` updated to support `device` reference (with `lib` fallback)
- [ ] Validation: `device` and `lib` mutually exclusive on a session
- [ ] `env start` resolves devices, parses flash files, pushes data before init
- [ ] File paths in flash blocks resolved relative to config file location
- [ ] `agent-sim device list` CLI command (optional, nice-to-have)

---

## Phase 4: Standalone Flash Support (Non-Env)

### Problem

Single-session workflows (`agent-sim load ./my_dll.dylib`) should also be able to load flash data without defining a full device + env.

### Design

Extend `[defaults.load]` and the `load` CLI command:

```toml
[defaults.load]
lib = "./my_dll.dylib"
flash = [
    { file = "./cal.hex" },
]
```

Or via CLI flags:

```
agent-sim load ./my_dll.dylib --flash ./cal.hex --flash ./tables.bin:0x08040000
```

The `--flash` flag accepts `path` or `path:base_addr` (base required for raw binary). Format is inferred from extension.

### Deliverables

- [ ] `flash` field in `[defaults.load]` config
- [ ] `--flash` CLI flag on `load` command (repeatable)
- [ ] Flash files parsed and written before `sim_init()` in single-session load path

---

## Open Questions

1. **`sim_init()` interaction**: the current Zig template's `init()` does `ctx.* = .{}` which would zero the flash region. Adapter authors who use flash must write their `init()` to preserve it. Should the template guide this explicitly, or should `root.zig` manage it (e.g. save/restore flash across init)? **Recommendation**: document it clearly in the template guide — `init()` must not overwrite flash. This matches real firmware where flash is non-volatile across resets.

2. **Reset semantics**: should `sim_reset()` re-apply flash data? In real hardware, reset doesn't erase flash. But the current `reset()` does `ctx.* = .{}`. Options: (a) document that adapter authors must preserve flash in reset, (b) have the runtime re-send flash writes before `sim_reset()`, (c) add a separate `sim_flash_erase()` for explicit erase. **Recommendation**: (a) for V1 — document it. The runtime doesn't track flash state after initial load.

3. **Address width**: `uint32_t base_addr` limits to 4 GB address space. Sufficient for all current ARM Cortex-M/R targets. If 64-bit targets are needed later, a V2 ABI bump would be required.

4. **Endianness for inline values**: V1 assumes little-endian (ARM). Should this be configurable per-device? **Recommendation**: no, little-endian only for V1. Add `endian = "big"` option in V2 if needed.

---

## Phase Ordering

```
Phase 1: Flash ABI Extension
    │
    ▼
Phase 2: File Format Parsers (can develop in parallel with Phase 1)
    │
    ▼
Phase 3: Device Config & TOML Schema (depends on Phase 1 + 2)
    │
    ▼
Phase 4: Standalone Flash Support (depends on Phase 2, light lift)
```

Phases 1 and 2 are independent and can be developed in parallel. Phase 3 ties them together with the config system. Phase 4 is a small extension of Phase 3.
