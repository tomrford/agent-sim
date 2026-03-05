mod common;

use common::{ensure_fixtures_built, run_agent, template_lib_path, unique_session};
use std::io::Write;

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

#[test]
fn shared_get_reads_latest_snapshot_from_writer_session() {
    ensure_fixtures_built();
    let writer_session = unique_session("shared-writer");
    let reader_session = unique_session("shared-reader");
    let env_name = unique_session("shared-env");
    let libpath = template_lib_path();
    let libpath = libpath
        .to_str()
        .expect("template path should be valid utf8")
        .to_string();

    let mut cfg = tempfile::NamedTempFile::new().expect("shared env config should be creatable");
    write!(
        cfg,
        r#"
[env.{env_name}]
sessions = [
  {{ name = "{writer_session}", lib = "{libpath}" }},
  {{ name = "{reader_session}", lib = "{libpath}" }},
]
[env.{env_name}.shared.sensor_feed]
members = ["{writer_session}:sensor_feed", "{reader_session}:sensor_feed"]
writer = "{writer_session}"
"#
    )
    .expect("shared env config should be writable");
    let cfg_path = cfg.path().display().to_string();

    let _ = run_agent(&["--config", &cfg_path, "env", "start", &env_name]);
    let _ = run_agent(&["--session", &writer_session, "set", "demo.input", "3.0"]);
    let _ = run_agent(&["--session", &writer_session, "time", "step", "20us"]);

    let shared = run_agent(&[
        "--session",
        &reader_session,
        "shared",
        "get",
        "sensor_feed.*",
    ]);
    assert!(
        shared.contains("Slot"),
        "expected slot table output from shared get, got: {shared}"
    );
    assert!(
        shared.contains("slot") || shared.contains("Slot"),
        "expected slot rows from shared get, got: {shared}"
    );

    let _ = run_agent(&["close", "--env", &env_name]);
}
