//! Progress feedback during the sandbox preparation.
//!
//! The spinner may only appear on a terminal, it must animate while the
//! preparation runs, and it must be erased before anything else is printed.

mod common;

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::FromRawFd;
use std::process::{Command, Stdio};

/// A pseudo terminal pair. The child gets the slave as its stderr, so it
/// looks like an interactive terminal to Realm.
fn pty() -> (std::fs::File, std::fs::File) {
    unsafe {
        let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        assert!(master >= 0, "posix_openpt failed");
        assert_eq!(libc::grantpt(master), 0, "grantpt failed");
        assert_eq!(libc::unlockpt(master), 0, "unlockpt failed");
        let name = libc::ptsname(master);
        assert!(!name.is_null(), "ptsname failed");
        let name = std::ffi::CStr::from_ptr(name).to_owned();
        let slave = libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
        assert!(slave >= 0, "cannot open the pty slave");
        (
            std::fs::File::from_raw_fd(master),
            std::fs::File::from_raw_fd(slave),
        )
    }
}

/// Read the pty until the slave side is gone. Linux ends the read with EIO
/// instead of EOF when the last slave closes.
fn read_pty(mut master: std::fs::File) -> String {
    let mut output = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        match master.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => output.extend_from_slice(&buffer[..read]),
            Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
            Err(error) => panic!("read the pty: {error}"),
        }
    }
    String::from_utf8_lossy(&output).into_owned()
}

/// A fixture whose `nix` is slow enough for the spinner to animate.
struct SlowPreparation {
    root: tempfile::TempDir,
    config: std::path::PathBuf,
}

impl SlowPreparation {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("create progress fixture");
        let config = root.path().join("realm.nix");
        std::fs::write(&config, "{}").unwrap();
        let nix = root.path().join("nix");
        std::fs::write(&nix, "#!/bin/sh\nsleep 0.5\necho fake-nix >&2\nexit 1\n").unwrap();
        let mut permissions = std::fs::metadata(&nix).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&nix, permissions).unwrap();
        Self { root, config }
    }

    /// Realm with the slow `nix` first on `PATH`.
    fn command(&self) -> Command {
        let path = format!(
            "{}:{}",
            self.root.path().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut command = Command::new(env!("CARGO_BIN_EXE_realm"));
        command
            .args(["--config", self.config.to_str().unwrap(), "agent"])
            .env("PATH", path)
            .env_remove("NO_COLOR");
        command
    }
}

#[test]
fn a_terminal_sees_the_spinner_and_its_erasure() {
    let fixture = SlowPreparation::new();
    let (master, slave) = pty();
    let mut child = fixture
        .command()
        .stderr(Stdio::from(slave))
        .stdout(Stdio::null())
        .spawn()
        .expect("start realm");
    let status = child.wait().expect("wait for realm");
    let output = read_pty(master);
    assert!(!status.success(), "the failing preparation must fail");
    assert!(
        output.contains("fake-nix"),
        "the slow preparation did not run: {output:?}"
    );

    assert!(
        output.contains("phase: evaluating sandbox configuration"),
        "the spinner never appeared: {output:?}"
    );
    assert!(
        output.matches("phase: evaluating sandbox configuration").count() > 1,
        "the spinner did not animate: {output:?}"
    );
    let erased = output
        .find("\x1b[2K")
        .expect("the spinner line was not erased");
    let error = output.find("Error:").expect("the failure was not reported");
    assert!(
        erased < error,
        "the spinner residue survived the error message: {output:?}"
    );
}

#[test]
fn a_pipe_never_sees_the_spinner() {
    let fixture = SlowPreparation::new();
    let output = fixture.command().output().expect("start realm");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("fake-nix"),
        "the slow preparation did not run: {stderr:?}"
    );
    assert!(
        !stderr.contains("Preparing sandbox"),
        "the spinner was written to a pipe: {stderr:?}"
    );
    assert!(
        !stderr.contains('\x1b') && !stderr.contains('\r'),
        "control characters reached a pipe: {stderr:?}"
    );
}

/// A configuration with a sandbox that prints to stdout and stderr. The
/// temporary directory must outlive the configuration.
fn sandbox_config() -> (tempfile::TempDir, std::path::PathBuf) {
    let root = tempfile::tempdir().expect("create progress fixture");
    let config = root.path().join("realm.nix");
    std::fs::write(
        &config,
        r#"{ nixpkgs ? builtins.getFlake "nixpkgs", system ? builtins.currentSystem }:
let pkgs = import nixpkgs.sourceInfo.outPath { inherit system; };
in
{
  progress-test = {
    packages = with pkgs; [ bash coreutils ];
    env = { };
    network = "none";
    resources = { enable = true; backend = "rlimit"; };
  };
}
"#,
    )
    .unwrap();
    (root, config)
}

#[test]
fn the_sandboxed_process_output_is_not_touched() {
    if !common::sandbox_available("progress") {
        return;
    }
    let (_root, config) = sandbox_config();
    let output = Command::new(env!("CARGO_BIN_EXE_realm"))
        .args([
            "--config",
            config.to_str().unwrap(),
            "progress-test",
            "--",
            "bash",
            "--noprofile",
            "--norc",
            "-c",
            "echo out; echo err >&2",
        ])
        .output()
        .expect("start realm");
    assert!(
        output.status.success(),
        "the sandbox failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "out\n");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "err\n");
}

#[test]
fn the_sandboxed_process_output_survives_the_spinner() {
    if !common::sandbox_available("progress") {
        return;
    }
    let (_root, config) = sandbox_config();
    let (master, slave) = pty();
    let mut child = Command::new(env!("CARGO_BIN_EXE_realm"))
        .args([
            "--config",
            config.to_str().unwrap(),
            "progress-test",
            "--",
            "bash",
            "--noprofile",
            "--norc",
            "-c",
            "echo out; echo err >&2",
        ])
        .stdout(Stdio::from(
            slave.try_clone().expect("duplicate the pty slave"),
        ))
        .stderr(Stdio::from(slave))
        .spawn()
        .expect("start realm");
    let status = child.wait().expect("wait for realm");
    let output = read_pty(master);
    assert!(status.success(), "the sandbox failed: {output:?}");
    // Everything the sandboxed process printed comes after the spinner line
    // was erased.
    let after = output.rsplit("\x1b[2K").next().unwrap_or_default();
    assert!(
        after.contains("out"),
        "the process output was lost: {output:?}"
    );
    assert!(
        after.contains("err"),
        "the process output was lost: {output:?}"
    );
}
