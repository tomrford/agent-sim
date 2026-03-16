mod common;

use common::{ensure_fixtures_built, run_agent, run_agent_fail, template_lib_path, unique_session};
use serial_test::serial;
use std::io::Write;

#[test]
#[serial]
fn env_signals_and_get_use_qualified_selectors() {
    ensure_fixtures_built();
    let instance_a = unique_session("env-signals-a");
    let instance_b = unique_session("env-signals-b");
    let env_name = unique_session("env-signals");
    let libpath = template_lib_path().to_string_lossy().replace('\\', "/");

    let mut cfg = tempfile::NamedTempFile::new().expect("env config should be creatable");
    write!(
        cfg,
        r#"
[env.{env_name}]
instances = [
  {{ name = "{instance_a}", lib = "{libpath}" }},
  {{ name = "{instance_b}", lib = "{libpath}" }},
]
"#
    )
    .expect("env config should be writable");
    let cfg_path = cfg.path().display().to_string();

    let _ = run_agent(&["--config", &cfg_path, "env", "start", &env_name]);
    let _ = run_agent(&["--instance", &instance_a, "set", "demo.input", "2.0"]);
    let _ = run_agent(&["--instance", &instance_b, "set", "demo.input", "7.0"]);
    let _ = run_agent(&["env", "time", &env_name, "step", "20us"]);

    let signals = run_agent(&["env", "signals", &env_name]);
    assert!(
        signals.contains(&format!("{instance_a}:demo.input")),
        "missing qualified signal in env signals output: {signals}"
    );
    assert!(
        signals.contains(&format!("{instance_b}:demo.output")),
        "missing qualified signal in env signals output: {signals}"
    );

    let values = run_agent(&[
        "env",
        "get",
        &env_name,
        &format!("{instance_a}:demo.output"),
        &format!("{instance_b}:demo.output"),
    ]);
    assert!(values.contains(&format!("{instance_a}:demo.output")));
    assert!(values.contains(&format!("{instance_b}:demo.output")));

    let invalid = run_agent_fail(&["env", "get", &env_name, &format!("{instance_a}:#1")]);
    assert!(
        invalid.contains("cannot use local signal ids"),
        "unexpected invalid-selector error: {invalid}"
    );

    let _ = run_agent(&["close", "--env", &env_name]);
}

#[test]
#[serial]
fn env_trace_writes_qualified_csv_headers() {
    ensure_fixtures_built();
    let instance = unique_session("env-trace");
    let env_name = unique_session("env-trace");
    let libpath = template_lib_path().to_string_lossy().replace('\\', "/");
    let trace_dir = tempfile::tempdir().expect("trace temp dir should be creatable");
    let trace_path = trace_dir.path().join("env-trace.csv");

    let mut cfg = tempfile::NamedTempFile::new().expect("env config should be creatable");
    write!(
        cfg,
        r#"
[env.{env_name}]
instances = [
  {{ name = "{instance}", lib = "{libpath}" }},
]
"#
    )
    .expect("env config should be writable");
    let cfg_path = cfg.path().display().to_string();

    let _ = run_agent(&["--config", &cfg_path, "env", "start", &env_name]);
    let _ = run_agent(&[
        "env",
        "trace",
        &env_name,
        "start",
        &trace_path.to_string_lossy(),
        "20us",
    ]);
    let _ = run_agent(&["env", "time", &env_name, "step", "40us"]);
    let _ = run_agent(&["env", "trace", &env_name, "stop"]);

    let content = std::fs::read_to_string(&trace_path).expect("env trace csv should be readable");
    let rows = content.lines().collect::<Vec<_>>();
    let header = rows
        .first()
        .copied()
        .expect("env trace csv should contain a header row");
    assert!(
        header.contains(&format!("{instance}:demo.input")),
        "missing qualified header entry: {header}"
    );
    assert!(
        header.contains(&format!("{instance}:demo.output")),
        "missing qualified header entry: {header}"
    );
    let ticks = rows
        .iter()
        .skip(1)
        .take(3)
        .map(|row| {
            row.split(',')
                .next()
                .expect("env trace row should include a tick column")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        ticks,
        vec!["0", "1", "2"],
        "unexpected env trace ticks: {rows:?}"
    );

    let _ = run_agent(&["env", "trace", &env_name, "clear"]);
    assert!(
        !trace_path.exists(),
        "env trace clear should remove output file '{}'",
        trace_path.display()
    );

    let _ = run_agent(&["close", "--env", &env_name]);
}
