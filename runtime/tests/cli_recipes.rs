mod common;

use common::{ensure_fixtures_built, run_agent, template_lib_path, unique_session};
use std::io::Write;

#[test]
fn recipe_run_and_dry_run_work() {
    ensure_fixtures_built();
    let session = unique_session("recipe");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let mut temp = tempfile::NamedTempFile::new().expect("temp config should be creatable");
    write!(
        temp,
        r#"
[recipe.double-input]
description = "Set and tick template"
steps = [
  {{ set = {{ "demo.input" = 3.5 }} }},
  {{ step = "20us" }},
  {{ print = ["demo.output"] }},
]
"#
    )
    .expect("temp config should be writable");
    let config = temp.path().display().to_string();

    let _ = run_agent(&["--instance", &session, "load", &libpath]);

    let dry_run = run_agent(&[
        "--instance",
        &session,
        "--config",
        &config,
        "run",
        "double-input",
        "--dry-run",
    ]);
    assert!(dry_run.contains("Dry run: true"));

    let run = run_agent(&[
        "--instance",
        &session,
        "--config",
        &config,
        "run",
        "double-input",
    ]);
    assert!(run.contains("Steps: 3"));

    let output = run_agent(&["--instance", &session, "get", "demo.output"]);
    assert!(
        output.contains("7"),
        "recipe should produce doubled output, got: {output}"
    );

    let _ = run_agent(&["--instance", &session, "close"]);
}
