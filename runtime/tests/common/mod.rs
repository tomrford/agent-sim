use std::path::{Path, PathBuf};
use std::sync::Once;
use std::thread;
use std::time::Duration;
use uuid::Uuid;

static FIXTURES_BUILT: Once = Once::new();

pub fn unique_session(prefix: &str) -> String {
    let id = Uuid::new_v4().simple().to_string();
    format!("{prefix}-{}", &id[..12])
}

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("runtime should have workspace parent")
        .to_path_buf()
}

pub fn template_lib_path() -> PathBuf {
    fixture_lib_path("template", "sim_template")
}

pub fn hvac_lib_path() -> PathBuf {
    fixture_lib_path("examples/hvac", "sim_hvac_example")
}

fn fixture_lib_path(project: &str, base_name: &str) -> PathBuf {
    repo_root()
        .join(project)
        .join("zig-out")
        .join(lib_dir())
        .join(format!("{}{}.{}", lib_prefix(), base_name, lib_extension()))
}

pub fn ensure_fixtures_built() {
    FIXTURES_BUILT.call_once(|| {
        run_command(&repo_root().join("template"), "zig", &["build"]);
        run_command(
            &repo_root().join("examples").join("hvac"),
            "zig",
            &["build"],
        );
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
    run_agent_in_home(&test_agent_sim_home(), args)
}

pub fn run_agent_in_home(home: &Path, args: &[&str]) -> String {
    run_agent_command(home, args, true).0
}

#[allow(dead_code)]
pub fn run_agent_fail(args: &[&str]) -> String {
    run_agent_fail_in_home(&test_agent_sim_home(), args)
}

#[allow(dead_code)]
pub fn run_agent_fail_in_home(home: &Path, args: &[&str]) -> String {
    run_agent_command(home, args, false).1
}

fn run_agent_command(home: &Path, args: &[&str], expect_success: bool) -> (String, String) {
    use std::sync::mpsc;

    let exe = resolve_agent_exe();
    let stdout_file = tempfile::NamedTempFile::new().expect("stdout temp file should be creatable");
    let stderr_file = tempfile::NamedTempFile::new().expect("stderr temp file should be creatable");
    let mut child = std::process::Command::new(&exe)
        .env("AGENT_SIM_HOME", home)
        .args(args)
        .stdout(std::process::Stdio::from(
            stdout_file
                .reopen()
                .expect("stdout temp file should be reopenable"),
        ))
        .stderr(std::process::Stdio::from(
            stderr_file
                .reopen()
                .expect("stderr temp file should be reopenable"),
        ))
        .spawn()
        .unwrap_or_else(|err| panic!("failed to spawn '{exe}' with args {args:?}: {err}"));

    let wait_timeout = Duration::from_secs(15);
    let pid = child.id();
    let (wait_tx, wait_rx) = mpsc::channel();
    let (wait_result, timed_out) = thread::scope(|scope| {
        scope.spawn(move || {
            let _ = wait_tx.send(child.wait());
        });
        match wait_rx.recv_timeout(wait_timeout) {
            Ok(status) => (status, false),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Err(err) = agent_sim::process::kill_pid(pid) {
                    panic!(
                        "timed out and failed to kill child '{exe}' (pid {pid}) with args {args:?}: {err}"
                    );
                }
                let status = wait_rx.recv().unwrap_or_else(|_| {
                    panic!("wait thread disconnected for '{exe}' with args {args:?}")
                });
                (status, true)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("wait thread disconnected for '{exe}' with args {args:?}")
            }
        }
    });
    let status = wait_result
        .unwrap_or_else(|err| panic!("failed to wait child '{exe}' with args {args:?}: {err}"));
    assert!(
        !timed_out,
        "command timed out after 15s: exe='{exe}' args={args:?} AGENT_SIM_HOME='{}'",
        home.display()
    );

    let stdout =
        std::fs::read_to_string(stdout_file.path()).expect("stdout temp file should be readable");
    let stderr =
        std::fs::read_to_string(stderr_file.path()).expect("stderr temp file should be readable");

    if expect_success {
        assert!(
            status.success(),
            "command failed: exe='{exe}' args={args:?}\nstdout: {stdout}\nstderr: {stderr}"
        );
    } else {
        assert!(
            !status.success(),
            "command unexpectedly succeeded: exe='{exe}' args={args:?}\nstdout: {stdout}\nstderr: {stderr}"
        );
    }

    (stdout, stderr)
}

fn run_command(dir: &Path, program: &str, args: &[&str]) {
    let output = std::process::Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("fixture build command should run");
    assert!(
        output.status.success(),
        "fixture build command failed: {program} {}\nstdout: {}\nstderr: {}",
        args.join(" "),
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
    let path = std::env::temp_dir().join(format!("asim-{short}"));
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

fn lib_dir() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "bin"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "lib"
    }
}

fn lib_prefix() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        ""
    }
    #[cfg(not(target_os = "windows"))]
    {
        "lib"
    }
}
