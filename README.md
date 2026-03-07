# agent-sim

Agent-first firmware SIL testing. Load compiled firmware as a shared library, control signals and time from the CLI or an AI agent.

Inspired by [agent-browser](https://github.com/vercel-labs/agent-browser).

## Quick Start

```sh
# Build the HVAC example DLL
nix develop -c bash -c 'cd examples/hvac && zig build'

# Build the runtime
nix develop -c bash -c 'cd runtime && cargo build'

# Load the example, power on, step time, read state
agent-sim load examples/hvac/zig-out/lib/libsim_hvac_example.<so|dylib|dll>
agent-sim set hvac.power true
agent-sim time step 1s
agent-sim get "*"

# Run a recipe
agent-sim run heat-test --config examples/hvac/agent-sim.toml

# Done
agent-sim close
```

## Architecture

```
CLI client  ◄── JSON lines over Unix sockets ──►  Env daemon (optional, env-owned time/CAN)
                                                   ├─ Instance daemons
                                                   │  └─ Project (dlopen DLL, sim_api.h ABI)
                                                   └─ Env-owned CAN manager / logical time
```

Single binary. No external runtime dependencies. Cross-platform (Linux, macOS, Windows).

DLLs are version-checked on load via `sim_get_api_version()`. The runtime accepts only the current ABI version.

## Concepts

| Term         | Meaning                                                                      |
| ------------ | ---------------------------------------------------------------------------- |
| **Project**  | Loaded shared library (`.so`/`.dylib`/`.dll`) implementing `sim_api.h`       |
| **Signal**   | Named, typed value exposed by the project. Address by name, `#id`, or glob   |
| **Tick**     | One simulation quantum. Duration from `sim_get_tick_duration_us()`           |
| **Device**   | Reusable library + optional flash preload definition                          |
| **Instance** | Running simulated instance of a device                                        |
| **Env**      | Coordinated collection of instances with env-owned time and CAN              |
| **Recipe**   | Named command sequence in `agent-sim.toml`                                   |

## Commands

```
agent-sim load [libpath]              # Start instance daemon bound to DLL
agent-sim info                        # Project metadata
agent-sim signals                     # List all signals
agent-sim reset                       # Reset device state to deterministic startup

agent-sim get <signal> [<signal>...]  # Read signals (name, #id, or glob)
agent-sim set <sig>=<val> [...]       # Write signals (batch supported)
agent-sim watch <signal> [ms]         # Stream signal values

agent-sim time start|pause|step|speed|status

agent-sim env start <name>
agent-sim env status <name>
agent-sim env reset <name>
agent-sim env time <name> start|pause|step|speed|status
agent-sim env can <name> buses|inspect|send|load-dbc|schedule ...

agent-sim run <recipe>                # Execute recipe from config
agent-sim instance [list]             # Instance info
agent-sim close                       # Shut down daemon
```

Global flags: `--json`, `--instance <name>`, `--config <path>`.

`load` is a bootstrap command: one instance daemon is permanently bound to one DLL for its lifetime. To run a different DLL or a second device, use a different `--instance`.

Flash preloads are part of load/device startup:

```sh
agent-sim load ./zig-out/lib/libsim_example.so --flash ./cal.hex
agent-sim load ./zig-out/lib/libsim_example.so --flash ./blob.bin:0x08040000
```

## Configuration

First-match priority:

1. `--config <path>` CLI flag
2. `AGENT_SIM_CONFIG` env var
3. `./agent-sim.toml` in working directory
4. Empty defaults

See `examples/hvac/agent-sim.toml` for device/env/recipe examples.

## Creating a DLL

Use the `template/` scaffold. Edit `src/adapter.zig` (logic + signals) and `project.zig` (name + includes).

- Human-oriented authoring guide: `docs/template-guide.md`
- Agent-oriented build/test guide: `docs/agent-guide.md`

CAN transport notes:

- Linux: SocketCAN interface names
- Windows: Peak CAN channel names (`usb1`, `usb2`, `pci1`, ...)

## Project Structure

```
runtime/         Rust CLI + daemon (Cargo)
template/        Zig shared-library template
examples/hvac/   HVAC thermostat example DLL
include/         sim_api.h — shared C ABI contract
docs/            guides for DLL authors and agents
```

## Toolchain

All toolchains managed via Nix (`flake.nix`). Install via `nix build` or develop with `nix develop`.

```sh
nix build            # Build agent-sim binary
nix develop          # Enter dev shell (Rust + Zig)
```

## License

See [LICENSE.md](LICENSE.md).
