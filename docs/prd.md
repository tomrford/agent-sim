# PRD: Bus Infrastructure, CAN Virtualisation & Multi-Device Simulation

## Context

agent-sim v1 delivers a working SIL framework: single-device simulation driven by CLI or agent over a Unix socket, with signal I/O, time control, and TOML recipes. This PRD covers the next phase — inter-device communication, CAN bus virtualisation, and the runtime changes needed to support them.

The target use case is multi-ECU cluster simulation where devices share state and exchange CAN frames at tick-level granularity (down to 20us quanta), with CAN buses backed by Linux SocketCAN VCAN interfaces.

### Reference Topology

The primary development target is a 4-device cluster with two CAN buses:

```
  ┌──────┐   ┌──────┐   ┌──────┐   ┌──────┐
  │ ecu1 │   │ ecu2 │   │ ecu3 │   │ ecu4 │
  └──┬─┬─┘   └──┬─┬─┘   └──┬───┘   └──┬───┘
     │ │        │ │         │          │
     │ └────────┘ └─────────┘──────────┘  vcan_internal (inter-device)
     │
     └────────────────────────────────►   vcan_external (agent / human / tools)
```

Each device runs as an independent daemon process with a 20us tick quantum. The internal bus carries inter-device CAN frames. The external bus is for agent injection, monitoring, or external tool integration (SavvyCAN, candump, python-can, etc.).

---

## Decisions

Resolved design questions captured here for reference:

- **CAN FD**: supported from day one. `SimCanFrame.data` is 64 bytes. Each bus descriptor declares FD capability. Classic-only buses reject oversized frames at the runtime level.
- **CAN transport**: Linux SocketCAN VCAN interfaces. Each CAN bus in the simulation maps to a `vcan` interface. Daemons open `AF_CAN` sockets directly — the kernel handles broadcast, ordering, and buffering. No custom ring buffer infrastructure needed for CAN. External tools connect to the same VCAN interface natively. **macOS/Windows**: CAN functionality is Linux-only for v1. Documented as a platform limitation; non-CAN simulation works everywhere.
- **Orchestrator model**: env is a stateless init recipe. `env start` expands the env config into a sequence of `load`, `can attach`, `can load-dbc` commands, executes them, and exits. No manifest, no env tracking, no supervisor. Daemons self-manage. Teardown is `close --all` or `close` per session. VCAN interfaces are user-managed (pre-created via `ip link` or setup script), not managed by agent-sim.
- **Multi-session tick synchronisation**: always loose coupling. Each daemon free-runs at its own pace. CAN frames are delivered asynchronously via the kernel, matching real hardware where ECUs have independent clocks and CAN arbitration handles the rest.
- **CLI signal interface**: unchanged. Direct FFI `sim_read_val`/`sim_write_val` remains the debug probe into a single device. CAN signals become readable via `get` only when a .dbc is loaded, as a decoded view layered on bus traffic.
- **Recipe/Watch execution**: CLI-side only. The daemon handles only fast, atomic actions (signal read/write, time step, etc.). Recipes are decomposed by the CLI into individual actions sent sequentially. Watch is a CLI polling loop. The daemon never blocks on long-running operations.
- **Signal targeting (multi-device)**: three-layer resolution for which session receives a CLI action: per-step `session` override (in recipe steps) > recipe-level `session` default > `--session` CLI flag > `"default"`. Single-device workflows are unaffected — everything defaults to `"default"` as today.
- **Env tag**: lightweight metadata on each daemon — `env: Option<String>` in `DaemonState`, set via hidden `--env-tag <name>` arg at spawn. Used for group operations (`close --env cluster`) and recipe precondition validation. Not used for routing or targeting — that's done by session name.
- **Loopback**: no loopback by default. A device does not receive its own TX frames via `sim_can_rx`. This matches the common firmware pattern where TX confirmation is handled separately from RX. (VCAN loopback can be configured at the kernel level if needed.)
- **Signal namespace**: the `can.` prefix is reserved for CAN signal overlays (DBC-decoded signals). DLL signals must not use the `can.` prefix; this is validated at load time.

---

## Phase 1: Daemon Loop Refactor & CLI-Side Recipe/Watch

### Problem

The current daemon loop (`server.rs:129-142`) interleaves tick pacing with client connection acceptance in a single `loop`:

```rust
loop {
    tick_realtime(&project);
    timeout(20ms, listener.accept()).await;
}
```

This causes:
1. **Tick burstiness** — with a 20us quantum, ~1000 ticks fire in a burst every 20ms rather than pacing smoothly. Acceptable for pure simulation, but problematic when CAN frames need timely delivery to VCAN.
2. **Connection handling blocks ticking** — while `handle_connection` processes a request (which can include recipe execution, watch sampling, and time steps), no realtime ticks fire.

Additionally, `Watch` (with async `sleep()`) and `RunRecipe` (recursive dispatch) are long-running actions that block the daemon loop even after a refactor if they remain daemon-side.

### Solution

Two changes:

#### 1. Split daemon into connection task + tick task

```
                     ┌─────────────────────┐
  CLI ──► socket ──► │  Connection Task     │
  CLI ──► socket ──► │  (accept + decode)   │
                     │                      │
                     │  Action ──► mpsc ──► │
                     └──────────┬───────────┘
                                │
                                ▼
                     ┌─────────────────────┐
                     │  Tick Task           │
                     │  (owns DaemonState)  │
                     │                      │
                     │  drain channel       │
                     │  CAN RX from socket  │
                     │  sim_tick()          │
                     │  CAN TX to socket    │
                     │  sleep to next tick  │
                     └─────────────────────┘
```

**Connection task** — `tokio::spawn`'d, runs `listener.accept()` in a normal async loop. Decodes JSON requests, sends `(Action, oneshot::Sender<Response>)` through an MPSC channel to the tick task.

**Tick task** — owns `DaemonState` exclusively (no Arc/Mutex). Each iteration:
1. Drain pending actions from the channel, dispatch each, send response via oneshot.
2. If time is running, calculate ticks due, call `sim_tick()` in a loop.
3. (Phase 2) CAN RX/TX around each tick.

Every action the tick task handles is fast and non-blocking: signal read, signal write, time step, reset, info, etc. No action sleeps or loops.

**Tick pacing**: batch-catch-up for realtime mode (current behaviour — deterministic and fast). Ticks fire in the correct count, just in bursts. Per-tick pacing is a future enhancement if VCAN frame timing matters.

#### 2. Move Watch and RunRecipe to CLI

**Recipes**: `RunRecipe` action is removed from the daemon protocol entirely. The CLI parses the recipe TOML, translates each step into individual `Set`, `TimeStep`, `Get` actions, and sends them one-at-a-time (waiting for each response before sending the next). The daemon doesn't know what a recipe is.

This naturally extends to multi-session recipes: the CLI targets different session sockets per step. No daemon-side changes needed.

**Watch**: `Watch` action is removed from the daemon protocol. The CLI runs a polling loop: send `Get`, print result, sleep, repeat. The daemon sees normal `Get` requests.

**Ordering guarantee**: a single CLI process sends actions sequentially (send → recv → send → recv). Ordering is guaranteed by the protocol. Two separate CLI processes sending concurrently would interleave, which is expected and correct.

### Deliverables

- [ ] Refactor `server.rs` — connection task + tick task with MPSC channel
- [ ] `DaemonState` accessed only from tick task (no synchronisation needed)
- [ ] Actions dispatched via `(Action, oneshot::Sender<Response>)` channel messages
- [ ] Remove `Watch` and `RunRecipe` actions from daemon protocol
- [ ] Move recipe execution to CLI: parse TOML, send individual actions per step
- [ ] Move watch to CLI: polling loop with `Get` actions
- [ ] Remove `async_recursion` dependency from server.rs
- [ ] Existing CLI behaviour unchanged for end users — all integration tests pass
- [ ] Concurrent connections handled without blocking tick loop

---

## Phase 2: CAN Bus (VCAN Transport)

### Problem

CAN is the primary inter-device communication bus in automotive/embedded. Firmware that talks CAN today has no way to exercise that code path in agent-sim.

### Architecture

Each CAN bus in the simulation maps to a Linux VCAN interface. Daemons open `AF_CAN` sockets on the VCAN interfaces they participate in. The kernel handles broadcast, frame ordering, and buffering.

```
  daemon(ecu1)                    daemon(ecu2)
  ┌──────────────┐               ┌──────────────┐
  │ AF_CAN sock  │──► vcan0 ◄───│ AF_CAN sock  │
  │   (RX/TX)    │               │   (RX/TX)    │
  └──────────────┘               └──────────────┘
                        │
                  ┌─────┴──────┐
                  │ candump    │  (or SavvyCAN, python-can, etc.)
                  │ cansend    │
                  └────────────┘
```

For the reference topology:
- `vcan_internal`: ecu1, ecu2, ecu3, ecu4 all open sockets on this interface
- `vcan_external`: ecu1 opens a socket; external tools connect directly

Two buses = two independent kernel interfaces. No contention, no cross-talk.

**Performance**: at typical CAN message rates (1000-4000 frames/sec across a 4-device bus), VCAN syscall overhead is negligible. Each `sendto()`/`recvfrom()` is ~1-5us. The tick quantum (20us) drives 50,000 ticks/sec, but CAN TX is periodic (every N ticks), not every tick.

**Loopback**: Linux VCAN has kernel-level loopback (sender sees its own frames). We disable this per-socket via `CAN_RAW_RECV_OWN_MSGS = 0` so that `sim_can_rx` only receives frames from other participants. A device's own TX frames are already reflected in its internal state.

### CAN ABI Extension

New optional DLL exports (discovered via `library.get()`, non-fatal if absent):

```c
/**
 * @brief CAN frame structure.
 *
 * Supports both classic CAN (8 bytes) and CAN FD (up to 64 bytes).
 *
 * Flag bits:
 *   bit 0: extended frame (29-bit arbitration ID)
 *   bit 1: FD frame (data may exceed 8 bytes)
 *   bit 2: BRS (bit rate switch, FD only)
 *   bit 3: ESI (error state indicator, FD only)
 *   bit 4: RTR (remote transmission request, classic only)
 *   bits 5-7: reserved (must be 0)
 */
typedef struct {
    uint32_t arb_id;      // 11-bit or 29-bit arbitration ID
    uint8_t  len;         // payload length in bytes (0-8 classic, 0-64 FD)
    uint8_t  flags;       // see flag bits above
    uint8_t  _pad[2];
    uint8_t  data[64];    // CAN FD max; classic CAN uses first 8
} SimCanFrame;

/**
 * @brief CAN bus descriptor returned by sim_can_get_buses().
 *
 * Each DLL declares the CAN buses it participates in.
 * The runtime uses the bus name to wire sessions to VCAN interfaces
 * (matching DLL bus names to env config bus names).
 *
 * Pointer lifetime: name must point to static storage (valid for
 * the lifetime of the loaded DLL). Must be null-terminated UTF-8.
 *
 * FD capability: bitrate_data > 0 and flags bit 0 set.
 * Classic-only: bitrate_data = 0, flags = 0.
 */
typedef struct {
    uint32_t    id;             // bus index local to this DLL (0, 1, ...)
    const char *name;           // e.g. "internal", "external"
    uint32_t    bitrate;        // nominal bitrate in bps (informational)
    uint32_t    bitrate_data;   // FD data-phase bitrate (0 = classic only)
    uint8_t     flags;          // bit 0: FD capable
    uint8_t     _pad[3];
} SimCanBusDesc;

/** Enumerate CAN buses this DLL participates in. */
SimStatus sim_can_get_buses(SimCanBusDesc *out, uint32_t capacity,
                            uint32_t *out_written);

/** Deliver received frames to the DLL (called before sim_tick). */
SimStatus sim_can_rx(uint32_t bus_id, const SimCanFrame *frames,
                     uint32_t count);

/**
 * @brief Collect frames the DLL wants to transmit (called after sim_tick).
 *
 * DLL fills the output buffer with queued TX frames and writes
 * the count to out_written. Returns SIM_ERR_BUFFER_TOO_SMALL if
 * capacity was insufficient (partial fill; host should call again).
 */
SimStatus sim_can_tx(uint32_t bus_id, SimCanFrame *out,
                     uint32_t capacity, uint32_t *out_written);
```

**Key design notes**:
- `len` is payload bytes, not DLC code. For CAN FD, valid lengths are 0-8, 12, 16, 20, 24, 32, 48, 64. The runtime validates this.
- Flag bits reserve space for BRS, ESI, RTR even though v1 may not use them all. This avoids ABI breaks later.
- If any CAN symbol is present, all three must be present (validated at load time). If none are present, the DLL simply doesn't participate in CAN — no error, no special handling for existing DLLs.

### Capability Detection

In `Project::load`:

```rust
// Non-fatal: DLLs without CAN simply have None here
let can_get_buses: Option<SimCanGetBusesFn> =
    unsafe { library.get(b"sim_can_get_buses\0") }.ok().map(|s| *s);
let can_rx: Option<SimCanRxFn> =
    unsafe { library.get(b"sim_can_rx\0") }.ok().map(|s| *s);
let can_tx: Option<SimCanTxFn> =
    unsafe { library.get(b"sim_can_tx\0") }.ok().map(|s| *s);
```

### Bus Discovery

At daemon startup (after `sim_init`), if CAN exports are detected, call `sim_can_get_buses` to enumerate the DLL's buses:

```rust
pub struct CanBusMeta {
    pub id: u32,
    pub name: String,
    pub bitrate: u32,
    pub bitrate_data: u32,
    pub fd_capable: bool,
}
```

This metadata is reported via `can buses` CLI and used by `env start` to validate bus wiring — ensuring bus names in the env config match names declared by each DLL.

### Tick-Loop Integration

In the tick task, for each CAN bus this daemon is attached to:

```
1. Non-blocking recvfrom() on AF_CAN socket → collect pending frames
2. sim_can_rx(bus_id, frames.as_ptr(), frames.len())
3. sim_tick()
4. sim_can_tx(bus_id, out_buf, capacity, &mut written)
5. sendto() each TX frame on AF_CAN socket
```

CAN socket I/O is non-blocking. If no frames are pending, `recvfrom` returns immediately. The tick loop is never blocked by CAN I/O.

**FD validation**: if a bus is declared classic-only (by the bus descriptor), the runtime checks outbound frames and rejects any with `len > 8` or the FD flag set.

### CAN Bus Attachment

A new `CanAttach` action tells a running daemon to open an `AF_CAN` socket on a VCAN interface and bind it to one of its declared buses:

```
Protocol action: CanAttach { bus_name: String, vcan_iface: String }
```

The daemon:
1. Resolves `bus_name` against its DLL's declared CAN buses
2. Opens `AF_CAN` socket on the named VCAN interface
3. Disables loopback (`CAN_RAW_RECV_OWN_MSGS = 0`)
4. If FD capable, enables `CAN_RAW_FD_FRAMES`
5. Registers the socket in daemon state for tick-loop I/O

`CanDetach { bus_name }` closes the socket and removes it from the tick loop.

For development/testing, these are issued manually. Phase 4's `env start` automates the sequence.

### CLI Commands

```
agent-sim can buses                              # list CAN buses exposed by loaded DLL
agent-sim can send <bus> <arb_id> <data_hex>     # inject a frame onto the VCAN interface
agent-sim can attach <bus> <vcan_iface>          # attach bus to VCAN interface
agent-sim can detach <bus>                       # detach bus from VCAN interface
```

`can send` writes a frame to the VCAN interface directly (via a temporary `AF_CAN` socket or via the daemon's socket). All participants on the bus — including simulated devices and external tools — see the frame. This is the correct behaviour: injecting a frame onto a shared bus.

`can monitor` (live frame dump) is deferred — users can use `candump vcan0` directly, which is better than anything we'd build.

### Zig Template

A CAN-enabled adapter template alongside the existing signal-only template:

```zig
const sim_types = @import("sim_types.zig");

pub const SimCanFrame = sim_types.SimCanFrame;
pub const SimCanBusDesc = sim_types.SimCanBusDesc;

pub const can_buses = [_]SimCanBusDesc{
    .{ .id = 0, .name = "internal", .bitrate = 500_000,
       .bitrate_data = 0, .flags = 0, ._pad = .{0, 0, 0} },
    .{ .id = 1, .name = "external", .bitrate = 500_000,
       .bitrate_data = 2_000_000, .flags = 0x01, ._pad = .{0, 0, 0} },
};

const TX_QUEUE_SIZE = 16;

pub const Ctx = struct {
    // ... existing signal state ...

    // CAN TX queues (per bus)
    internal_tx: [TX_QUEUE_SIZE]SimCanFrame = undefined,
    internal_tx_count: u32 = 0,
    external_tx: [TX_QUEUE_SIZE]SimCanFrame = undefined,
    external_tx_count: u32 = 0,
};

pub fn canRx(ctx: *Ctx, bus_id: u32, frames: [*]const SimCanFrame, count: u32) void {
    var i: u32 = 0;
    while (i < count) : (i += 1) {
        const frame = frames[i];
        switch (bus_id) {
            0 => handleInternalFrame(ctx, frame),
            1 => handleExternalFrame(ctx, frame),
            else => {},
        }
    }
}

pub fn canTx(ctx: *Ctx, bus_id: u32, out: [*]SimCanFrame, capacity: u32, out_written: *u32) sim_types.SimStatus {
    switch (bus_id) {
        0 => {
            const n = @min(capacity, ctx.internal_tx_count);
            var i: u32 = 0;
            while (i < n) : (i += 1) out[i] = ctx.internal_tx[i];
            out_written.* = n;
            const had = ctx.internal_tx_count;
            ctx.internal_tx_count = 0;
            return if (capacity < had) .BUFFER_TOO_SMALL else .OK;
        },
        1 => {
            const n = @min(capacity, ctx.external_tx_count);
            var i: u32 = 0;
            while (i < n) : (i += 1) out[i] = ctx.external_tx[i];
            out_written.* = n;
            const had = ctx.external_tx_count;
            ctx.external_tx_count = 0;
            return if (capacity < had) .BUFFER_TOO_SMALL else .OK;
        },
        else => { out_written.* = 0; return .OK; },
    }
}

fn handleInternalFrame(ctx: *Ctx, frame: SimCanFrame) void {
    // Dispatch by arb_id to internal state updates
    _ = ctx;
    _ = frame;
}

fn handleExternalFrame(ctx: *Ctx, frame: SimCanFrame) void {
    _ = ctx;
    _ = frame;
}
```

### Platform Limitation

CAN functionality requires Linux with SocketCAN (`AF_CAN`). On macOS and Windows, CAN-related features are unavailable:
- `can attach` returns an error explaining the platform limitation
- `can buses` still works (reads DLL metadata, no socket needed)
- `can send` unavailable
- DLLs with CAN exports load and run normally; CAN RX/TX is simply skipped in the tick loop

This is acceptable for v1. Non-CAN simulation (signals, time control, recipes) works on all platforms.

### Deliverables

- [ ] `SimCanFrame`, `SimCanBusDesc` types in `sim_api.h` and Rust/Zig equivalents
- [ ] Optional CAN symbol binding in `Project::load` (all-or-nothing validation)
- [ ] Bus discovery at daemon startup (`sim_can_get_buses`)
- [ ] `CanAttach` / `CanDetach` protocol actions
- [ ] `AF_CAN` socket integration in tick loop (non-blocking RX before tick, TX after tick)
- [ ] Loopback disabled per socket (`CAN_RAW_RECV_OWN_MSGS = 0`)
- [ ] FD frame validation on classic-only buses
- [ ] `can buses`, `can send`, `can attach`, `can detach` CLI commands
- [ ] CAN-enabled Zig adapter template with TX queue pattern
- [ ] Integration test: two sessions exchanging CAN frames over VCAN (Linux CI)

---

## Phase 3: DBC Signal Overlay

### Problem

Raw CAN frames are opaque byte arrays. Engineers work with named signals (engine_rpm, throttle_position) defined in .dbc files. agent-sim should let users interact with CAN signals by name via the existing `get`/`set` CLI.

### Design

**DBC parsing**: Use the `can-dbc` crate (or similar) to parse .dbc files into an in-memory signal database.

**Supported DBC subset for v1**:
- Scalar signals (bool, integer, float) with byte order (big-endian / little-endian)
- Signedness (signed / unsigned)
- Scale + offset (physical = raw * factor + offset)
- Min/max value ranges
- Unit strings
- Standard (non-multiplexed) messages only — multiplexed messages are out of scope for v1

**Signal projection**: CAN signals are projected into the CLI signal namespace with a `can.<bus>.` prefix:

```
can.internal.engine_rpm          # from message 0x0CF004
can.internal.throttle_position   # from message 0x0CF004
```

The `can.` prefix is reserved and validated: DLL signals starting with `can.` are rejected at load time to prevent namespace collisions.

These are decoded views from CAN frames captured by the runtime. The daemon maintains a **frame state table**: the latest frame seen per arbitration ID per bus (populated from both RX and TX traffic on the VCAN socket).

**Read**: `get can.internal.engine_rpm` decodes the value from the latest frame for that message's arbitration ID using the DBC signal definition (start bit, length, byte order, scale, offset).

**Write**: `set can.internal.throttle_position=50` encodes the value into the appropriate bits of the latest frame for that message ID (read-modify-write) and sends the frame to the VCAN interface. If no frame has been seen for that message ID, the daemon synthesises a frame with all-zeros data before applying the write — this is deterministic and matches the common pattern of default DBC values being 0.

### CLI

```
agent-sim can load-dbc <bus> <path>              # parse .dbc, register signal overlay
agent-sim get can.internal.engine_rpm            # decoded from latest frame
agent-sim get can.internal.*                     # all decoded CAN signals on bus
agent-sim set can.internal.throttle_position=50  # encode + send to VCAN
```

### Deliverables

- [ ] DBC file parser integration (`can-dbc` crate or similar)
- [ ] Signal database stored per bus in daemon state
- [ ] Frame state table (latest frame per arb_id per bus)
- [ ] `can load-dbc` CLI command and `CanLoadDbc` protocol action
- [ ] CAN signal read via `get can.<bus>.<signal_name>`
- [ ] CAN signal write via `set can.<bus>.<signal_name>=<value>` (encode + inject)
- [ ] Glob support for CAN signals (`get can.internal.*`)
- [ ] `can.` prefix reservation enforced at DLL signal load time

---

## Phase 4: Environment Configuration & Multi-Session Recipes

### Problem

Multi-device simulation requires coordinating multiple sessions and CAN bus wiring. Manually issuing `load`, `can attach`, `can load-dbc` for each session is tedious and error-prone.

### Design Principle

An env is a **stateless init recipe** — a declarative description of a simulation topology that expands into a sequence of CLI commands. There is no persistent env state, no manifest, no supervisor process. After `env start` exits, the running sessions are independent daemons discoverable via `session list` as today. Teardown is `close --all` or `close` per session.

VCAN interfaces are **user-managed** infrastructure, not managed by agent-sim. The user (or a CI setup script) creates them once with `ip link add ... type vcan`. `env start` references them by name but does not create or delete them. This avoids permissions issues, ownership tracking, and cleanup complexity.

### Environment Config

A new `[env.<name>]` section in `agent-sim.toml` declares the topology:

```toml
[env.cluster]
sessions = [
    { name = "ecu1", lib = "./zig-out/lib/libecu1.dylib" },
    { name = "ecu2", lib = "./zig-out/lib/libecu2.dylib" },
    { name = "ecu3", lib = "./zig-out/lib/libecu3.dylib" },
    { name = "ecu4", lib = "./zig-out/lib/libecu4.dylib" },
]

# Internal bus: all 4 ECUs, classic CAN
[env.cluster.can.internal]
members = ["ecu1:internal", "ecu2:internal", "ecu3:internal", "ecu4:internal"]
vcan = "vcan_internal"

# External bus: only ecu1, FD capable
[env.cluster.can.external]
members = ["ecu1:external"]
vcan = "vcan_external"
dbc = "./dbc/external.dbc"
```

The `member:bus_name` syntax maps a session to the DLL's declared bus name. If a DLL only declares one CAN bus, the bus name portion can be omitted (e.g. just `"ecu1"`).

### `env start` Expansion

`env start cluster` expands to and executes this sequence:

```
# 1. Spawn sessions (tagged with env name)
agent-sim load ./libecu1.so --session ecu1 --env-tag cluster
agent-sim load ./libecu2.so --session ecu2 --env-tag cluster
agent-sim load ./libecu3.so --session ecu3 --env-tag cluster
agent-sim load ./libecu4.so --session ecu4 --env-tag cluster

# 2. Attach CAN buses to VCAN interfaces
agent-sim can attach internal vcan_internal --session ecu1
agent-sim can attach internal vcan_internal --session ecu2
agent-sim can attach internal vcan_internal --session ecu3
agent-sim can attach internal vcan_internal --session ecu4
agent-sim can attach external vcan_external --session ecu1

# 3. Load DBC files
agent-sim can load-dbc external ./dbc/external.dbc --session ecu1
```

**Validation before execution**:
- Check that referenced VCAN interfaces exist and are up (fail fast with clear error if not)
- Check that session names don't collide with already-running sessions

**Rollback on failure**: if any step fails mid-sequence, `env start` sends `Close` to every session it successfully started in this run, then exits with an error. The user is left with a clean state.

**Collision handling**: running `env start cluster` when ecu1 is already running fails at the first `load` with `AlreadyRunning`. This is correct — you can't have two sessions with the same name. To run multiple instances of the same topology, define separate envs with different session names.

### Multi-Session Recipes

Recipes can optionally declare which sessions they expect, and which env they belong to:

```toml
[recipe.integration-test]
env = "cluster"                     # precondition: all "cluster"-tagged sessions must be running
sessions = ["ecu1", "ecu2"]        # precondition: these specific sessions must be running
session = "ecu1"                   # default session for steps that don't specify one
steps = [
  { set = { writes = { "ignition" = "true" } } },               # → targets "ecu1" (recipe default)
  { step = { duration = "100ms" } },                             # → targets "ecu1" (recipe default)
  { step = { session = "ecu2", duration = "100ms" } },           # → targets "ecu2" (step override)
  { assert = { session = "ecu2", signal = "can.internal.rpm", gt = 800.0 } },
]
```

- `env` — optional. If set, the CLI queries running sessions, checks that all sessions tagged with this env name are alive. Fails fast if any are missing.
- `sessions` — optional. The CLI checks that these specific session names are running. More granular than `env`.
- `session` — optional recipe-level default. Steps without an explicit `session` field use this. Falls back to `--session` CLI flag, then `"default"`.

Recipes without any of these fields work as today (single session, implicit `--session` flag targeting).

**Signal targeting resolution** (per step):

1. Step-level `session = "ecu1"` — explicit, highest priority
2. Recipe-level `session` default (top-level field in recipe) — applies to all steps that don't override
3. `--session` CLI flag — applies when running the recipe from the command line
4. `"default"` — the implicit session name when nothing is specified

This ensures single-device recipes work unchanged (everything resolves to `"default"` or the `--session` flag), while multi-device recipes can target specific sessions per step.

Since recipes run CLI-side (Phase 1), multi-session support is natural: the CLI simply connects to different session sockets per step. No daemon-side changes needed.

### Teardown

No special env teardown command. Use existing session management plus env-tag filtering:

```
agent-sim close --session ecu1     # close one session
agent-sim close --env cluster      # close all sessions tagged with env "cluster"
agent-sim close --all              # close every running session
```

`close --env cluster` queries `session list`, filters by env tag, sends `Close` to each matching session. `close --all` sends `Close` to every running session regardless of tag. If a session doesn't respond within 5s, kill by PID (from `~/.agent-sim/<session>.pid`).

The env tag is exposed in `session list` output so users can see which sessions belong to which env.

### Deliverables

- [ ] `[env]` config parser with bus wiring validation
- [ ] `env start <name>` — validate VCAN interfaces, spawn sessions (with `--env-tag`), attach buses, load DBCs
- [ ] Rollback on partial failure (close successfully-started sessions)
- [ ] `--env-tag` hidden CLI arg, stored as `env: Option<String>` in `DaemonState`
- [ ] Env tag exposed in `session list` output and queryable via protocol
- [ ] `close --env <name>` — close all sessions matching the env tag
- [ ] `close --all` — close every running session (graceful + kill fallback)
- [ ] Recipe `env` field with precondition validation (all tagged sessions alive)
- [ ] Recipe `sessions` field with validation
- [ ] Recipe-level `session` default with per-step override
- [ ] Multi-session recipe step targeting (`session = "ecu1"`)

---

## Phase 5: Shared-State IPC

### Problem

Some multi-device scenarios need to share typed values every tick without CAN framing overhead (e.g. 3-4 FP32 sensor readings between co-located ECUs, or a shared physical model).

Note: for many cases, this can be achieved by packing values into CAN FD frames (4 x FP32 = 16 bytes, fits in one FD frame). This phase is only needed when CAN framing is genuinely inappropriate.

### Design

A shared snapshot region in mapped memory: a flat struct that one session writes and others read. Not a ring buffer — shared state wants latest-value semantics, not queued history.

```
┌───────────────────────────────────────┐
│  Shared Region (mmap'd, per channel)  │
│                                       │
│  Header:                              │
│    generation: AtomicU64              │
│    slot_count: u32                    │
│    writer_session: [u8; 64]           │
│                                       │
│  Slots:                               │
│    [SimSharedSlot; slot_count]        │
│                                       │
└───────────────────────────────────────┘
```

Single-writer, multiple-reader. Writer updates slots and bumps generation atomically. Readers check generation before/after read to detect torn writes. Simple, no ring complexity.

**ABI extension** (optional exports):

```c
typedef struct {
    uint32_t id;
    const char *name;      // e.g. "sensor_feed"
    uint32_t slot_count;   // number of values in this region
} SimSharedDesc;

typedef struct {
    uint32_t slot_id;
    SimType  type;
    SimValue value;
} SimSharedSlot;

SimStatus sim_shared_get_channels(SimSharedDesc *out, uint32_t capacity,
                                  uint32_t *out_written);

/** Write outbound shared state (called after sim_tick). */
SimStatus sim_shared_write(uint32_t channel_id, SimSharedSlot *out,
                           uint32_t capacity, uint32_t *out_written);

/** Read inbound shared state (called before sim_tick). */
SimStatus sim_shared_read(uint32_t channel_id, const SimSharedSlot *slots,
                          uint32_t count);
```

### Deliverables

- [ ] `SimSharedDesc`, `SimSharedSlot` types in `sim_api.h` and Rust/Zig equivalents
- [ ] Optional shared-state symbol binding (all-or-nothing like CAN)
- [ ] Shared snapshot region implementation (`mmap` + generation counter)
- [ ] Tick-loop integration (read before tick, write after tick)
- [ ] Env config: `[env.<name>.shared.<channel>]` with members + writer designation
- [ ] CLI: `shared list`, `shared get <channel>.*`

---

## Phase 6: Recipe Assertions

### Problem

Recipes can set signals and step time, but cannot validate outcomes. They're procedures, not tests.

### Design

New recipe step types:

```toml
# Exact equality
{ assert = { signal = "hvac.state", eq = 2 } }

# Numeric comparison
{ assert = { signal = "hvac.current_temp", gt = 22.0 } }
{ assert = { signal = "hvac.current_temp", lt = 25.0 } }

# Tolerance (for floating point)
{ assert = { signal = "hvac.current_temp", approx = 22.0, tolerance = 0.5 } }

# CAN signal (requires DBC loaded)
{ assert = { signal = "can.internal.engine_rpm", gt = 800.0 } }
```

Since recipes run CLI-side, assertion logic lives in the CLI. On failure: stop recipe, report signal name + expected vs actual + tick/time, exit with non-zero code for CI.

### Deliverables

- [ ] `Assert` recipe step type with eq/gt/lt/gte/lte/approx comparisons
- [ ] Clear failure reporting with signal name, expected, actual, tick count
- [ ] Non-zero exit code on assertion failure
- [ ] Works with DLL signals and CAN signals (once DBC overlay exists)

---

## Phase Ordering & Dependencies

```
Phase 1: Daemon Loop Refactor + CLI-Side Recipe/Watch
    │
    ▼
Phase 2: CAN Bus (VCAN Transport + ABI)
    │
    ▼
Phase 3: DBC Signal Overlay
    │
    ▼
Phase 4: Env Config & Orchestration

Phase 5: Shared-State IPC (independent of CAN, can land after Phase 1)

Phase 6: Recipe Assertions (independent, can land any time after Phase 1)
```

Phase 6 (assertions) is low effort and high value — consider landing it alongside or immediately after Phase 1, since recipes are now CLI-side and assertions are just another step type.

Phase 5 (shared-state IPC) is independent of CAN work. It only needs the daemon loop refactor (Phase 1) and can be developed in parallel with CAN phases.

---

## Open Questions

1. **Reset semantics**: currently `Action::Reset` resets the DLL but not the `TimeEngine`. Should reset also zero the tick counter and pause time? This affects assertion behavior (tick count in failure reports). Recommendation: reset both, document clearly.

2. **CAN FD frame length validation**: should the runtime enforce that FD frame `len` values are one of {0-8, 12, 16, 20, 24, 32, 48, 64}, or allow arbitrary lengths up to 64? Real CAN FD hardware only supports the discrete set. Recommendation: enforce the discrete set.
