const std = @import("std");
const adapter = @import("adapter.zig");
const sim_types = @import("sim_types.zig");

pub const SimCtx = opaque {};
pub const SimStatus = sim_types.SimStatus;
pub const SimValue = sim_types.SimValue;
pub const SimSignalDesc = sim_types.SimSignalDesc;

const CtxImpl = struct {
    magic: u64 = 0x53494D544D504C31,
    inner: adapter.Ctx = .{},
};

const alloc = std.heap.c_allocator;

fn asImpl(raw: ?*SimCtx) ?*CtxImpl {
    const ptr = raw orelse return null;
    const impl: *CtxImpl = @ptrCast(@alignCast(ptr));
    if (impl.magic != 0x53494D544D504C31) return null;
    return impl;
}

pub export fn sim_new() ?*SimCtx {
    const ctx = alloc.create(CtxImpl) catch return null;
    ctx.* = .{};
    adapter.init(&ctx.inner);
    return @ptrCast(ctx);
}

pub export fn sim_free(raw: ?*SimCtx) void {
    const impl = asImpl(raw) orelse return;
    impl.magic = 0;
    alloc.destroy(impl);
}

pub export fn sim_reset(raw: ?*SimCtx) SimStatus {
    const impl = asImpl(raw) orelse return .INVALID_CTX;
    adapter.reset(&impl.inner);
    return .OK;
}

pub export fn sim_tick(raw: ?*SimCtx) SimStatus {
    const impl = asImpl(raw) orelse return .INVALID_CTX;
    adapter.tick(&impl.inner);
    return .OK;
}

pub export fn sim_read_val(raw: ?*SimCtx, id: u32, out: ?*SimValue) SimStatus {
    const impl = asImpl(raw) orelse return .INVALID_CTX;
    const out_val = out orelse return .INVALID_ARG;
    return adapter.read(&impl.inner, id, out_val);
}

pub export fn sim_write_val(raw: ?*SimCtx, id: u32, in: ?*const SimValue) SimStatus {
    const impl = asImpl(raw) orelse return .INVALID_CTX;
    const in_val = in orelse return .INVALID_ARG;
    return adapter.write(&impl.inner, id, in_val);
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

test "template sanity" {
    const ctx = sim_new() orelse return error.OutOfMemory;
    defer sim_free(ctx);

    const in = SimValue{ .type = .F32, .data = .{ .f32 = 5.0 } };
    try std.testing.expect(sim_write_val(ctx, 0, &in) == .OK);
    try std.testing.expect(sim_tick(ctx) == .OK);

    var out: SimValue = undefined;
    try std.testing.expect(sim_read_val(ctx, 1, &out) == .OK);
    try std.testing.expectEqual(@as(f32, 10.0), out.data.f32);
}
