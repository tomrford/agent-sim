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

    let _ = run_agent(&["--instance", &session, "load", &libpath]);
    let _ = run_agent(&["--instance", &session, "set", "demo.input", "4.0"]);
    let _ = run_agent(&["--instance", &session, "time", "step", "20us"]);

    let value_out = run_agent(&["--instance", &session, "get", "demo.output"]);
    assert!(
        value_out.contains("8"),
        "expected persisted state from previous commands, got: {value_out}"
    );

    let session_out = run_agent(&["--instance", &session, "instance"]);
    assert!(session_out.contains(&session));

    let _ = run_agent(&["--instance", &session, "close"]);
}

#[test]
fn commands_fail_when_session_not_loaded() {
    let session = unique_session("session-not-running");
    let output = common::run_agent_fail(&["--instance", &session, "info"]);
    assert!(
        output.contains("run `agent-sim load <libpath>` first"),
        "expected not-running guidance, got: {output}"
    );
}

#[test]
fn multiple_sessions_run_independent_device_states() {
    ensure_fixtures_built();
    let session_a = unique_session("session-a");
    let session_b = unique_session("session-b");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let _ = run_agent(&["--instance", &session_a, "load", &libpath]);
    let _ = run_agent(&["--instance", &session_b, "load", &libpath]);

    let _ = run_agent(&["--instance", &session_a, "set", "demo.input", "3.0"]);
    let _ = run_agent(&["--instance", &session_a, "time", "step", "20us"]);
    let out_a = run_agent(&["--instance", &session_a, "get", "demo.output"]);
    assert!(
        out_a.contains("6"),
        "expected instance A output 6, got: {out_a}"
    );

    let _ = run_agent(&["--instance", &session_b, "set", "demo.input", "9.0"]);
    let _ = run_agent(&["--instance", &session_b, "time", "step", "20us"]);
    let out_b = run_agent(&["--instance", &session_b, "get", "demo.output"]);
    assert!(
        out_b.contains("18"),
        "expected instance B output 18, got: {out_b}"
    );

    let _ = run_agent(&["--instance", &session_a, "close"]);
    let _ = run_agent(&["--instance", &session_b, "close"]);
}
