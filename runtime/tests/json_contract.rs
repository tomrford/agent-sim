mod common;

use common::{ensure_fixtures_built, run_agent, template_lib_path, unique_session};

#[test]
fn json_output_contract_and_trace_status() {
    ensure_fixtures_built();
    let session = unique_session("json");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let load_json = run_agent(&["--json", "--instance", &session, "load", &libpath]);
    let load_value: serde_json::Value =
        serde_json::from_str(load_json.trim()).expect("load output should be valid json object");
    assert_eq!(load_value["success"], serde_json::Value::Bool(true));

    let trace_dir = tempfile::tempdir().expect("trace temp dir should be creatable");
    let trace_path = trace_dir.path().join("trace.csv");
    let _ = run_agent(&[
        "--instance",
        &session,
        "trace",
        "start",
        &trace_path.to_string_lossy(),
        "20us",
    ]);
    let _ = run_agent(&["--instance", &session, "time", "step", "40us"]);
    let _ = run_agent(&["--instance", &session, "trace", "stop"]);

    let status_json = run_agent(&["--json", "--instance", &session, "trace", "status"]);
    let status: serde_json::Value =
        serde_json::from_str(status_json.trim()).expect("trace status should be valid json");
    assert_eq!(status["success"], serde_json::Value::Bool(true));
    assert_eq!(status["data"]["kind"], serde_json::Value::String("trace_status".to_string()));
    assert_eq!(
        status["data"]["value"]["active"],
        serde_json::Value::Bool(false)
    );
    assert_eq!(
        status["data"]["value"]["signal_count"],
        serde_json::Value::Number(3.into())
    );
    assert!(
        trace_path.exists(),
        "trace output should exist at {}",
        trace_path.display()
    );

    let _ = run_agent(&["--instance", &session, "close"]);
}
