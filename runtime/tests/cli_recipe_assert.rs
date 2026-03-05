mod common;

use common::{ensure_fixtures_built, run_agent, run_agent_fail, template_lib_path, unique_session};
use std::io::Write;

#[test]
fn recipe_assertion_steps_pass_and_fail() {
    ensure_fixtures_built();
    let session = unique_session("recipe-assert");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let mut temp = tempfile::NamedTempFile::new().expect("temp config should be creatable");
    write!(
        temp,
        r#"
[recipe.pass]
steps = [
  {{ set = {{ "demo.input" = 3.0 }} }},
  {{ step = "20us" }},
  {{ assert = {{ signal = "demo.output", gt = 5.0 }} }},
  {{ assert = {{ signal = "demo.output", approx = 6.0, tolerance = 0.001 }} }},
]

[recipe.fail]
steps = [
  {{ set = {{ "demo.input" = 3.0 }} }},
  {{ step = "20us" }},
  {{ assert = {{ signal = "demo.output", lt = 5.0 }} }},
]
"#
    )
    .expect("temp config should be writable");
    let config = temp.path().display().to_string();

    let _ = run_agent(&["--instance", &session, "load", &libpath]);

    let pass = run_agent(&["--instance", &session, "--config", &config, "run", "pass"]);
    assert!(pass.contains("Steps: 4"), "unexpected pass output: {pass}");

    let fail = run_agent_fail(&["--instance", &session, "--config", &config, "run", "fail"]);
    assert!(
        fail.contains("assertion failed"),
        "expected assertion failure output, got: {fail}"
    );
    assert!(
        fail.contains("demo.output"),
        "expected signal name in assertion failure, got: {fail}"
    );

    let _ = run_agent(&["--instance", &session, "close"]);
}

#[test]
fn recipe_approx_requires_explicit_tolerance() {
    ensure_fixtures_built();
    let session = unique_session("recipe-assert-tolerance");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let mut temp = tempfile::NamedTempFile::new().expect("temp config should be creatable");
    write!(
        temp,
        r#"
[recipe.invalid]
steps = [
  {{ assert = {{ signal = "demo.output", approx = 6.0 }} }},
]
"#
    )
    .expect("temp config should be writable");
    let config = temp.path().display().to_string();

    let _ = run_agent(&["--instance", &session, "load", &libpath]);
    let fail = run_agent_fail(&[
        "--instance",
        &session,
        "--config",
        &config,
        "run",
        "invalid",
    ]);
    assert!(
        fail.contains("explicit tolerance"),
        "expected explicit tolerance failure, got: {fail}"
    );

    let _ = run_agent(&["--instance", &session, "close"]);
}
