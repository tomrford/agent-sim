mod common;

use common::{ensure_fixtures_built, run_agent, run_agent_fail, template_lib_path, unique_session};

#[test]
fn time_status_step_speed_commands_work() {
    ensure_fixtures_built();
    let session = unique_session("time");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let _ = run_agent(&["--session", &session, "load", &libpath]);

    let status_before = run_agent(&["--session", &session, "time", "status"]);
    assert!(status_before.contains("Paused"));

    let step_out = run_agent(&["--session", &session, "time", "step", "40us"]);
    assert!(step_out.contains("Advanced: 2 ticks"));

    let speed_out = run_agent(&["--session", &session, "time", "speed", "2.5"]);
    assert!(speed_out.contains("2.5"));

    let _ = run_agent(&["--session", &session, "close"]);
}

#[test]
fn step_while_running_is_rejected() {
    ensure_fixtures_built();
    let session = unique_session("time-running-step");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let _ = run_agent(&["--session", &session, "load", &libpath]);
    let _ = run_agent(&["--session", &session, "time", "start"]);
    let err = run_agent_fail(&["--session", &session, "time", "step", "20us"]);
    assert!(
        err.contains("step while running is not allowed; pause first"),
        "expected state transition error, got: {err}"
    );
    let _ = run_agent(&["--session", &session, "time", "pause"]);
    let _ = run_agent(&["--session", &session, "close"]);
}
