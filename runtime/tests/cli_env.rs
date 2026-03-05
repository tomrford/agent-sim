mod common;

use common::{ensure_fixtures_built, run_agent, run_agent_fail, template_lib_path, unique_session};
use std::io::Write;

#[test]
fn env_start_and_close_by_env_tag() {
    ensure_fixtures_built();
    let session_a = unique_session("env-a");
    let session_b = unique_session("env-b");
    let env_name = unique_session("cluster");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let mut cfg = tempfile::NamedTempFile::new().expect("env config should be creatable");
    write!(
        cfg,
        r#"
[env.{env_name}]
sessions = [
  {{ name = "{session_a}", lib = "{libpath}" }},
  {{ name = "{session_b}", lib = "{libpath}" }},
]
"#
    )
    .expect("env config should be writable");
    let cfg_path = cfg.path().display().to_string();

    let _ = run_agent(&["--config", &cfg_path, "env", "start", &env_name]);
    let status_a = run_agent(&["--session", &session_a, "session"]);
    assert!(
        status_a.contains("Running: true"),
        "unexpected session output: {status_a}"
    );
    assert!(
        status_a.contains(&env_name),
        "expected env tag in status: {status_a}"
    );

    let _ = run_agent(&["close", "--env", &env_name]);
    let err = run_agent_fail(&["--session", &session_a, "info"]);
    assert!(
        err.contains("run `agent-sim load <libpath>` first"),
        "expected stopped session after close --env, got: {err}"
    );
}

#[test]
fn close_all_closes_every_running_session() {
    ensure_fixtures_built();
    let session_a = unique_session("close-all-a");
    let session_b = unique_session("close-all-b");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let _ = run_agent(&["--session", &session_a, "load", &libpath]);
    let _ = run_agent(&["--session", &session_b, "load", &libpath]);

    let _ = run_agent(&["close", "--all"]);

    let err_a = run_agent_fail(&["--session", &session_a, "info"]);
    let err_b = run_agent_fail(&["--session", &session_b, "info"]);
    assert!(err_a.contains("run `agent-sim load <libpath>` first"));
    assert!(err_b.contains("run `agent-sim load <libpath>` first"));
}
