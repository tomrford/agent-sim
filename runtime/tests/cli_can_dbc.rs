mod common;

use common::{ensure_fixtures_built, run_agent, run_agent_fail, template_lib_path, unique_session};
use std::io::Write;

#[test]
fn can_load_dbc_registers_overlay_signals() {
    ensure_fixtures_built();
    let session = unique_session("can-dbc");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let mut dbc = tempfile::NamedTempFile::new().expect("temp dbc should be creatable");
    write!(
        dbc,
        r#"
VERSION ""
NS_ :
BS_:
BU_: ECU
BO_ 256 TEST: 8 ECU
 SG_ speed : 0|16@1+ (0.1,0) [0|250] "kmh" ECU
"#
    )
    .expect("dbc file should be writable");
    let dbc_path = dbc.path().display().to_string();

    let _ = run_agent(&["--session", &session, "load", &libpath]);
    let out = run_agent(&[
        "--session",
        &session,
        "can",
        "load-dbc",
        "internal",
        &dbc_path,
    ]);
    assert!(
        out.contains("Loaded DBC for internal: 1 signals"),
        "unexpected output: {out}"
    );

    let err = run_agent_fail(&["--session", &session, "get", "can.internal.speed"]);
    assert!(
        err.contains("no frame observed yet"),
        "expected no-frame error, got: {err}"
    );

    let _ = run_agent(&["--session", &session, "close"]);
}
