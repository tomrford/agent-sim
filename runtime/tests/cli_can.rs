mod common;

use common::{ensure_fixtures_built, run_agent, run_agent_fail, template_lib_path, unique_session};
use std::io::Write;

#[test]
fn standalone_instance_can_buses_are_available() {
    ensure_fixtures_built();
    let session = unique_session("can-standalone");
    let libpath = template_lib_path();
    let libpath = libpath.to_string_lossy().into_owned();

    let _ = run_agent(&["--instance", &session, "load", &libpath]);
    let buses = run_agent(&["--instance", &session, "can", "buses"]);
    assert!(
        buses.contains("internal") && buses.contains("external"),
        "expected standalone CAN bus output, got: {buses}"
    );
    let _ = run_agent(&["--instance", &session, "close"]);
}

#[test]
fn env_can_buses_reports_env_owned_topology() {
    ensure_fixtures_built();
    let session = unique_session("env-can");
    let env_name = unique_session("env-can");
    let libpath = template_lib_path().to_string_lossy().replace('\\', "/");

    let mut cfg = tempfile::NamedTempFile::new().expect("env config should be creatable");
    write!(
        cfg,
        r#"
[env.{env_name}]
instances = [
  {{ name = "{session}", lib = "{libpath}" }},
]
"#
    )
    .expect("env config should be writable");

    let _ = run_agent(&[
        "--config",
        &cfg.path().display().to_string(),
        "env",
        "start",
        &env_name,
    ]);
    let buses = run_agent(&["env", "can", &env_name, "buses"]);
    assert!(
        buses.contains("Bus") || buses.contains("ID"),
        "expected env CAN buses output, got: {buses}"
    );
    let err = run_agent_fail(&["--instance", &session, "can", "buses"]);
    assert!(
        err.contains("CAN is env-owned"),
        "expected env-owned CAN rejection for attached instance, got: {err}"
    );
    let _ = run_agent(&["close", "--env", &env_name]);
}
