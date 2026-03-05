pub const SignalId = u32;

pub const SimStatus = enum(u32) {
    OK = 0,
    NOT_INITIALIZED = 1,
    INVALID_ARG = 2,
    INVALID_SIGNAL = 3,
    TYPE_MISMATCH = 4,
    BUFFER_TOO_SMALL = 5,
    INTERNAL = 255,
};

pub const SimType = enum(u32) {
    BOOL = 0,
    U32 = 1,
    I32 = 2,
    F32 = 3,
    F64 = 4,
};

pub const SimValueData = extern union {
    b: bool,
    u32: u32,
    i32: i32,
    f32: f32,
    f64: f64,
};

pub const SimValue = extern struct {
    type: SimType,
    data: SimValueData,
};

pub const SimSignalDesc = extern struct {
    id: SignalId,
    name: [*:0]const u8,
    type: SimType,
    units: ?[*:0]const u8,
};

pub const SimCanFrame = extern struct {
    arb_id: u32,
    len: u8,
    flags: u8,
    _pad: [2]u8,
    data: [64]u8,
};

pub const SimCanBusDesc = extern struct {
    id: u32,
    name: [*:0]const u8,
    bitrate: u32,
    bitrate_data: u32,
    flags: u8,
    _pad: [3]u8,
};
