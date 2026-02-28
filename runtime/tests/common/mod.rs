use assert_cmd::Command;
use std::path::{Path, PathBuf};
use std::sync::Once;
use uuid::Uuid;

static FIXTURES_BUILT: Once = Once::new();

pub fn unique_session(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("runtime should have workspace parent")
        .to_path_buf()
}

pub fn template_lib_path() -> PathBuf {
    let ext = lib_extension();
    repo_root()
        .join("template")
        .join("zig-out")
        .join("lib")
        .join(format!("libsim_template.{ext}"))
}

pub fn hvac_lib_path() -> PathBuf {
    let ext = lib_extension();
    repo_root()
        .join("examples")
        .join("hvac")
        .join("zig-out")
        .join("lib")
        .join(format!("libsim_hvac_example.{ext}"))
}

pub fn ensure_fixtures_built() {
    FIXTURES_BUILT.call_once(|| {
        run_shell("cd template && zig build");
        run_shell("cd examples/hvac && zig build");
        assert!(
            template_lib_path().exists(),
            "template fixture library should exist after build"
        );
        assert!(
            hvac_lib_path().exists(),
            "hvac fixture library should exist after build"
        );
    });
}

pub fn run_agent(args: &[&str]) -> String {
    let exe = std::env::var("CARGO_BIN_EXE_agent-sim")
        .or_else(|_| std::env::var("CARGO_BIN_EXE_agent_sim"))
        .unwrap_or_else(|_| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("debug")
                .join("agent-sim")
                .display()
                .to_string()
        });
    let output = Command::new(exe)
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8(output).expect("stdout should be valid utf8")
}

fn run_shell(command: &str) {
    let status = std::process::Command::new("bash")
        .arg("-lc")
        .arg(command)
        .current_dir(repo_root())
        .status()
        .expect("fixture build command should run");
    assert!(status.success(), "fixture build command failed: {command}");
}

fn lib_extension() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "dll"
    }
    #[cfg(target_os = "macos")]
    {
        "dylib"
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        "so"
    }
}
