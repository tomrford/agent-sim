mod common;

use common::{ensure_fixtures_built, run_agent, template_lib_path, unique_session};

#[test]
fn session_state_persists_across_invocations() {
    ensure_fixtures_built();
    let session = unique_session("session");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let _ = run_agent(&["--session", &session, "load", &libpath]);
    let _ = run_agent(&["--session", &session, "set", "demo.input", "4.0"]);
    let _ = run_agent(&["--session", &session, "time", "step", "20us"]);

    let value_out = run_agent(&["--session", &session, "get", "demo.output"]);
    assert!(
        value_out.contains("8"),
        "expected persisted state from previous commands, got: {value_out}"
    );

    let session_out = run_agent(&["--session", &session, "session"]);
    assert!(session_out.contains(&session));

    let _ = run_agent(&["--session", &session, "close"]);
}

#[test]
fn commands_fail_when_session_not_loaded() {
    let session = unique_session("session-not-running");
    let output = common::run_agent_fail(&["--session", &session, "info"]);
    assert!(
        output.contains("run `agent-sim load <libpath>` first"),
        "expected not-running guidance, got: {output}"
    );
}
