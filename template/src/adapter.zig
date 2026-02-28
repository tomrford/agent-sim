const sim_types = @import("sim_types.zig");

pub const SimStatus = sim_types.SimStatus;
pub const SimType = sim_types.SimType;
pub const SimValue = sim_types.SimValue;
pub const SimSignalDesc = sim_types.SimSignalDesc;

pub const TickDurationUs: u32 = 20;

pub const Ctx = struct {
    input: f32 = 0.0,
    output: f32 = 0.0,
};

const signals = [_]SimSignalDesc{
    .{ .id = 0, .name = "demo.input", .type = .F32, .units = null },
    .{ .id = 1, .name = "demo.output", .type = .F32, .units = null },
};

pub fn init(ctx: *Ctx) void {
    ctx.* = .{};
}

pub fn reset(ctx: *Ctx) void {
    ctx.* = .{};
}

pub fn tick(ctx: *Ctx) void {
    ctx.output = ctx.input * 2.0;
}

pub fn signalCount() u32 {
    return signals.len;
}

pub fn fillSignals(out: [*]SimSignalDesc, capacity: u32, out_written: *u32) SimStatus {
    const n: u32 = @min(capacity, signals.len);
    var i: u32 = 0;
    while (i < n) : (i += 1) out[i] = signals[i];
    out_written.* = n;
    return if (capacity < signals.len) .BUFFER_TOO_SMALL else .OK;
}

pub fn read(ctx: *Ctx, id: u32, out: *SimValue) SimStatus {
    switch (id) {
        0 => {
            out.* = .{ .type = .F32, .data = .{ .f32 = ctx.input } };
            return .OK;
        },
        1 => {
            out.* = .{ .type = .F32, .data = .{ .f32 = ctx.output } };
            return .OK;
        },
        else => return .INVALID_SIGNAL,
    }
}

pub fn write(ctx: *Ctx, id: u32, in: *const SimValue) SimStatus {
    if (id != 0) return .INVALID_SIGNAL;
    if (in.type != .F32) return .TYPE_MISMATCH;
    ctx.input = in.data.f32;
    return .OK;
}
