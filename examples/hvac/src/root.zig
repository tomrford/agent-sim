const std = @import("std");
const adapter = @import("adapter.zig");
const sim_types = @import("sim_types.zig");

pub const SimStatus = sim_types.SimStatus;
pub const SimValue = sim_types.SimValue;
pub const SimSignalDesc = sim_types.SimSignalDesc;

var g_ctx: adapter.Ctx = .{};
var g_initialized = false;

fn requireInitialized() ?*adapter.Ctx {
    if (!g_initialized) return null;
    return &g_ctx;
}

pub export fn sim_init() SimStatus {
    g_ctx = .{};
    adapter.init(&g_ctx);
    g_initialized = true;
    return .OK;
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

test "hvac heating cycle" {
    try std.testing.expect(sim_init() == .OK);

    var v = SimValue{ .type = .BOOL, .data = .{ .b = true } };
    try std.testing.expect(sim_write_val(0, &v) == .OK);

    v = SimValue{ .type = .F32, .data = .{ .f32 = 25.0 } };
    try std.testing.expect(sim_write_val(1, &v) == .OK);

    try std.testing.expect(sim_tick() == .OK);

    var out: SimValue = undefined;
    try std.testing.expect(sim_read_val(5, &out) == .OK);
    try std.testing.expectEqual(@as(u32, 2), out.data.u32); // HEATING

    try std.testing.expect(sim_read_val(7, &out) == .OK);
    try std.testing.expect(out.data.b); // heater on

    try std.testing.expect(sim_read_val(8, &out) == .OK);
    try std.testing.expect(out.data.b); // fan on

    try std.testing.expect(sim_read_val(4, &out) == .OK);
    try std.testing.expect(out.data.f32 > 20.0); // temp rose
}

test "read-only signals reject writes" {
    try std.testing.expect(sim_init() == .OK);

    const v = SimValue{ .type = .U32, .data = .{ .u32 = 0 } };
    try std.testing.expect(sim_write_val(5, &v) == .INVALID_SIGNAL);
    try std.testing.expect(sim_write_val(6, &v) == .INVALID_SIGNAL);
    try std.testing.expect(sim_write_val(10, &v) == .INVALID_SIGNAL);
}

test "fault on over-temperature" {
    try std.testing.expect(sim_init() == .OK);

    var v = SimValue{ .type = .BOOL, .data = .{ .b = true } };
    try std.testing.expect(sim_write_val(0, &v) == .OK);

    v = SimValue{ .type = .F32, .data = .{ .f32 = 65.0 } };
    try std.testing.expect(sim_write_val(4, &v) == .OK);

    try std.testing.expect(sim_tick() == .OK);

    var out: SimValue = undefined;
    try std.testing.expect(sim_read_val(5, &out) == .OK);
    try std.testing.expectEqual(@as(u32, 5), out.data.u32); // FAULT

    try std.testing.expect(sim_read_val(9, &out) == .OK);
    try std.testing.expectEqual(@as(u32, 1), out.data.u32); // over-temp code
}
