const std = @import("std");
const adapter = @import("adapter.zig");
const sim_types = @import("sim_types.zig");

pub const SimStatus = sim_types.SimStatus;
pub const SimValue = sim_types.SimValue;
pub const SimSignalDesc = sim_types.SimSignalDesc;
pub const SimInitConfig = sim_types.SimInitConfig;
pub const SimCanFrame = sim_types.SimCanFrame;
pub const SimCanBusDesc = sim_types.SimCanBusDesc;
pub const SimSharedDesc = sim_types.SimSharedDesc;
pub const SimSharedSlot = sim_types.SimSharedSlot;

var g_ctx: adapter.Ctx = .{};
var g_initialized = false;

fn requireInitialized() ?*adapter.Ctx {
    if (!g_initialized) return null;
    return &g_ctx;
}

pub export fn sim_init(config: ?*const SimInitConfig) SimStatus {
    g_ctx = .{};
    const status = adapter.init(&g_ctx, config);
    if (status == .OK) g_initialized = true;
    return status;
}

pub export fn sim_reset() SimStatus {
    const ctx = requireInitialized() orelse return .NOT_INITIALIZED;
    adapter.reset(ctx);
    return .OK;
}

pub export fn sim_tick() SimStatus {
    const ctx = requireInitialized() orelse return .NOT_INITIALIZED;
    adapter.tick(ctx);
    return .OK;
}

pub export fn sim_read_val(id: u32, out: ?*SimValue) SimStatus {
    const ctx = requireInitialized() orelse return .NOT_INITIALIZED;
    const out_val = out orelse return .INVALID_ARG;
    return adapter.read(ctx, id, out_val);
}

pub export fn sim_write_val(id: u32, in: ?*const SimValue) SimStatus {
    const ctx = requireInitialized() orelse return .NOT_INITIALIZED;
    const in_val = in orelse return .INVALID_ARG;
    return adapter.write(ctx, id, in_val);
}

pub export fn sim_get_signal_count(out_count: ?*u32) SimStatus {
    const out = out_count orelse return .INVALID_ARG;
    out.* = adapter.signalCount();
    return .OK;
}

pub export fn sim_get_signals(out: ?[*]SimSignalDesc, capacity: u32, out_written: ?*u32) SimStatus {
    const written = out_written orelse return .INVALID_ARG;
    if (capacity > 0 and out == null) return .INVALID_ARG;
    if (capacity == 0) {
        written.* = 0;
        return if (adapter.signalCount() == 0) .OK else .BUFFER_TOO_SMALL;
    }
    return adapter.fillSignals(out.?, capacity, written);
}

pub export fn sim_get_tick_duration_us(out_tick_us: ?*u32) SimStatus {
    const out = out_tick_us orelse return .INVALID_ARG;
    out.* = adapter.TickDurationUs;
    return .OK;
}

pub export fn sim_can_get_buses(out: ?[*]SimCanBusDesc, capacity: u32, out_written: ?*u32) SimStatus {
    const written = out_written orelse return .INVALID_ARG;
    if (capacity > 0 and out == null) return .INVALID_ARG;
    if (capacity == 0) {
        written.* = 0;
        return if (adapter.canBusCount() == 0) .OK else .BUFFER_TOO_SMALL;
    }
    return adapter.fillCanBuses(out.?, capacity, written);
}

pub export fn sim_can_rx(bus_id: u32, frames: ?[*]const SimCanFrame, count: u32) SimStatus {
    const ctx = requireInitialized() orelse return .NOT_INITIALIZED;
    if (count > 0 and frames == null) return .INVALID_ARG;
    if (count == 0) return .OK;
    adapter.canRx(ctx, bus_id, frames.?, count);
    return .OK;
}

pub export fn sim_can_tx(bus_id: u32, out: ?[*]SimCanFrame, capacity: u32, out_written: ?*u32) SimStatus {
    const ctx = requireInitialized() orelse return .NOT_INITIALIZED;
    const written = out_written orelse return .INVALID_ARG;
    if (capacity > 0 and out == null) return .INVALID_ARG;
    if (capacity == 0) {
        written.* = 0;
        return .OK;
    }
    return adapter.canTx(ctx, bus_id, out.?, capacity, written);
}

pub export fn sim_shared_get_channels(out: ?[*]SimSharedDesc, capacity: u32, out_written: ?*u32) SimStatus {
    const written = out_written orelse return .INVALID_ARG;
    if (capacity > 0 and out == null) return .INVALID_ARG;
    if (capacity == 0) {
        written.* = 0;
        return if (adapter.sharedChannelCount() == 0) .OK else .BUFFER_TOO_SMALL;
    }
    return adapter.fillSharedChannels(out.?, capacity, written);
}

pub export fn sim_shared_read(channel_id: u32, slots: ?[*]const SimSharedSlot, count: u32) SimStatus {
    const ctx = requireInitialized() orelse return .NOT_INITIALIZED;
    if (count > 0 and slots == null) return .INVALID_ARG;
    if (count == 0) return .OK;
    return adapter.sharedRead(ctx, channel_id, slots.?, count);
}

pub export fn sim_shared_write(channel_id: u32, out: ?[*]SimSharedSlot, capacity: u32, out_written: ?*u32) SimStatus {
    const ctx = requireInitialized() orelse return .NOT_INITIALIZED;
    const written = out_written orelse return .INVALID_ARG;
    if (capacity > 0 and out == null) return .INVALID_ARG;
    if (capacity == 0) {
        written.* = 0;
        return .OK;
    }
    return adapter.sharedWrite(ctx, channel_id, out.?, capacity, written);
}

test "template sanity" {
    try std.testing.expect(sim_init(null) == .OK);

    const in = SimValue{ .type = .F32, .data = .{ .f32 = 5.0 } };
    try std.testing.expect(sim_write_val(0, &in) == .OK);
    try std.testing.expect(sim_tick() == .OK);

    var out: SimValue = undefined;
    try std.testing.expect(sim_read_val(1, &out) == .OK);
    try std.testing.expectEqual(@as(f32, 10.0), out.data.f32);
}
