mod common;

use common::{ensure_fixtures_built, run_agent, template_lib_path, unique_session};

#[test]
fn trace_lifecycle_writes_csv_and_clears_file() {
    ensure_fixtures_built();
    let session = unique_session("trace");
    let libpath = template_lib_path().to_string_lossy().into_owned();
    let trace_dir = tempfile::tempdir().expect("trace temp dir should be creatable");
    let trace_path = trace_dir.path().join("instance-trace.csv");

    let _ = run_agent(&["--instance", &session, "load", &libpath]);
    let _ = run_agent(&[
        "--instance",
        &session,
        "trace",
        "start",
        &trace_path.to_string_lossy(),
        "20us",
    ]);
    let _ = run_agent(&["--instance", &session, "time", "step", "60us"]);
    let _ = run_agent(&["--instance", &session, "trace", "stop"]);

    let status = run_agent(&["--instance", &session, "trace", "status"]);
    assert!(status.contains("Active: false"), "unexpected status: {status}");
    assert!(status.contains("Signals: 3"), "unexpected status: {status}");

    let content =
        std::fs::read_to_string(&trace_path).expect("instance trace output should be readable");
    let rows = content.lines().collect::<Vec<_>>();
    assert_eq!(
        rows.first().copied(),
        Some("tick,time_us,demo.input,demo.output,demo.flash_value")
    );
    assert!(
        rows.len() >= 5,
        "expected header plus sampled rows, got {} rows:\n{content}",
        rows.len()
    );

    let _ = run_agent(&["--instance", &session, "trace", "clear"]);
    assert!(
        !trace_path.exists(),
        "trace clear should remove output file '{}'",
        trace_path.display()
    );

    let _ = run_agent(&["--instance", &session, "close"]);
}
