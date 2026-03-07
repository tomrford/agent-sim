const std = @import("std");
const sim_types = @import("shared_sim_types");

pub const SimStatus = sim_types.SimStatus;
pub const SimValue = sim_types.SimValue;
pub const SimSignalDesc = sim_types.SimSignalDesc;
pub const SimCanFrame = sim_types.SimCanFrame;
pub const SimCanBusDesc = sim_types.SimCanBusDesc;
pub const SimSharedDesc = sim_types.SimSharedDesc;
pub const SimSharedSlot = sim_types.SimSharedSlot;

pub const TickDurationUs: u32 = 20;

pub const FlashBase: u32 = 0x0800_0000;
pub const FlashSize: u32 = 256;

pub const NonVolatileState = struct {
    flash: [FlashSize]u8 = [_]u8{0xFF} ** FlashSize,
};

pub const VolatileState = struct {
    input: f32 = 0.0,
    output: f32 = 0.0,
};

pub const Ctx = struct {
    non_volatile: NonVolatileState = .{},
    runtime: VolatileState = .{},
};

const Signal = enum(u32) {
    input = 0,
    output = 1,
    flash_value = 2,
};

const signal_catalog = [_]Signal{
    .input,
    .output,
    .flash_value,
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

pub fn init(ctx: *Ctx) SimStatus {
    ctx.runtime = .{};
    return .OK;
}

pub fn reset(ctx: *Ctx) void {
    ctx.runtime = .{};
}

pub fn tick(ctx: *Ctx) void {
    ctx.runtime.output = ctx.runtime.input * 2.0;
}

pub fn signalCount() u32 {
    return signal_catalog.len;
}

pub fn fillSignals(out: [*]SimSignalDesc, capacity: u32, out_written: *u32) SimStatus {
    const n: u32 = @min(capacity, signal_catalog.len);
    var i: u32 = 0;
    while (i < n) : (i += 1) {
        out[i] = signalDesc(signal_catalog[i]);
    }
    out_written.* = n;
    return if (capacity < signal_catalog.len) .BUFFER_TOO_SMALL else .OK;
}

pub fn read(ctx: *Ctx, id: u32, out: *SimValue) SimStatus {
    const signal = std.meta.intToEnum(Signal, id) catch return .INVALID_SIGNAL;
    switch (signal) {
        .input => {
            out.* = .{ .type = .F32, .data = .{ .f32 = ctx.runtime.input } };
            return .OK;
        },
        .output => {
            out.* = .{ .type = .F32, .data = .{ .f32 = ctx.runtime.output } };
            return .OK;
        },
        .flash_value => {
            out.* = .{ .type = .U32, .data = .{ .u32 = flashValue(ctx) } };
            return .OK;
        },
    }
}

pub fn write(ctx: *Ctx, id: u32, in: *const SimValue) SimStatus {
    const signal = std.meta.intToEnum(Signal, id) catch return .INVALID_SIGNAL;
    if (signal != .input) return .INVALID_SIGNAL;
    if (in.type != .F32) return .TYPE_MISMATCH;
    ctx.runtime.input = in.data.f32;
    return .OK;
}

pub fn flashWrite(ctx: *Ctx, base_addr: u32, data: [*]const u8, len: u32) SimStatus {
    if (len == 0) return .OK;
    if (base_addr < FlashBase) return .INVALID_ARG;
    const offset = base_addr - FlashBase;
    if (@as(u64, offset) + @as(u64, len) > FlashSize) return .INVALID_ARG;
    @memcpy(ctx.non_volatile.flash[offset .. offset + len], data[0..len]);
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
    if (count != shared_channels[0].slot_count) return .INVALID_ARG;
    var i: u32 = 0;
    while (i < count) : (i += 1) {
        const slot = slots[i];
        if (slot.slot_id != i) return .INVALID_ARG;
        if (slot.type != slot.value.type) return .TYPE_MISMATCH;
        switch (slot.slot_id) {
            0 => {
                if (slot.value.type != .F32) return .TYPE_MISMATCH;
                ctx.runtime.input = slot.value.data.f32;
            },
            1 => {
                if (slot.value.type != .F32) return .TYPE_MISMATCH;
                _ = slot.value.data.f32;
            },
            else => return .INVALID_ARG,
        }
    }
    return .OK;
}

pub fn sharedWrite(ctx: *Ctx, channel_id: u32, out: [*]SimSharedSlot, capacity: u32, out_written: *u32) SimStatus {
    if (channel_id != 0) return .INVALID_ARG;
    const required = shared_channels[0].slot_count;
    const written: u32 = @min(capacity, required);
    if (written > 0) {
        out[0] = .{ .slot_id = 0, .type = .F32, .value = .{ .type = .F32, .data = .{ .f32 = ctx.runtime.input } } };
    }
    if (written > 1) {
        out[1] = .{ .slot_id = 1, .type = .F32, .value = .{ .type = .F32, .data = .{ .f32 = ctx.runtime.output } } };
    }
    out_written.* = written;
    return if (written < required) .BUFFER_TOO_SMALL else .OK;
}

fn flashValue(ctx: *const Ctx) u32 {
    return std.mem.readInt(u32, ctx.non_volatile.flash[0..4], .little);
}

fn signalDesc(signal: Signal) SimSignalDesc {
    return switch (signal) {
        .input => .{ .id = @intFromEnum(signal), .name = "demo.input", .type = .F32, .units = null },
        .output => .{ .id = @intFromEnum(signal), .name = "demo.output", .type = .F32, .units = null },
        .flash_value => .{ .id = @intFromEnum(signal), .name = "demo.flash_value", .type = .U32, .units = null },
    };
}
