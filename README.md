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
CLI client  ◄── JSON lines over Unix socket ──►  Daemon (same binary, --daemon)
                                                   ├─ Project (dlopen DLL, sim_api.h ABI)
                                                   │  └─ Instance 0..N (SimCtx*)
                                                   └─ Time Engine (tick loop)
```

Single binary. No external runtime dependencies. Cross-platform (Linux, macOS, Windows).

## Concepts

| Term         | Meaning                                                                      |
| ------------ | ---------------------------------------------------------------------------- |
| **Project**  | Loaded shared library (`.so`/`.dylib`/`.dll`) implementing `sim_api.h`       |
| **Instance** | One `SimCtx*` handle — a simulated device                                    |
| **Signal**   | Named, typed value exposed by the project. Address by name, `#id`, or glob   |
| **Tick**     | One simulation quantum. Duration from `sim_get_tick_duration_us()`           |
| **Session**  | Isolated daemon process with its own socket, project, instances, time engine |
| **Recipe**   | Named command sequence in `agent-sim.toml`                                   |

## Commands

```
agent-sim load <libpath>              # Load project
agent-sim unload                      # Unload project
agent-sim info                        # Project metadata
agent-sim signals                     # List all signals

agent-sim get <signal> [<signal>...]  # Read signals (name, #id, or glob)
agent-sim set <sig>=<val> [...]       # Write signals (batch supported)
agent-sim watch <signal> [ms]         # Stream signal values

agent-sim instance new|list|select|reset|free
agent-sim time start|pause|step|speed|status

agent-sim run <recipe>                # Execute recipe from config
agent-sim session [list]              # Session info
agent-sim close                       # Shut down daemon
```

Global flags: `--json`, `--session <name>`, `--instance <index>`, `--config <path>`.

## Configuration

First-match priority:

1. `--config <path>` CLI flag
2. `AGENT_SIM_CONFIG` env var
3. `./agent-sim.toml` in working directory
4. Empty defaults

See `examples/hvac/agent-sim.toml` for a complete recipe reference.

## Creating a DLL

Use the `template/` scaffold. Edit `src/adapter.zig` (logic + signals) and `project.zig` (name + includes). See `docs/template-guide.md`.

## Project Structure

```
runtime/         Rust CLI + daemon (Cargo)
template/        Zig shared-library template
examples/hvac/   HVAC thermostat example DLL
include/         sim_api.h — shared C ABI contract
docs/            template guide
```

## Toolchain

All toolchains managed via Nix (`flake.nix`). Install via `nix build` or develop with `nix develop`.

```sh
nix build            # Build agent-sim binary
nix develop          # Enter dev shell (Rust + Zig)
```

## License

See [LICENSE.md](LICENSE.md).
