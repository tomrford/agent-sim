mod common;

use common::{ensure_fixtures_built, run_agent, run_agent_fail, template_lib_path, unique_session};

#[test]
fn can_signal_projection_is_rejected() {
    ensure_fixtures_built();
    let session = unique_session("can-projection");
    let libpath = template_lib_path().to_string_lossy().into_owned();

    let _ = run_agent(&["--instance", &session, "load", &libpath]);
    let err = run_agent_fail(&["--instance", &session, "get", "can.internal.speed"]);
    assert!(
        err.contains("CAN signal projection is no longer supported"),
        "expected CAN projection rejection, got: {err}"
    );
    let _ = run_agent(&["--instance", &session, "close"]);
}

#[test]
fn can_signal_writes_are_rejected() {
    ensure_fixtures_built();
    let session = unique_session("can-write-projection");
    let libpath = template_lib_path().to_string_lossy().into_owned();

    let _ = run_agent(&["--instance", &session, "load", &libpath]);
    let err = run_agent_fail(&["--instance", &session, "set", "can.internal.speed=12.0"]);
    assert!(
        err.contains("CAN signal projection is no longer supported"),
        "expected CAN projection write rejection, got: {err}"
    );
    let _ = run_agent(&["--instance", &session, "close"]);
}
