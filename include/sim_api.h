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
 * - One opaque context per simulated device instance.
 * - Host drives simulation explicitly in fixed quanta via sim_tick().
 * - Host discovers signal surface at runtime via metadata APIs.
 * - All read/write operations are runtime type-checked.
 *
 * Typical host flow:
 * 1. sim_get_tick_duration_us()
 * 2. sim_get_signal_count() + sim_get_signals()
 * 3. sim_new()
 * 4. loop: sim_write_val() -> sim_tick() -> sim_read_val()
 * 5. sim_free()
 *
 * Threading contract:
 * - A single SimCtx must not be used concurrently from multiple threads.
 * - Different SimCtx objects may be used on different threads.
 * - Host should serialize operations per SimCtx.
 */

/** Major ABI version for this header contract. */
#define SIM_API_VERSION_MAJOR 1U
/** Minor ABI version for additive non-breaking changes. */
#define SIM_API_VERSION_MINOR 0U

/** Opaque simulation context (owned by DLL). */
typedef struct SimCtx SimCtx;

/** Runtime signal identifier (discovered via metadata APIs). */
typedef uint32_t SignalId;

/**
 * @brief Status codes returned by API calls.
 */
typedef enum {
  /** Call succeeded. */
  SIM_OK = 0,

  /** SimCtx pointer was null, freed, or not recognized by this DLL instance. */
  SIM_ERR_INVALID_CTX = 1,

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
 * @brief Allocate a new simulation context.
 *
 * Context is returned already reset to deterministic post-startup state.
 *
 * @return non-null on success, NULL on allocation/initialization failure.
 */
SimCtx *sim_new(void);

/**
 * @brief Free a simulation context.
 *
 * Safe with NULL (no-op).
 */
void sim_free(SimCtx *ctx);

/**
 * @brief Reset context to deterministic startup state.
 */
SimStatus sim_reset(SimCtx *ctx);

/**
 * @brief Advance simulation by exactly one tick quantum.
 *
 * Tick duration is reported by sim_get_tick_duration_us().
 */
SimStatus sim_tick(SimCtx *ctx);

/**
 * @brief Read current signal value.
 *
 * @param ctx simulation context
 * @param id signal identifier from catalog
 * @param out output SimValue (must be non-null)
 */
SimStatus sim_read_val(SimCtx *ctx, SignalId id, SimValue *out);

/**
 * @brief Write signal value (applied to context state).
 *
 * Type must match catalog metadata exactly.
 */
SimStatus sim_write_val(SimCtx *ctx, SignalId id, const SimValue *in);

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

#ifdef __cplusplus
}
#endif

#endif // SIM_API_H
