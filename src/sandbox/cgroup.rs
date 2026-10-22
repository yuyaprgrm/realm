//! cgroup resource limits.
//!
//! The limits are applied through a transient systemd user scope
//! (`systemd-run --user --scope`). The scope is mandatory: without a systemd
//! user manager the sandbox refuses to start instead of running with weaker
//! limits.

use crate::error::{Error, Result};
use crate::sandbox::spec::SandboxSpec;
use std::path::Path;
use std::process::{Command, Stdio};

/// Run the sandbox command inside a systemd scope that carries the cgroup
/// limits.
pub fn exec(spec: &SandboxSpec, command: Command) -> Result<Command> {
    exec_with(spec, command, systemd_user_scope_available())
}

fn exec_with(spec: &SandboxSpec, command: Command, systemd_available: bool) -> Result<Command> {
    if !systemd_available {
        return Err(Error::Bwrap(
            "resources.backend = \"cgroup\" requires a systemd user manager to enforce the cgroup \
             limits, but no user scope could be created; the sandbox refuses to start instead of \
             running with weaker limits"
                .into(),
        ));
    }
    systemd_wrapper(spec, &command)
}

/// Build the `systemd-run --user --scope ...` wrapper around a sandbox command.
fn systemd_wrapper(spec: &SandboxSpec, command: &Command) -> Result<Command> {
    let env = which::which("env").map_err(|error| {
        Error::Bwrap(format!(
            "cannot sanitize the sandbox process environment: `env` was not found on PATH: {error}"
        ))
    })?;
    Ok(systemd_wrapper_with(&env, spec, command))
}

fn systemd_wrapper_with(env: &Path, spec: &SandboxSpec, command: &Command) -> Command {
    let mut cmd = Command::new("systemd-run");
    // systemd-run needs a user bus/runtime directory, but the sandbox command
    // it launches is wrapped in `env -i`, so no other host variable reaches
    // the sandbox process (or /proc/1/environ).
    cmd.env_clear();
    for key in ["XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS", "PATH"] {
        if let Some(value) = std::env::var_os(key) {
            cmd.env(key, value);
        }
    }
    cmd.arg("--user")
        .arg("--scope")
        .arg("--quiet")
        .arg("--unit")
        .arg(format!("realm-sandbox-{}.scope", std::process::id()))
        .arg("-p")
        .arg(format!("MemoryMax={}", spec.limits.memory_bytes))
        .arg("-p")
        .arg(format!("CPUQuota={}%", spec.limits.cpu_percent))
        .arg("-p")
        .arg(format!("TasksMax={}", spec.limits.max_pids));
    if let Some(t) = spec.limits.timeout_secs {
        cmd.arg("-p").arg(format!("RuntimeMaxSec={}", t));
    }
    cmd.arg("--");
    // `env -i` keeps the variables of the systemd-run environment out of the
    // sandbox process.
    cmd.arg(env);
    cmd.arg("-i").arg("--");
    cmd.arg(command.get_program());
    cmd.args(command.get_args());
    cmd
}

/// Whether `systemd-run --user` can actually create a user scope.
fn systemd_user_scope_available() -> bool {
    Command::new("systemd-run")
        .args(["--user", "--scope", "--quiet", "true"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::spec::{NetworkMode, ResourceLimits, SandboxSpec};

    fn spec() -> SandboxSpec {
        SandboxSpec {
            mounts: vec![],
            env: vec![],
            cwd: "/".into(),
            executable: "/bin/true".into(),
            args: vec![],
            network: NetworkMode::None,
            limits: ResourceLimits::default(),
            uid: 1000,
            gid: 1000,
            keep_alive: vec![],
        }
    }

    fn wrapper_args(s: &SandboxSpec) -> Vec<String> {
        systemd_wrapper_with(
            Path::new("/usr/bin/env"),
            s,
            &Command::new("/usr/bin/bwrap"),
        )
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
    }

    #[test]
    fn systemd_wrapper_sets_limits() {
        let args = wrapper_args(&spec());
        assert_eq!(args[0], "--user");
        assert_eq!(args[1], "--scope");
        assert!(args.contains(&"MemoryMax=2147483648".to_string()));
        assert!(args.contains(&"CPUQuota=100%".to_string()));
        assert!(args.contains(&"TasksMax=512".to_string()));
        assert!(args.contains(&"RuntimeMaxSec=3600".to_string()));
    }

    #[test]
    fn systemd_wrapper_includes_timeout_when_set() {
        let mut s = spec();
        s.limits.timeout_secs = Some(60);
        assert!(wrapper_args(&s).contains(&"RuntimeMaxSec=60".to_string()));
    }

    #[test]
    fn systemd_wrapper_runs_the_sandbox_with_an_empty_environment() {
        let mut bwrap = Command::new("/usr/bin/bwrap");
        bwrap.arg("--unshare-pid");
        let args = systemd_wrapper_with(Path::new("/usr/bin/env"), &spec(), &bwrap)
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let separator = args
            .iter()
            .position(|arg| arg == "--")
            .expect("systemd-run separator");
        // `env -i --` keeps the host environment out of the sandbox.
        assert_eq!(args[separator + 1], "/usr/bin/env");
        assert_eq!(args[separator + 2], "-i");
        assert_eq!(args[separator + 3], "--");
        assert_eq!(args[separator + 4], "/usr/bin/bwrap");
        assert_eq!(args[separator + 5], "--unshare-pid");
    }

    #[test]
    fn a_missing_systemd_manager_fails_closed() {
        let error = exec_with(&spec(), Command::new("/usr/bin/bwrap"), false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("cgroup"), "{error}");
        assert!(error.contains("systemd"), "{error}");
    }

    #[test]
    fn an_available_systemd_manager_builds_the_scope() {
        let command = exec_with(&spec(), Command::new("/usr/bin/bwrap"), true).unwrap();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args[0], "--user");
        assert!(args.contains(&"MemoryMax=2147483648".to_string()));
    }
}
