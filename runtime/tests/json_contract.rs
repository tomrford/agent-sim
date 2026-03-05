mod common;

use common::{ensure_fixtures_built, run_agent, template_lib_path, unique_session};

#[test]
fn json_output_contract_and_watch_ndjson() {
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

    let watch = run_agent(&[
        "--json",
        "--instance",
        &session,
        "watch",
        "demo.output",
        "1",
        "--samples",
        "2",
    ]);
    let lines = watch.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);
    for line in lines {
        let row: serde_json::Value =
            serde_json::from_str(line).expect("watch output line should be valid json");
        assert!(row.get("tick").is_some());
        assert!(row.get("name").is_some());
        assert!(row.get("value").is_some());
    }

    let _ = run_agent(&["--instance", &session, "close"]);
}
