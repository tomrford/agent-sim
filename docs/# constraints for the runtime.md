# constraints for the runtime

- AX (Agent experience) needs to be clean for minimal token usage. Workflow should be similar to vercel's https://github.com/vercel-labs/agent-browser.
- Since we have a very large number of signals to play with and many need to be changed in parallel, we should consider how the CLI could handle multi-value changes in a single call.
- We will need 'sessions' and 'IPC' between sessions eventually so we can parallel test firmware that can communicate with itself. This can be V2.
- cli commands/structure should match agent-browser.
- We need to control the runtime for each linked library and these should be passed as args at runtime.
- How do we achieve the same 'stateful cli' experience as agent-browser?

Usage should be something like:

agent-sim new <libpath>
agent-sim list

agent-sim start
agent-sim pause
agent-sim speed
agent-sim reset
agent-sim stop

then getters/setters for the signals, per-active simulation:
agent-sim list-signals
agent-sim get <signal>
agent-sim set <signal> <value>

These possibly need to be more complex to handle things like structs etc, depending on how granular signals are exposed.

I was considering an option for defining pre-set instructions in a config file of sorts, like a config.agent-sim.toml file that can be written/saved by the user 'per project' for things like a 'init' command that sets a pre-known/hardcoded list of 10/20 signals to specific values. Then we could also have 'steps' or similar in there to allow for deterministic testing of firmware inside CI or similar for integration testing.

these could be called something like:
agent-sim run <batched-command-name>
