mod common;

use common::{ensure_fixtures_built, run_agent, template_lib_path, unique_session};

#[test]
fn set_step_get_template_signals() {
    ensure_fixtures_built();
    let session = unique_session("signal-io");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let _ = run_agent(&["--session", &session, "load", &libpath]);
    let _ = run_agent(&["--session", &session, "set", "demo.input", "5.0"]);
    let _ = run_agent(&["--session", &session, "time", "step", "20us"]);
    let out = run_agent(&["--session", &session, "get", "demo.output"]);

    assert!(out.contains("demo.output"));
    assert!(
        out.contains("F32(10") || out.contains("F64(10"),
        "expected scaled output value in get output: {out}"
    );

    let _ = run_agent(&["--session", &session, "close"]);
}
