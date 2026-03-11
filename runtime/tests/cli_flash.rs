mod common;

use common::{
    ensure_fixtures_built, hvac_lib_path, run_agent, run_agent_fail, template_lib_path,
    unique_session,
};

fn flash_base_arg() -> &'static str {
    "0x08000000"
}

fn write_flash_blob(dir: &std::path::Path, bytes: &[u8]) -> std::path::PathBuf {
    let path = dir.join("flash.bin");
    std::fs::write(&path, bytes).expect("flash blob should be writable");
    path
}

#[test]
fn load_flash_flag_preloads_flash_and_reset_preserves_it() {
    ensure_fixtures_built();
    let session = unique_session("flash-flag");
    let libpath = template_lib_path();
    let libpath = libpath.to_string_lossy().into_owned();
    let temp = tempfile::tempdir().expect("tempdir should be creatable");
    let flash = write_flash_blob(temp.path(), &[0x78, 0x56, 0x34, 0x12]);
    let flash_arg = format!("{}:{}", flash.display(), flash_base_arg());

    let _ = run_agent(&[
        "--instance",
        &session,
        "load",
        &libpath,
        "--flash",
        &flash_arg,
    ]);
    let value = run_agent(&["--instance", &session, "get", "demo.flash_value"]);
    assert!(
        value.contains("305419896"),
        "expected flashed u32 value in output, got: {value}"
    );

    let _ = run_agent(&["--instance", &session, "reset"]);
    let after_reset = run_agent(&["--instance", &session, "get", "demo.flash_value"]);
    assert!(
        after_reset.contains("305419896"),
        "flash-backed value should survive reset, got: {after_reset}"
    );

    let _ = run_agent(&["--instance", &session, "close"]);
}

#[test]
fn defaults_load_can_supply_lib_and_flash_relative_to_config_dir() {
    ensure_fixtures_built();
    let session = unique_session("flash-defaults");
    let template_lib = template_lib_path();
    let libname = template_lib
        .file_name()
        .expect("template lib should have a filename")
        .to_string_lossy()
        .to_string();

    let temp = tempfile::tempdir().expect("tempdir should be creatable");
    let config_dir = temp.path().join("cfg");
    let lib_dir = config_dir.join("libs");
    std::fs::create_dir_all(&lib_dir).expect("lib dir should be creatable");
    std::fs::copy(&template_lib, lib_dir.join(&libname)).expect("template lib should copy");
    let flash = write_flash_blob(&config_dir, &[0xEF, 0xBE, 0xAD, 0xDE]);

    let config_path = config_dir.join("agent-sim.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"
[defaults.load]
lib = "./libs/{libname}"
flash = [
  {{ file = "./{}", format = "bin", base = "{}" }},
]
"#,
            flash
                .file_name()
                .expect("flash blob should have filename")
                .to_string_lossy(),
            flash_base_arg(),
        ),
    )
    .expect("config should be writable");

    let _ = run_agent(&[
        "--instance",
        &session,
        "--config",
        &config_path.to_string_lossy(),
        "load",
    ]);
    let value = run_agent(&["--instance", &session, "get", "demo.flash_value"]);
    assert!(
        value.contains("3735928559"),
        "expected flashed defaults value in output, got: {value}"
    );

    let _ = run_agent(&["--instance", &session, "close"]);
}

#[test]
fn env_start_supports_device_flash_blocks() {
    ensure_fixtures_built();
    let session = unique_session("flash-device");
    let env_name = unique_session("flash-env");
    let libpath = template_lib_path();
    let libpath = libpath.to_string_lossy().replace('\\', "/");

    let mut cfg = tempfile::NamedTempFile::new().expect("env config should be creatable");
    std::io::Write::write_all(
        &mut cfg,
        format!(
            r#"
[device.demo]
lib = "{libpath}"
flash = [
  {{ u32 = 3405691582, addr = "0x08000000" }},
]

[env.{env_name}]
instances = [
  {{ name = "{session}", device = "demo" }},
]
"#
        )
        .as_bytes(),
    )
    .expect("env config should be writable");

    let _ = run_agent(&[
        "--config",
        &cfg.path().display().to_string(),
        "env",
        "start",
        &env_name,
    ]);
    let value = run_agent(&["--instance", &session, "get", "demo.flash_value"]);
    assert!(
        value.contains("3405691582"),
        "expected flashed device value in output, got: {value}"
    );

    let _ = run_agent(&["close", "--env", &env_name]);
}

#[test]
fn load_flash_requires_project_flash_export() {
    ensure_fixtures_built();
    let session = unique_session("flash-missing-export");
    let libpath = hvac_lib_path();
    let libpath = libpath.to_string_lossy().into_owned();
    let temp = tempfile::tempdir().expect("tempdir should be creatable");
    let flash = write_flash_blob(temp.path(), &[1, 2, 3, 4]);
    let flash_arg = format!("{}:{}", flash.display(), flash_base_arg());

    let err = run_agent_fail(&[
        "--instance",
        &session,
        "load",
        &libpath,
        "--flash",
        &flash_arg,
    ]);
    assert!(
        err.contains("does not export sim_flash_write"),
        "expected missing flash export error, got: {err}"
    );
}
