const sim_types = @import("sim_types.zig");

pub const SimStatus = sim_types.SimStatus;
pub const SimValue = sim_types.SimValue;
pub const SimSignalDesc = sim_types.SimSignalDesc;

pub const TickDurationUs: u32 = 10_000; // 10 ms per tick

// -- Tuning constants ---------------------------------------------------------

const deadband: f32 = 1.0; // +/- hysteresis around target
const heat_rate: f32 = 0.02; // degC per tick while heater is on
const cool_rate: f32 = 0.015; // degC per tick while compressor is on
const ambient_drift: f32 = 0.005; // degC per tick toward ambient
const compressor_lockout: u32 = 300; // min off-ticks before compressor can restart
const fan_overrun: u32 = 100; // fan keeps running after heat/cool stops
const temp_hi_limit: f32 = 60.0;
const temp_lo_limit: f32 = -40.0;

// -- State / Mode enums -------------------------------------------------------

const State = enum(u32) { off = 0, idle = 1, heating = 2, cooling = 3, fan_drain = 4, fault = 5 };
const Mode = enum(u32) { auto = 0, heat_only = 1, cool_only = 2 };

// -- Context ------------------------------------------------------------------

pub const Ctx = struct {
    // Writable inputs
    power: bool = false,
    target_temp: f32 = 22.0,
    mode: u32 = 0,
    ambient_temp: f32 = 20.0,
    current_temp: f32 = 20.0,

    // Read-only outputs
    state: State = .off,
    compressor: bool = false,
    heater: bool = false,
    fan: bool = false,
    error_code: u32 = 0,
    uptime: u32 = 0,

    // Internal timers (not exposed as signals)
    comp_off_timer: u32 = 0,
    fan_timer: u32 = 0,
};

// -- Signal catalog -----------------------------------------------------------
//
//  IDs 0-4  : writable inputs
//  IDs 5-10 : read-only outputs

const signals = [_]SimSignalDesc{
    .{ .id = 0, .name = "hvac.power", .type = .BOOL, .units = null },
    .{ .id = 1, .name = "hvac.target_temp", .type = .F32, .units = "degC" },
    .{ .id = 2, .name = "hvac.mode", .type = .U32, .units = null },
    .{ .id = 3, .name = "hvac.ambient_temp", .type = .F32, .units = "degC" },
    .{ .id = 4, .name = "hvac.current_temp", .type = .F32, .units = "degC" },
    .{ .id = 5, .name = "hvac.state", .type = .U32, .units = null },
    .{ .id = 6, .name = "hvac.compressor", .type = .BOOL, .units = null },
    .{ .id = 7, .name = "hvac.heater", .type = .BOOL, .units = null },
    .{ .id = 8, .name = "hvac.fan", .type = .BOOL, .units = null },
    .{ .id = 9, .name = "hvac.error_code", .type = .U32, .units = null },
    .{ .id = 10, .name = "hvac.uptime", .type = .U32, .units = "ticks" },
};

// -- Public API ---------------------------------------------------------------

pub fn init(ctx: *Ctx) SimStatus {
    ctx.* = .{};
    return .OK;
}

pub fn reset(ctx: *Ctx) void {
    ctx.* = .{};
}

pub fn tick(ctx: *Ctx) void {
    if (!ctx.power) {
        ctx.state = .off;
        ctx.compressor = false;
        ctx.heater = false;
        ctx.fan = false;
        ctx.uptime = 0;
        ctx.comp_off_timer = 0;
        ctx.fan_timer = 0;
        driftToward(ctx);
        return;
    }

    ctx.uptime +|= 1;
    if (ctx.comp_off_timer > 0) ctx.comp_off_timer -= 1;
    if (ctx.fan_timer > 0) ctx.fan_timer -= 1;

    if (ctx.current_temp >= temp_hi_limit or ctx.current_temp <= temp_lo_limit) {
        ctx.state = .fault;
        ctx.error_code = if (ctx.current_temp >= temp_hi_limit) 1 else 2;
        ctx.compressor = false;
        ctx.heater = false;
        ctx.fan = false;
        return;
    }

    if (ctx.state == .fault) return; // latches until power cycle

    const mode: Mode = if (ctx.mode <= 2) @enumFromInt(ctx.mode) else .auto;
    const want_heat = (mode == .auto or mode == .heat_only) and
        ctx.current_temp < ctx.target_temp - deadband;
    const want_cool = (mode == .auto or mode == .cool_only) and
        ctx.current_temp > ctx.target_temp + deadband;

    switch (ctx.state) {
        .off, .idle => {
            if (want_heat) {
                ctx.state = .heating;
                ctx.heater = true;
                ctx.fan = true;
            } else if (want_cool and ctx.comp_off_timer == 0) {
                ctx.state = .cooling;
                ctx.compressor = true;
                ctx.fan = true;
            } else {
                ctx.state = .idle;
            }
        },
        .heating => {
            if (!want_heat) {
                ctx.heater = false;
                ctx.fan_timer = fan_overrun;
                ctx.state = .fan_drain;
            }
        },
        .cooling => {
            if (!want_cool) {
                ctx.compressor = false;
                ctx.comp_off_timer = compressor_lockout;
                ctx.fan_timer = fan_overrun;
                ctx.state = .fan_drain;
            }
        },
        .fan_drain => {
            if (ctx.fan_timer == 0) {
                ctx.fan = false;
                ctx.state = .idle;
            }
        },
        .fault => {},
    }

    if (ctx.heater) ctx.current_temp += heat_rate;
    if (ctx.compressor) ctx.current_temp -= cool_rate;
    driftToward(ctx);
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
        0 => out.* = .{ .type = .BOOL, .data = .{ .b = ctx.power } },
        1 => out.* = .{ .type = .F32, .data = .{ .f32 = ctx.target_temp } },
        2 => out.* = .{ .type = .U32, .data = .{ .u32 = ctx.mode } },
        3 => out.* = .{ .type = .F32, .data = .{ .f32 = ctx.ambient_temp } },
        4 => out.* = .{ .type = .F32, .data = .{ .f32 = ctx.current_temp } },
        5 => out.* = .{ .type = .U32, .data = .{ .u32 = @intFromEnum(ctx.state) } },
        6 => out.* = .{ .type = .BOOL, .data = .{ .b = ctx.compressor } },
        7 => out.* = .{ .type = .BOOL, .data = .{ .b = ctx.heater } },
        8 => out.* = .{ .type = .BOOL, .data = .{ .b = ctx.fan } },
        9 => out.* = .{ .type = .U32, .data = .{ .u32 = ctx.error_code } },
        10 => out.* = .{ .type = .U32, .data = .{ .u32 = ctx.uptime } },
        else => return .INVALID_SIGNAL,
    }
    return .OK;
}

pub fn write(ctx: *Ctx, id: u32, in: *const SimValue) SimStatus {
    switch (id) {
        0 => {
            if (in.type != .BOOL) return .TYPE_MISMATCH;
            ctx.power = in.data.b;
        },
        1 => {
            if (in.type != .F32) return .TYPE_MISMATCH;
            ctx.target_temp = in.data.f32;
        },
        2 => {
            if (in.type != .U32) return .TYPE_MISMATCH;
            ctx.mode = in.data.u32;
        },
        3 => {
            if (in.type != .F32) return .TYPE_MISMATCH;
            ctx.ambient_temp = in.data.f32;
        },
        4 => {
            if (in.type != .F32) return .TYPE_MISMATCH;
            ctx.current_temp = in.data.f32;
        },
        else => return .INVALID_SIGNAL,
    }
    return .OK;
}

// -- Internal -----------------------------------------------------------------

fn driftToward(ctx: *Ctx) void {
    const diff = ctx.ambient_temp - ctx.current_temp;
    if (diff > ambient_drift) {
        ctx.current_temp += ambient_drift;
    } else if (diff < -ambient_drift) {
        ctx.current_temp -= ambient_drift;
    } else {
        ctx.current_temp = ctx.ambient_temp;
    }
}
