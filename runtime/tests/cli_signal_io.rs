mod common;

use common::{
    ensure_fixtures_built, hvac_lib_path, run_agent, run_agent_fail, template_lib_path,
    unique_session,
};

#[test]
fn set_step_get_template_signals() {
    ensure_fixtures_built();
    let session = unique_session("signal-io");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let _ = run_agent(&["--session", &session, "load", &libpath]);
    let _ = run_agent(&["--session", &session, "set", "demo.input", "5.0"]);
    let _ = run_agent(&["--session", &session, "time", "step", "20us"]);
    let out = run_agent(&["--session", &session, "get", "demo.output"]);

    assert!(out.contains("demo.output"));
    assert!(
        out.contains("F32(10") || out.contains("F64(10"),
        "expected scaled output value in get output: {out}"
    );

    let _ = run_agent(&["--session", &session, "close"]);
}

#[test]
fn hvac_writable_and_read_only_signal_behavior() {
    ensure_fixtures_built();
    let session = unique_session("signal-io-hvac");
    let libpath = hvac_lib_path();
    let libpath = libpath
        .to_str()
        .expect("hvac path should be valid utf8")
        .to_string();

    let _ = run_agent(&["--session", &session, "load", &libpath]);
    let _ = run_agent(&["--session", &session, "set", "hvac.power", "true"]);
    let _ = run_agent(&["--session", &session, "set", "hvac.target_temp", "25.0"]);
    let _ = run_agent(&["--session", &session, "time", "step", "20ms"]);
    let state = run_agent(&["--session", &session, "get", "hvac.state"]);
    assert!(state.contains("hvac.state"));

    let err = run_agent_fail(&["--session", &session, "set", "hvac.state", "1"]);
    assert!(
        err.contains("signal not found: 'hvac.state'"),
        "expected read-only write rejection, got: {err}"
    );

    let _ = run_agent(&["--session", &session, "close"]);
}

#[test]
fn glob_selector_reads_matching_signals() {
    ensure_fixtures_built();
    let session = unique_session("signal-io-glob");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let _ = run_agent(&["--session", &session, "load", &libpath]);
    let out = run_agent(&["--session", &session, "get", "demo.*"]);
    assert!(out.contains("demo.input"));
    assert!(out.contains("demo.output"));

    let _ = run_agent(&["--session", &session, "close"]);
}

#[test]
fn wildcard_and_id_selectors_work() {
    ensure_fixtures_built();
    let session = unique_session("signal-io-id");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let _ = run_agent(&["--session", &session, "load", &libpath]);
    let all = run_agent(&["--session", &session, "get", "*"]);
    assert!(all.contains("demo.input"));
    assert!(all.contains("demo.output"));

    let by_id = run_agent(&["--session", &session, "get", "#1"]);
    assert!(by_id.contains("demo.output"));

    let missing = run_agent_fail(&["--session", &session, "get", "#999"]);
    assert!(
        missing.contains("signal not found: '#999'"),
        "expected missing id error, got: {missing}"
    );

    let _ = run_agent(&["--session", &session, "close"]);
}
