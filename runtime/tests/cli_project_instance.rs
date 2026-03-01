mod common;

use common::{
    ensure_fixtures_built, hvac_lib_path, run_agent, run_agent_fail, template_lib_path,
    unique_session,
};

#[test]
fn load_info_and_instance_workflow() {
    ensure_fixtures_built();
    let session = unique_session("project-instance");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let load_out = run_agent(&["--session", &session, "load", &libpath]);
    assert!(load_out.contains("Loaded:"));

    let info_out = run_agent(&["--session", &session, "info"]);
    assert!(info_out.contains("Loaded: true"));
    assert!(info_out.contains("Signals: 2"));
    assert!(info_out.contains("Instances: 1"));

    let _new_out = run_agent(&["--session", &session, "instance", "new"]);
    let list_out = run_agent(&["--session", &session, "instance", "list"]);
    assert!(list_out.contains("0"));
    assert!(list_out.contains("1"));

    let select_out = run_agent(&["--session", &session, "instance", "select", "1"]);
    assert!(select_out.contains("Active instance: 1"));

    let free_out = run_agent(&["--session", &session, "instance", "free", "1"]);
    assert!(free_out.contains("0"));

    let _ = run_agent(&["--session", &session, "close"]);
}

#[test]
fn load_invalid_library_path_returns_error() {
    let session = unique_session("project-invalid-path");
    let err = run_agent_fail(&[
        "--session",
        &session,
        "load",
        "/tmp/this-library-does-not-exist.so",
    ]);
    assert!(
        err.contains("library load failed"),
        "expected load error for invalid library path, got: {err}"
    );
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

    let _ = run_agent(&["--session", &session, "load", &libpath]);
    let signals = run_agent(&["--session", &session, "signals"]);
    assert!(signals.contains("hvac.power"));
    assert!(signals.contains("hvac.state"));
    assert!(signals.contains("hvac.current_temp"));

    let _ = run_agent(&["--session", &session, "close"]);
}
