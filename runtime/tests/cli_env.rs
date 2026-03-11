mod common;

use common::{
    ensure_fixtures_built, run_agent, run_agent_capture_in_home, run_agent_fail, template_lib_path,
    unique_session,
};
use serial_test::serial;
use std::io::Write;

#[test]
#[serial]
fn env_start_and_close_by_env_tag() {
    ensure_fixtures_built();
    let session_a = unique_session("env-a");
    let session_b = unique_session("env-b");
    let env_name = unique_session("cluster");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .replace('\\', "/");

    let mut cfg = tempfile::NamedTempFile::new().expect("env config should be creatable");
    write!(
        cfg,
        r#"
[env.{env_name}]
instances = [
  {{ name = "{session_a}", lib = "{libpath}" }},
  {{ name = "{session_b}", lib = "{libpath}" }},
]
"#
    )
    .expect("env config should be writable");
    let cfg_path = cfg.path().display().to_string();

    let _ = run_agent(&["--config", &cfg_path, "env", "start", &env_name]);
    let status_a = run_agent(&["--instance", &session_a, "instance"]);
    assert!(
        status_a.contains("Running: true"),
        "unexpected instance output: {status_a}"
    );
    assert!(
        status_a.contains(&env_name),
        "expected env tag in status: {status_a}"
    );

    let _ = run_agent(&["close", "--env", &env_name]);
    let err = run_agent_fail(&["--instance", &session_a, "info"]);
    assert!(
        err.contains("run `agent-sim load <libpath>` first"),
        "expected stopped instance after close --env, got: {err}"
    );
}

#[test]
#[serial]
fn close_all_closes_every_running_session() {
    ensure_fixtures_built();
    let session_a = unique_session("close-all-a");
    let session_b = unique_session("close-all-b");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let _ = run_agent(&["--instance", &session_a, "load", &libpath]);
    let _ = run_agent(&["--instance", &session_b, "load", &libpath]);

    let _ = run_agent(&["close", "--all"]);

    let err_a = run_agent_fail(&["--instance", &session_a, "info"]);
    let err_b = run_agent_fail(&["--instance", &session_b, "info"]);
    assert!(err_a.contains("run `agent-sim load <libpath>` first"));
    assert!(err_b.contains("run `agent-sim load <libpath>` first"));
}

#[test]
#[serial]
fn close_all_closes_running_env_daemons_too() {
    ensure_fixtures_built();
    let session = unique_session("close-all-env");
    let env_name = unique_session("close-all-env");
    let libpath = template_lib_path().to_string_lossy().into_owned();
    let libpath = libpath.replace('\\', "/");

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
    let _ = run_agent(&["close", "--all"]);

    let env_err = run_agent_fail(&["env", "status", &env_name]);
    assert!(
        env_err.contains("has no running daemon"),
        "expected env daemon to stop after close --all, got: {env_err}"
    );
    let instance_err = run_agent_fail(&["--instance", &session, "info"]);
    assert!(
        instance_err.contains("run `agent-sim load <libpath>` first"),
        "expected env instance to stop after close --all, got: {instance_err}"
    );
}

#[test]
#[serial]
fn env_start_rejects_unknown_session_fields() {
    ensure_fixtures_built();
    let session = unique_session("env-init");
    let env_name = unique_session("cluster-init");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .replace('\\', "/");

    let mut cfg = tempfile::NamedTempFile::new().expect("env config should be creatable");
    write!(
        cfg,
        r#"
[env.{env_name}]
instances = [
  {{ name = "{session}", lib = "{libpath}", init = {{ "demo.input" = 4.5 }} }},
]
"#
    )
    .expect("env config should be writable");
    let cfg_path = cfg.path().display().to_string();

    let err = run_agent_fail(&["--config", &cfg_path, "env", "start", &env_name]);
    assert!(err.contains("unknown field"), "unexpected error: {err}");
    assert!(err.contains("init"), "unexpected error: {err}");
}

#[test]
#[serial]
fn env_start_resolves_session_lib_relative_to_config_dir() {
    ensure_fixtures_built();
    let session = unique_session("env-relative-lib");
    let env_name = unique_session("cluster-relative-lib");
    let libpath = template_lib_path();
    let libname = libpath
        .file_name()
        .expect("template library should have a filename")
        .to_string_lossy()
        .to_string();

    let temp = tempfile::tempdir().expect("tempdir should be creatable");
    let config_dir = temp.path().join("cfg");
    let lib_dir = config_dir.join("libs");
    std::fs::create_dir_all(&lib_dir).expect("lib dir should be creatable");
    std::fs::copy(&libpath, lib_dir.join(&libname)).expect("fixture library should copy");

    let cfg_path = config_dir.join("agent-sim.toml");
    std::fs::write(
        &cfg_path,
        format!(
            r#"
[env.{env_name}]
instances = [
  {{ name = "{session}", lib = "./libs/{libname}" }},
]
"#
        ),
    )
    .expect("env config should be writable");
    let cfg_path = cfg_path.display().to_string();

    let _ = run_agent(&["--config", &cfg_path, "env", "start", &env_name]);
    let status = run_agent(&["--instance", &session, "instance"]);
    assert!(
        status.contains("Running: true"),
        "unexpected instance output: {status}"
    );

    let _ = run_agent(&["close", "--env", &env_name]);
}

#[test]
#[serial]
fn env_start_cleans_up_instances_when_env_socket_bind_fails() {
    ensure_fixtures_built();
    let home = tempfile::tempdir().expect("temp home should be creatable");
    let session = unique_session("env-bind-cleanup");
    let env_name = unique_session("cluster-bind-cleanup");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .replace('\\', "/");

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
    let cfg_path = cfg.path().display().to_string();

    let occupied_socket_path = home.path().join("envs").join(format!("{env_name}.sock"));
    std::fs::create_dir_all(&occupied_socket_path)
        .expect("occupied socket path should be creatable");

    let err = common::run_agent_fail_in_home(
        home.path(),
        &["--config", &cfg_path, "env", "start", &env_name],
    );
    assert!(
        err.contains("env daemon startup failed"),
        "unexpected env start error: {err}"
    );

    let status_err = common::run_agent_fail_in_home(home.path(), &["--instance", &session, "info"]);
    assert!(
        status_err.contains("run `agent-sim load <libpath>` first"),
        "expected instance cleanup after env bind failure, got: {status_err}"
    );
    assert!(
        !home.path().join(format!("{session}.pid")).exists(),
        "instance pid should be removed after env bind failure"
    );
    assert!(
        !home
            .path()
            .join("envs")
            .join(format!("{env_name}.pid"))
            .exists(),
        "env pid should not be left behind after bind failure"
    );
}

#[test]
#[serial]
fn concurrent_env_start_keeps_env_reachable() {
    ensure_fixtures_built();
    let home = tempfile::tempdir().expect("temp home should be creatable");
    let session = unique_session("env-concurrent");
    let env_name = unique_session("cluster-concurrent");
    let libpath = template_lib_path()
        .to_string_lossy()
        .replace('\\', "/")
        .to_string();

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
    let cfg_path = cfg.path().display().to_string();

    let (first, second) = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            run_agent_capture_in_home(
                home.path(),
                &["--config", &cfg_path, "env", "start", &env_name],
            )
        });
        let second = scope.spawn(|| {
            run_agent_capture_in_home(
                home.path(),
                &["--config", &cfg_path, "env", "start", &env_name],
            )
        });
        (
            first.join().expect("first env-start thread should join"),
            second.join().expect("second env-start thread should join"),
        )
    });

    let successes = [first.0, second.0]
        .into_iter()
        .filter(|success| *success)
        .count();
    assert_eq!(successes, 1, "expected exactly one successful env start");
    let combined_stderr = format!("{}\n{}", first.2, second.2);
    assert!(
        combined_stderr.contains("already has a running daemon"),
        "expected duplicate env start rejection, got: {combined_stderr}"
    );

    let status = run_agent_capture_in_home(home.path(), &["env", "status", &env_name]);
    assert!(
        status.0,
        "env should remain reachable after concurrent start"
    );

    let close = run_agent_capture_in_home(home.path(), &["close", "--env", &env_name]);
    assert!(close.0, "env close should succeed after concurrent start");
}
