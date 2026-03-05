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
    let exe = resolve_agent_exe();
    let output = Command::new(exe)
        .env("AGENT_SIM_HOME", test_agent_sim_home())
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8(output).expect("stdout should be valid utf8")
}

#[allow(dead_code)]
pub fn run_agent_fail(args: &[&str]) -> String {
    let exe = resolve_agent_exe();
    let output = Command::new(exe)
        .env("AGENT_SIM_HOME", test_agent_sim_home())
        .args(args)
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    String::from_utf8(output).expect("stderr should be valid utf8")
}

fn run_shell(command: &str) {
    let output = std::process::Command::new("bash")
        .arg("-c")
        .arg(command)
        .current_dir(repo_root())
        .output()
        .expect("fixture build command should run");
    assert!(
        output.status.success(),
        "fixture build command failed: {command}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn test_agent_sim_home() -> PathBuf {
    let test_bin = std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_stem()
                .map(|value| value.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "agent-sim-tests".to_string());
    let short = test_bin.chars().take(12).collect::<String>();
    let path = PathBuf::from(format!("/tmp/asim-{short}"));
    let _ = std::fs::create_dir_all(&path);
    path
}

fn resolve_agent_exe() -> String {
    if let Ok(exe) = std::env::var("CARGO_BIN_EXE_agent-sim") {
        return exe;
    }
    if let Ok(exe) = std::env::var("CARGO_BIN_EXE_agent_sim") {
        return exe;
    }

    if let Ok(current_exe) = std::env::current_exe()
        && let Some(profile_dir) = current_exe.parent().and_then(|p| p.parent())
    {
        let direct = profile_dir.join(bin_name());
        if direct.exists() {
            return direct.display().to_string();
        }
    }

    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target"));
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());

    let mut candidates = Vec::new();
    if let Ok(target) = std::env::var("TARGET") {
        candidates.push(target_dir.join(&target).join(&profile).join(bin_name()));
        candidates.push(target_dir.join(target).join("debug").join(bin_name()));
    }
    candidates.push(target_dir.join(&profile).join(bin_name()));
    candidates.push(target_dir.join("debug").join(bin_name()));

    candidates
        .iter()
        .find(|path| path.exists())
        .unwrap_or(&candidates[0])
        .display()
        .to_string()
}

fn bin_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "agent-sim.exe"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "agent-sim"
    }
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
