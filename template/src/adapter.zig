const sim_types = @import("sim_types.zig");

pub const SimStatus = sim_types.SimStatus;
pub const SimType = sim_types.SimType;
pub const SimValue = sim_types.SimValue;
pub const SimSignalDesc = sim_types.SimSignalDesc;
pub const SimCanFrame = sim_types.SimCanFrame;
pub const SimCanBusDesc = sim_types.SimCanBusDesc;
pub const SimSharedDesc = sim_types.SimSharedDesc;
pub const SimSharedSlot = sim_types.SimSharedSlot;

pub const TickDurationUs: u32 = 20;

pub const Ctx = struct {
    input: f32 = 0.0,
    output: f32 = 0.0,
};

const signals = [_]SimSignalDesc{
    .{ .id = 0, .name = "demo.input", .type = .F32, .units = null },
    .{ .id = 1, .name = "demo.output", .type = .F32, .units = null },
};

pub const can_buses = [_]SimCanBusDesc{
    .{
        .id = 0,
        .name = "internal",
        .bitrate = 500_000,
        .bitrate_data = 0,
        .flags = 0,
        ._pad = .{ 0, 0, 0 },
    },
    .{
        .id = 1,
        .name = "external",
        .bitrate = 500_000,
        .bitrate_data = 2_000_000,
        .flags = 0x01,
        ._pad = .{ 0, 0, 0 },
    },
};

pub const shared_channels = [_]SimSharedDesc{
    .{
        .id = 0,
        .name = "sensor_feed",
        .slot_count = 2,
    },
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

pub fn canBusCount() u32 {
    return can_buses.len;
}

pub fn fillCanBuses(out: [*]SimCanBusDesc, capacity: u32, out_written: *u32) SimStatus {
    const n: u32 = @min(capacity, can_buses.len);
    var i: u32 = 0;
    while (i < n) : (i += 1) out[i] = can_buses[i];
    out_written.* = n;
    return if (capacity < can_buses.len) .BUFFER_TOO_SMALL else .OK;
}

pub fn canRx(ctx: *Ctx, bus_id: u32, frames: [*]const SimCanFrame, count: u32) void {
    _ = ctx;
    _ = bus_id;
    _ = frames;
    _ = count;
}

pub fn canTx(ctx: *Ctx, bus_id: u32, out: [*]SimCanFrame, capacity: u32, out_written: *u32) SimStatus {
    _ = ctx;
    _ = bus_id;
    _ = out;
    _ = capacity;
    out_written.* = 0;
    return .OK;
}

pub fn sharedChannelCount() u32 {
    return shared_channels.len;
}

pub fn fillSharedChannels(out: [*]SimSharedDesc, capacity: u32, out_written: *u32) SimStatus {
    const n: u32 = @min(capacity, shared_channels.len);
    var i: u32 = 0;
    while (i < n) : (i += 1) out[i] = shared_channels[i];
    out_written.* = n;
    return if (capacity < shared_channels.len) .BUFFER_TOO_SMALL else .OK;
}

pub fn sharedRead(ctx: *Ctx, channel_id: u32, slots: [*]const SimSharedSlot, count: u32) SimStatus {
    if (channel_id != 0) return .INVALID_ARG;
    var i: u32 = 0;
    while (i < count) : (i += 1) {
        const slot = slots[i];
        if (slot.slot_id == 0 and slot.value.type == .F32) {
            ctx.input = slot.value.data.f32;
        }
    }
    return .OK;
}

pub fn sharedWrite(ctx: *Ctx, channel_id: u32, out: [*]SimSharedSlot, capacity: u32, out_written: *u32) SimStatus {
    if (channel_id != 0) return .INVALID_ARG;
    if (capacity < 2) {
        out_written.* = capacity;
        return .BUFFER_TOO_SMALL;
    }
    out[0] = .{ .slot_id = 0, .type = .F32, .value = .{ .type = .F32, .data = .{ .f32 = ctx.input } } };
    out[1] = .{ .slot_id = 1, .type = .F32, .value = .{ .type = .F32, .data = .{ .f32 = ctx.output } } };
    out_written.* = 2;
    return .OK;
}
