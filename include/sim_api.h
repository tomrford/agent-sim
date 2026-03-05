#ifndef SIM_API_H
#define SIM_API_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/**
 * @file sim_api.h
 * @brief Stable C ABI for firmware simulation DLLs.
 *
 * Design intent:
 * - One DLL process hosts one simulation device state.
 * - Host drives simulation explicitly in fixed quanta via sim_tick().
 * - Host discovers signal surface at runtime via metadata APIs.
 * - All read/write operations are runtime type-checked.
 *
 * Typical host flow:
 * 1. sim_get_tick_duration_us()
 * 2. sim_get_signal_count() + sim_get_signals()
 * 3. sim_init()
 * 4. loop: sim_write_val() -> sim_tick() -> sim_read_val()
 * 5. sim_reset() as needed
 *
 * Threading contract:
 * - Calls must be serialized per loaded DLL.
 * - Concurrent calls into a single loaded DLL are not supported.
 */

/** Major ABI version for this header contract. */
#define SIM_API_VERSION_MAJOR 1U
/** Minor ABI version for additive non-breaking changes. */
#define SIM_API_VERSION_MINOR 0U

/** Runtime signal identifier (discovered via metadata APIs). */
typedef uint32_t SignalId;

/**
 * @brief Status codes returned by API calls.
 */
typedef enum {
  /** Call succeeded. */
  SIM_OK = 0,

  /** Simulation state has not been initialized via sim_init(). */
  SIM_ERR_NOT_INITIALIZED = 1,

  /** One or more input pointers/arguments are invalid. */
  SIM_ERR_INVALID_ARG = 2,

  /** SignalId does not exist in the active catalog. */
  SIM_ERR_INVALID_SIGNAL = 3,

  /** SimValue.type does not match signal metadata type for write/read contract.
   */
  SIM_ERR_TYPE_MISMATCH = 4,

  /** Output buffer capacity was smaller than required; partial fill may occur.
   */
  SIM_ERR_BUFFER_TOO_SMALL = 5,

  /** Unexpected internal failure. */
  SIM_ERR_INTERNAL = 255,
} SimStatus;

/**
 * @brief Runtime scalar types used by SimValue and signal metadata.
 */
typedef enum {
  SIM_TYPE_BOOL = 0,
  SIM_TYPE_U32 = 1,
  SIM_TYPE_I32 = 2,
  SIM_TYPE_F32 = 3,
  SIM_TYPE_F64 = 4,
} SimType;

/**
 * @brief Tagged scalar value used by read/write APIs.
 *
 * For writes:
 * - set `type` to the target signal type
 * - populate the matching union field
 *
 * For reads:
 * - DLL writes both `type` and matching union field
 */
typedef struct {
  SimType type;
  union {
    bool b;
    uint32_t u32;
    int32_t i32;
    float f32;
    double f64;
  } data;
} SimValue;

/**
 * @brief Signal metadata entry returned by sim_get_signals().
 *
 * Notes:
 * - `id` is used for read/write calls.
 * - `name` is null-terminated and owned by DLL (do not free).
 * - `units` may be NULL.
 * - Signal IDs/names are stable only within a given build unless project policy
 * says otherwise.
 */
typedef struct {
  SignalId id;
  const char *name;
  SimType type;
  const char *units;
} SimSignalDesc;

/**
 * @brief Initialize simulation state.
 *
 * Implementations must set deterministic startup state.
 * Safe to call multiple times; each call should restore deterministic startup
 * state.
 */
SimStatus sim_init(void);

/**
 * @brief Reset simulation state to deterministic startup state.
 */
SimStatus sim_reset(void);

/**
 * @brief Advance simulation by exactly one tick quantum.
 *
 * Tick duration is reported by sim_get_tick_duration_us().
 */
SimStatus sim_tick(void);

/**
 * @brief Read current signal value.
 *
 * @param id signal identifier from catalog
 * @param out output SimValue (must be non-null)
 */
SimStatus sim_read_val(SignalId id, SimValue *out);

/**
 * @brief Write signal value (applied to simulation state).
 *
 * Type must match catalog metadata exactly.
 */
SimStatus sim_write_val(SignalId id, const SimValue *in);

/**
 * @brief Get number of signals in current catalog.
 */
SimStatus sim_get_signal_count(uint32_t *out_count);

/**
 * @brief Fill signal metadata array.
 *
 * Behavior:
 * - writes up to `capacity` entries to `out`
 * - writes actual entries written to `out_written`
 * - returns SIM_ERR_BUFFER_TOO_SMALL if capacity < total count
 * - can be called with capacity==0 and out==NULL to probe size
 */
SimStatus sim_get_signals(SimSignalDesc *out, uint32_t capacity,
                          uint32_t *out_written);

/**
 * @brief Get fixed tick duration in microseconds for this DLL build.
 */
SimStatus sim_get_tick_duration_us(uint32_t *out_tick_us);

/**
 * @brief CAN frame structure for classic CAN and CAN FD.
 *
 * Flag bits:
 *   bit 0: extended frame (29-bit arbitration ID)
 *   bit 1: FD frame (data may exceed 8 bytes)
 *   bit 2: BRS (bit rate switch, FD only)
 *   bit 3: ESI (error state indicator, FD only)
 *   bit 4: RTR (remote transmission request, classic only)
 *   bits 5-7: reserved (must be zero)
 */
typedef struct {
  uint32_t arb_id;
  uint8_t len;
  uint8_t flags;
  uint8_t _pad[2];
  uint8_t data[64];
} SimCanFrame;

/**
 * @brief CAN bus descriptor returned by sim_can_get_buses().
 */
typedef struct {
  uint32_t id;
  const char *name;
  uint32_t bitrate;
  uint32_t bitrate_data;
  uint8_t flags; /* bit 0: FD capable */
  uint8_t _pad[3];
} SimCanBusDesc;

/**
 * @brief Enumerate CAN buses exposed by the DLL.
 *
 * Optional export: if any sim_can_* symbol is exported, all must be exported.
 */
SimStatus sim_can_get_buses(SimCanBusDesc *out, uint32_t capacity,
                            uint32_t *out_written);

/**
 * @brief Deliver received CAN frames to the DLL before sim_tick().
 */
SimStatus sim_can_rx(uint32_t bus_id, const SimCanFrame *frames, uint32_t count);

/**
 * @brief Collect CAN frames queued for TX by the DLL after sim_tick().
 *
 * Returns SIM_ERR_BUFFER_TOO_SMALL for partial fills; host should call again.
 */
SimStatus sim_can_tx(uint32_t bus_id, SimCanFrame *out, uint32_t capacity,
                     uint32_t *out_written);

/**
 * @brief Shared-state channel descriptor.
 */
typedef struct {
  uint32_t id;
  const char *name;
  uint32_t slot_count;
} SimSharedDesc;

/**
 * @brief Shared-state slot payload.
 */
typedef struct {
  uint32_t slot_id;
  SimType type;
  SimValue value;
} SimSharedSlot;

/**
 * @brief Enumerate shared-state channels exposed by the DLL.
 *
 * Optional export: if any sim_shared_* symbol is exported, all must be exported.
 */
SimStatus sim_shared_get_channels(SimSharedDesc *out, uint32_t capacity,
                                  uint32_t *out_written);

/**
 * @brief Read inbound shared-state snapshot before sim_tick().
 */
SimStatus sim_shared_read(uint32_t channel_id, const SimSharedSlot *slots,
                          uint32_t count);

/**
 * @brief Collect outbound shared-state snapshot after sim_tick().
 */
SimStatus sim_shared_write(uint32_t channel_id, SimSharedSlot *out,
                           uint32_t capacity, uint32_t *out_written);

#ifdef __cplusplus
}
#endif

#endif // SIM_API_H
