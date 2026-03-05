mod common;

use common::{ensure_fixtures_built, run_agent, run_agent_fail, template_lib_path, unique_session};
use std::io::Write;

#[test]
fn recipe_level_session_default_targets_specified_session() {
    ensure_fixtures_built();
    let session_a = unique_session("recipe-a");
    let session_b = unique_session("recipe-b");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let _ = run_agent(&["--session", &session_a, "load", &libpath]);
    let _ = run_agent(&["--session", &session_b, "load", &libpath]);

    let mut cfg = tempfile::NamedTempFile::new().expect("recipe config should be creatable");
    write!(
        cfg,
        r#"
[recipe.to-b]
session = "{session_b}"
steps = [
  {{ set = {{ "demo.input" = 4.0 }} }},
  {{ step = "20us" }},
]
"#
    )
    .expect("recipe config should be writable");
    let cfg_path = cfg.path().display().to_string();

    let _ = run_agent(&[
        "--session",
        &session_a,
        "--config",
        &cfg_path,
        "run",
        "to-b",
    ]);

    let out_b = run_agent(&["--session", &session_b, "get", "demo.output"]);
    assert!(
        out_b.contains("8"),
        "expected session B to be updated, got: {out_b}"
    );

    let out_a = run_agent(&["--session", &session_a, "get", "demo.output"]);
    assert!(
        out_a.contains("0"),
        "expected session A to remain unchanged, got: {out_a}"
    );

    let _ = run_agent(&["--session", &session_a, "close"]);
    let _ = run_agent(&["--session", &session_b, "close"]);
}

#[test]
fn recipe_session_preconditions_must_be_running() {
    ensure_fixtures_built();
    let session_a = unique_session("recipe-precond-a");
    let session_missing = unique_session("recipe-precond-missing");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let _ = run_agent(&["--session", &session_a, "load", &libpath]);

    let mut cfg = tempfile::NamedTempFile::new().expect("recipe config should be creatable");
    write!(
        cfg,
        r#"
[recipe.requires]
sessions = ["{session_missing}"]
steps = [{{ step = "20us" }}]
"#
    )
    .expect("recipe config should be writable");
    let cfg_path = cfg.path().display().to_string();

    let err = run_agent_fail(&[
        "--session",
        &session_a,
        "--config",
        &cfg_path,
        "run",
        "requires",
    ]);
    assert!(
        err.contains(&session_missing),
        "expected missing-session precondition error, got: {err}"
    );

    let _ = run_agent(&["--session", &session_a, "close"]);
}

#[test]
fn recipe_env_whitelist_requires_target_session_env() {
    ensure_fixtures_built();
    let session = unique_session("recipe-env-session");
    let env_name = unique_session("recipe-env");
    let other_env = unique_session("recipe-other-env");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let mut cfg = tempfile::NamedTempFile::new().expect("recipe config should be creatable");
    write!(
        cfg,
        r#"
[env.{env_name}]
sessions = [
  {{ name = "{session}", lib = "{libpath}" }},
]

[recipe.allowed]
env = ["{env_name}"]
steps = [{{ step = "20us" }}]

[recipe.blocked]
env = ["{other_env}"]
steps = [{{ step = "20us" }}]
"#
    )
    .expect("recipe config should be writable");
    let cfg_path = cfg.path().display().to_string();

    let _ = run_agent(&["--config", &cfg_path, "env", "start", &env_name]);
    let _ = run_agent(&[
        "--session",
        &session,
        "--config",
        &cfg_path,
        "run",
        "allowed",
    ]);

    let err = run_agent_fail(&[
        "--session",
        &session,
        "--config",
        &cfg_path,
        "run",
        "blocked",
    ]);
    assert!(
        err.contains("only allowed in envs"),
        "unexpected error: {err}"
    );
    assert!(err.contains(&other_env), "unexpected error: {err}");

    let _ = run_agent(&["close", "--env", &env_name]);
}

#[test]
fn recipe_env_whitelist_rejects_sessions_without_env() {
    ensure_fixtures_built();
    let session = unique_session("recipe-no-env");
    let env_name = unique_session("recipe-env");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let _ = run_agent(&["--session", &session, "load", &libpath]);

    let mut cfg = tempfile::NamedTempFile::new().expect("recipe config should be creatable");
    write!(
        cfg,
        r#"
[recipe.allowed]
env = ["{env_name}"]
steps = [{{ step = "20us" }}]
"#
    )
    .expect("recipe config should be writable");
    let cfg_path = cfg.path().display().to_string();

    let err = run_agent_fail(&[
        "--session",
        &session,
        "--config",
        &cfg_path,
        "run",
        "allowed",
    ]);
    assert!(
        err.contains("is not attached to any env"),
        "unexpected error: {err}"
    );

    let _ = run_agent(&["--session", &session, "close"]);
}
