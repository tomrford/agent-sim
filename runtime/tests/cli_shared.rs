mod common;

use common::{ensure_fixtures_built, run_agent, template_lib_path, unique_session};

#[test]
fn shared_list_reports_channels_from_loaded_project() {
    ensure_fixtures_built();
    let session = unique_session("shared-list");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let _ = run_agent(&["--session", &session, "load", &libpath]);
    let out = run_agent(&["--session", &session, "shared", "list"]);
    assert!(
        out.contains("Channel"),
        "expected shared channel table output, got: {out}"
    );
    let _ = run_agent(&["--session", &session, "close"]);
}
