mod common;

use common::{ensure_fixtures_built, run_agent, run_agent_fail, template_lib_path, unique_session};

#[test]
fn can_buses_lists_declared_template_buses() {
    ensure_fixtures_built();
    let session = unique_session("can-buses");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let _ = run_agent(&["--session", &session, "load", &libpath]);
    let buses = run_agent(&["--session", &session, "can", "buses"]);
    assert!(buses.contains("internal"), "expected internal bus: {buses}");
    assert!(buses.contains("external"), "expected external bus: {buses}");
    let _ = run_agent(&["--session", &session, "close"]);
}

#[test]
fn can_send_requires_bus_attachment() {
    ensure_fixtures_built();
    let session = unique_session("can-send");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let _ = run_agent(&["--session", &session, "load", &libpath]);
    let err = run_agent_fail(&[
        "--session",
        &session,
        "can",
        "send",
        "internal",
        "0x123",
        "0102",
    ]);
    assert!(
        err.contains("is not attached"),
        "expected unattached bus error, got: {err}"
    );
    let _ = run_agent(&["--session", &session, "close"]);
}
