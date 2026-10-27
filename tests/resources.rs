mod common;

use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture {
    _root: tempfile::TempDir,
    config: PathBuf,
}

impl Fixture {
    /// Fixture with the given `resources` block.
    fn new(resources: &str) -> Self {
        let root = tempfile::tempdir().expect("create resources fixture");
        let config = root.path().join("realm.nix");
        std::fs::write(
            &config,
            format!(
                r#"{{ nixpkgs ? builtins.getFlake "nixpkgs", system ? builtins.currentSystem }}:
let pkgs = import nixpkgs.sourceInfo.outPath {{ inherit system; }};
in
{{
  resources-test = {{
    packages = with pkgs; [ bash coreutils ];
    env = {{ }};
    network = "none";
    resources = {resources};
  }};
}}
"#
            ),
        )
        .unwrap();
        Self {
            _root: root,
            config,
        }
    }

    fn run(&self, script: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_realm"))
            .args([
                "--config",
                self.config.to_str().unwrap(),
                "run",
                "resources-test",
                "--",
                "bash",
                "--noprofile",
                "--norc",
                "-c",
                script,
            ])
            .output()
            .expect("start sandbox binary")
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The limit the host process itself runs with, so the assertions do not
/// depend on the limits of the machine running the tests.
fn host_limit(kind: &str) -> String {
    let output = Command::new("bash")
        .arg("-c")
        .arg(format!("ulimit -{kind}"))
        .output()
        .expect("read host limit");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn disabled_resources_apply_no_limit() {
    if !common::sandbox_available("resources::disabled") {
        return;
    }
    let fixture = Fixture::new("{ enable = false; }");
    let output = fixture.run("ulimit -v; ulimit -t");
    assert!(
        output.status.success(),
        "sandbox failed: {}",
        stderr(&output)
    );
    let limits = stdout(&output);
    let (address_space, cpu_time) = limits.split_once('\n').expect("two limits");
    assert_eq!(address_space, host_limit("v"), "{limits}");
    assert_eq!(cpu_time, host_limit("t"), "{limits}");
}

#[test]
fn the_rlimit_backend_applies_the_configured_limits() {
    if !common::sandbox_available("resources::rlimit") {
        return;
    }
    let fixture = Fixture::new(r#"{ backend = "rlimit"; memory = "256M"; }"#);
    let output = fixture.run("ulimit -v; ulimit -t");
    assert!(
        output.status.success(),
        "sandbox failed: {}",
        stderr(&output)
    );
    let limits = stdout(&output);
    let (address_space, cpu_time) = limits.split_once('\n').expect("two limits");
    // RLIMIT_AS is in 1024-byte blocks, RLIMIT_CPU in seconds.
    assert_eq!(address_space, "262144", "{limits}");
    assert_eq!(cpu_time, "3600", "{limits}");
}

#[test]
fn the_cgroup_backend_does_not_fall_back_to_rlimits() {
    if !common::sandbox_available("resources::cgroup") {
        return;
    }
    if !common::systemd_user_scope_available() {
        eprintln!("SKIP resources::cgroup: no systemd user manager");
        return;
    }
    let fixture = Fixture::new(r#"{ backend = "cgroup"; }"#);
    let output = fixture.run("ulimit -v; ulimit -t");
    assert!(
        output.status.success(),
        "sandbox failed: {}",
        stderr(&output)
    );
    let limits = stdout(&output);
    let (address_space, cpu_time) = limits.split_once('\n').expect("two limits");
    // The cgroup backend limits the cgroup, not the rlimits of the process.
    assert_eq!(address_space, host_limit("v"), "{limits}");
    assert_eq!(cpu_time, host_limit("t"), "{limits}");
}

#[test]
fn the_cgroup_backend_fails_closed_without_systemd() {
    if !common::sandbox_available("resources::cgroup-missing") {
        return;
    }
    if common::systemd_user_scope_available() {
        eprintln!("SKIP resources::cgroup-missing: a systemd user manager is available");
        return;
    }
    let fixture = Fixture::new(r#"{ backend = "cgroup"; }"#);
    let output = fixture.run("true");
    assert!(
        !output.status.success(),
        "expected the sandbox to refuse to start"
    );
    let message = stderr(&output);
    assert!(message.contains("cgroup"), "{message}");
    assert!(message.contains("systemd"), "{message}");
}

#[test]
fn an_unknown_backend_is_rejected() {
    if !common::sandbox_available("resources::backend") {
        return;
    }
    let fixture = Fixture::new(r#"{ backend = "systemd"; }"#);
    let output = fixture.run("true");
    assert!(
        !output.status.success(),
        "expected the configuration to be rejected"
    );
    let message = stderr(&output);
    assert!(message.contains("resources.backend"), "{message}");
}
