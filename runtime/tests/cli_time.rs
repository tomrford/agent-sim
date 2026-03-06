mod common;

use common::{ensure_fixtures_built, run_agent, run_agent_fail, template_lib_path, unique_session};
use std::io::Write;

#[test]
fn time_status_step_speed_commands_work() {
    ensure_fixtures_built();
    let session = unique_session("time");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let _ = run_agent(&["--instance", &session, "load", &libpath]);

    let status_before = run_agent(&["--instance", &session, "time", "status"]);
    assert!(status_before.contains("Paused"));

    let step_out = run_agent(&["--instance", &session, "time", "step", "40us"]);
    assert!(step_out.contains("Advanced: 2 ticks"));

    let speed_out = run_agent(&["--instance", &session, "time", "speed", "2.5"]);
    assert!(speed_out.contains("2.5"));

    let _ = run_agent(&["--instance", &session, "close"]);
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

    let _ = run_agent(&["--instance", &session, "load", &libpath]);
    let _ = run_agent(&["--instance", &session, "time", "start"]);
    let err = run_agent_fail(&["--instance", &session, "time", "step", "20us"]);
    assert!(
        err.contains("step while running is not allowed; pause first"),
        "expected state transition error, got: {err}"
    );
    let _ = run_agent(&["--instance", &session, "time", "pause"]);
    let _ = run_agent(&["--instance", &session, "close"]);
}

#[test]
fn env_time_controls_instances_but_allows_local_status_reads() {
    ensure_fixtures_built();
    let instance_a = unique_session("env-time-a");
    let instance_b = unique_session("env-time-b");
    let env_name = unique_session("env-time");
    let libpath = template_lib_path().to_string_lossy().into_owned();

    let mut cfg = tempfile::NamedTempFile::new().expect("env config should be creatable");
    write!(
        cfg,
        r#"
[env.{env_name}]
instances = [
  {{ name = "{instance_a}", lib = "{libpath}" }},
  {{ name = "{instance_b}", lib = "{libpath}" }},
]
"#
    )
    .expect("env config should be writable");
    let cfg_path = cfg.path().display().to_string();

    let _ = run_agent(&["--config", &cfg_path, "env", "start", &env_name]);
    let _ = run_agent(&["--instance", &instance_a, "set", "demo.input", "3.0"]);
    let _ = run_agent(&["--instance", &instance_b, "set", "demo.input", "5.0"]);

    let status_before = run_agent(&["--instance", &instance_a, "time", "status"]);
    assert!(
        status_before.contains("Ticks: 0"),
        "expected readable local time status before env step, got: {status_before}"
    );

    let err = run_agent_fail(&["--instance", &instance_a, "time", "step", "20us"]);
    assert!(
        err.contains("instance-local time control is unavailable"),
        "expected env-managed time rejection, got: {err}"
    );

    let _ = run_agent(&["env", "time", &env_name, "step", "20us"]);
    let env_status = run_agent(&["env", "time", &env_name, "status"]);
    let status_after = run_agent(&["--instance", &instance_a, "time", "status"]);
    let out_a = run_agent(&["--instance", &instance_a, "get", "demo.output"]);
    let out_b = run_agent(&["--instance", &instance_b, "get", "demo.output"]);
    assert!(
        env_status.contains("Ticks: 1"),
        "expected env time status to reflect the shared step, got: {env_status}"
    );
    assert!(
        status_after.contains("Ticks: 1"),
        "expected local time status to reflect env-managed stepping, got: {status_after}"
    );
    assert!(
        out_a.contains("6"),
        "expected env step to advance instance A, got: {out_a}"
    );
    assert!(
        out_b.contains("10"),
        "expected env step to advance instance B, got: {out_b}"
    );

    let _ = run_agent(&["close", "--env", &env_name]);
}
