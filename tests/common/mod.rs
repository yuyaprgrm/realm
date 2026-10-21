#![allow(dead_code)]

use std::process::Command;

/// Probe the capabilities the integration tests need. Returns the reason a
/// test should be skipped instead of failing.
pub fn sandbox_unavailable() -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Err("Realm runtime tests require Linux".into());
    }
    for tool in ["bwrap", "nix"] {
        if Command::new(tool).arg("--version").output().is_err() {
            return Err(format!("`{tool}` is not available on PATH"));
        }
    }

    // The fixtures evaluate `builtins.getFlake "nixpkgs"`.
    let registry = Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command flakes",
            "--offline",
            "--impure",
            "--raw",
            "--expr",
            "(builtins.getFlake \"nixpkgs\").outPath",
        ])
        .output()
        .map_err(|error| format!("cannot run nix: {error}"))?;
    if !registry.status.success() {
        return Err(format!(
            "the nixpkgs flake registry entry is unavailable: {}",
            String::from_utf8_lossy(&registry.stderr).trim()
        ));
    }

    // Verify that this environment really allows bwrap to create namespaces.
    let smoke = Command::new("bwrap")
        .args([
            "--unshare-user",
            "--unshare-pid",
            "--unshare-net",
            "--ro-bind",
            "/",
            "/",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--",
            "true",
        ])
        .output()
        .map_err(|error| format!("cannot run bwrap: {error}"))?;
    if !smoke.status.success() {
        return Err(format!(
            "bwrap cannot create user/PID namespaces here: {}",
            String::from_utf8_lossy(&smoke.stderr).trim()
        ));
    }

    Ok(())
}

/// Private networking needs pasta and a working systemd user manager.
pub fn private_network_unavailable() -> Result<(), String> {
    sandbox_unavailable()?;
    if Command::new("pasta").arg("--version").output().is_err() {
        return Err("`pasta` is not available on PATH".into());
    }
    let scope = Command::new("systemd-run")
        .args(["--user", "--scope", "--quiet", "true"])
        .output()
        .map_err(|error| format!("cannot run systemd-run: {error}"))?;
    if !scope.status.success() {
        return Err(format!(
            "systemd user scopes are unavailable: {}",
            String::from_utf8_lossy(&scope.stderr).trim()
        ));
    }
    Ok(())
}

pub fn sandbox_available(test: &str) -> bool {
    report(test, sandbox_unavailable())
}

pub fn private_network_available(test: &str) -> bool {
    report(test, private_network_unavailable())
}

fn report(test: &str, result: Result<(), String>) -> bool {
    match result {
        Ok(()) => true,
        Err(reason) => {
            eprintln!("SKIP {test}: {reason}");
            false
        }
    }
}
