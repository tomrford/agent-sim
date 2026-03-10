mod common;

use common::{
    ensure_fixtures_built, hvac_lib_path, run_agent, run_agent_fail, template_lib_path,
    unique_session,
};

#[test]
fn load_info_and_reset_workflow() {
    ensure_fixtures_built();
    let session = unique_session("project-singleton");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let load_out = run_agent(&["--instance", &session, "load", &libpath]);
    assert!(load_out.contains("Loaded:"));

    let info_out = run_agent(&["--instance", &session, "info"]);
    assert!(info_out.contains("Loaded: true"));
    assert!(info_out.contains("Signals: 3"));

    let _ = run_agent(&["--instance", &session, "set", "demo.input", "4.0"]);
    let _ = run_agent(&["--instance", &session, "time", "step", "20us"]);
    let value_before_reset = run_agent(&["--instance", &session, "get", "demo.output"]);
    assert!(value_before_reset.contains("8"));

    let _ = run_agent(&["--instance", &session, "reset"]);
    let value_after_reset = run_agent(&["--instance", &session, "get", "demo.output"]);
    assert!(value_after_reset.contains("0"));

    let _ = run_agent(&["--instance", &session, "close"]);
}

#[test]
fn load_invalid_library_path_returns_error() {
    let session = unique_session("project-invalid-path");
    let missing = std::env::temp_dir().join("agent-sim-missing-lib");
    let missing =
        missing.with_extension(common::template_lib_path().extension().unwrap_or_default());
    let err = run_agent_fail(&["--instance", &session, "load", &missing.to_string_lossy()]);
    assert!(
        err.contains("failed to resolve shared library path"),
        "expected load error for invalid library path, got: {err}"
    );
}

#[test]
fn second_load_same_session_is_rejected() {
    ensure_fixtures_built();
    let session = unique_session("project-singleton-reload");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let _ = run_agent(&["--instance", &session, "load", &libpath]);
    let err = run_agent_fail(&["--instance", &session, "load", &libpath]);
    assert!(
        err.contains("already has a running daemon"),
        "expected singleton daemon load rejection, got: {err}"
    );

    let _ = run_agent(&["--instance", &session, "close"]);
}

#[test]
fn hvac_signal_catalog_contains_expected_names() {
    ensure_fixtures_built();
    let session = unique_session("project-hvac-signals");
    let libpath = hvac_lib_path();
    let libpath = libpath
        .to_str()
        .expect("hvac path should be valid utf8")
        .to_string();

    let _ = run_agent(&["--instance", &session, "load", &libpath]);
    let signals = run_agent(&["--instance", &session, "signals"]);
    assert!(signals.contains("hvac.power"));
    assert!(signals.contains("hvac.state"));
    assert!(signals.contains("hvac.current_temp"));

    let _ = run_agent(&["--instance", &session, "close"]);
}
