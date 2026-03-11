mod common;

use agent_sim::daemon::lifecycle;
use common::{
    ensure_fixtures_built, hvac_lib_path, run_agent, run_agent_fail, template_lib_path,
    unique_session,
};
use std::thread;
use std::time::Duration;

#[test]
fn set_step_get_template_signals() {
    ensure_fixtures_built();
    let session = unique_session("signal-io");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let _ = run_agent(&["--instance", &session, "load", &libpath]);
    let _ = run_agent(&["--instance", &session, "set", "demo.input", "5.0"]);
    let _ = run_agent(&["--instance", &session, "time", "step", "20us"]);
    let out = run_agent(&["--instance", &session, "get", "demo.output"]);

    assert!(out.contains("demo.output"));
    assert!(
        out.contains("F32(10") || out.contains("F64(10"),
        "expected scaled output value in get output: {out}"
    );

    let _ = run_agent(&["--instance", &session, "close"]);
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

    let _ = run_agent(&["--instance", &session, "load", &libpath]);
    let _ = run_agent(&["--instance", &session, "set", "hvac.power", "true"]);
    let _ = run_agent(&["--instance", &session, "set", "hvac.target_temp", "25.0"]);
    let _ = run_agent(&["--instance", &session, "time", "step", "20ms"]);
    let state = run_agent(&["--instance", &session, "get", "hvac.state"]);
    assert!(state.contains("hvac.state"));

    let err = run_agent_fail(&["--instance", &session, "set", "hvac.state", "1"]);
    assert!(
        err.contains("signal not found: 'hvac.state'"),
        "expected read-only write rejection, got: {err}"
    );

    let invalid_mode = run_agent_fail(&["--instance", &session, "set", "hvac.mode", "99"]);
    assert!(
        invalid_mode.contains("invalid argument"),
        "expected invalid mode rejection, got: {invalid_mode}"
    );

    let _ = run_agent(&["--instance", &session, "close"]);
}

#[test]
fn hvac_power_cycle_clears_fault_code() {
    ensure_fixtures_built();
    let session = unique_session("signal-io-hvac-fault");
    let libpath = hvac_lib_path();
    let libpath = libpath
        .to_str()
        .expect("hvac path should be valid utf8")
        .to_string();

    let _ = run_agent(&["--instance", &session, "load", &libpath]);
    let _ = run_agent(&["--instance", &session, "set", "hvac.power", "true"]);
    let _ = run_agent(&["--instance", &session, "set", "hvac.current_temp", "65.0"]);
    let _ = run_agent(&["--instance", &session, "time", "step", "10ms"]);
    let fault = run_agent(&[
        "--instance",
        &session,
        "get",
        "hvac.error_code",
        "hvac.state",
    ]);
    assert!(
        fault.contains("U32(1)"),
        "expected fault code 1, got: {fault}"
    );
    assert!(
        fault.contains("U32(5)"),
        "expected fault state before power cycle, got: {fault}"
    );

    let _ = run_agent(&["--instance", &session, "set", "hvac.power", "false"]);
    let _ = run_agent(&["--instance", &session, "time", "step", "10ms"]);
    let cleared = run_agent(&[
        "--instance",
        &session,
        "get",
        "hvac.error_code",
        "hvac.state",
    ]);
    assert!(
        cleared.contains("U32(0)"),
        "expected cleared outputs, got: {cleared}"
    );

    let _ = run_agent(&["--instance", &session, "close"]);
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

    let _ = run_agent(&["--instance", &session, "load", &libpath]);
    let out = run_agent(&["--instance", &session, "get", "demo.*"]);
    assert!(out.contains("demo.input"));
    assert!(out.contains("demo.output"));

    let _ = run_agent(&["--instance", &session, "close"]);
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

    let _ = run_agent(&["--instance", &session, "load", &libpath]);
    let all = run_agent(&["--instance", &session, "get", "*"]);
    assert!(all.contains("demo.input"));
    assert!(all.contains("demo.output"));

    let by_id = run_agent(&["--instance", &session, "get", "#1"]);
    assert!(by_id.contains("demo.output"));

    let missing = run_agent_fail(&["--instance", &session, "get", "#999"]);
    assert!(
        missing.contains("signal not found: '#999'"),
        "expected missing id error, got: {missing}"
    );

    let _ = run_agent(&["--instance", &session, "close"]);
}

#[test]
fn close_is_idempotent_after_shutdown() {
    ensure_fixtures_built();
    let session = unique_session("signal-io-close");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let _ = run_agent(&["--instance", &session, "load", &libpath]);
    let _ = run_agent(&["--instance", &session, "close"]);

    let socket_path = lifecycle::socket_path(&session);
    for _ in 0..50 {
        if !socket_path.exists() {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        !socket_path.exists(),
        "instance socket should be removed after close: {}",
        socket_path.display()
    );

    let _ = run_agent(&["--instance", &session, "close"]);
}
