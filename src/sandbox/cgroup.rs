use crate::error::{Error, Result};
use crate::sandbox::bwrap;
use crate::sandbox::spec::SandboxSpec;
use std::os::unix::process::CommandExt;
use std::process::Command;

/// Run the sandbox, applying resource limits via a transient systemd scope
/// (`systemd-run --user --scope`) when available. This gives real cgroup v2
/// enforcement of memory/CPU/task counts the way bwrap alone cannot.
///
/// Falls back to in-process rlimits when the user systemd manager is not
/// reachable (headless / non-systemd hosts).
pub fn exec_with_limits(spec: &SandboxSpec) -> Result<()> {
    // For Internet policy, pasta needs to run alongside bwrap in the same
    // namespace plumbing. We build the command graph here.
    if spec.network == crate::sandbox::spec::NetworkPolicy::Internet {
        return exec_with_pasta(spec);
    }
    let bwrap = bwrap::build_command(spec)?;

    let mut cmd = if systemd_user_scope_available() {
        systemd_wrapper(spec, &bwrap)
    } else {
        let mut c = bwrap;
        bwrap::apply_rlimits(&mut c, spec);
        c
    };

    // exec() only returns on failure.
    let err = cmd.exec();
    Err(Error::Bwrap(format!("exec failed: {}", err)))
}

/// Build the `systemd-run --user --scope ...` wrapper around a bwrap command.
fn systemd_wrapper(spec: &SandboxSpec, bwrap: &Command) -> Command {
    let mut cmd = Command::new("systemd-run");
    cmd.arg("--user")
        .arg("--scope")
        .arg("--quiet")
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
    cmd.arg(bwrap.get_program());
    for a in bwrap.get_args() {
        cmd.arg(a);
    }
    cmd
}

/// Whether `systemd-run --user` can reach a user manager.
fn systemd_user_scope_available() -> bool {
    if which::which("systemd-run").is_err() {
        return false;
    }
    // `systemd-run --user` needs a session/user bus.
    std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some()
        || std::env::var_os("XDG_RUNTIME_DIR")
            .map(|d| std::path::Path::new(&d).join("systemd/private").exists())
            .unwrap_or(false)
}

/// Attach pasta to provide network connectivity for Internet policy.
pub fn exec_with_pasta(spec: &SandboxSpec) -> Result<()> {
    let pasta_path = crate::sandbox::network::require_pasta()?;
    let bwrap_cmd = bwrap::build_command(spec)?;
    let mut pasta = Command::new(&pasta_path);
    // Run bwrap under pasta's network namespace
    pasta.arg("--");
    pasta.arg(bwrap_cmd.get_program());
    for a in bwrap_cmd.get_args() {
        pasta.arg(a);
    }
    let mut cmd = pasta;
    let err = cmd.exec();
    Err(Error::Bwrap(format!("exec failed: {}", err)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::spec::{NetworkPolicy, ResourceLimits, SandboxSpec};

    fn spec() -> SandboxSpec {
        SandboxSpec {
            mounts: vec![],
            env: vec![],
            cwd: "/".into(),
            path_bins: vec![],
            executable: "/bin/true".into(),
            args: vec![],
            network: NetworkPolicy::Disabled,
            limits: ResourceLimits::default(),
            uid: 1000,
            gid: 1000,
            keep_alive: vec![],
        }
    }

    fn wrapper_args(s: &SandboxSpec) -> Vec<String> {
        systemd_wrapper(s, &Command::new("bwrap"))
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
        // no timeout configured => no RuntimeMaxSec
        assert!(!args.iter().any(|a| a.starts_with("RuntimeMaxSec")));
    }

    #[test]
    fn systemd_wrapper_includes_timeout_when_set() {
        let mut s = spec();
        s.limits.timeout_secs = Some(60);
        assert!(wrapper_args(&s).contains(&"RuntimeMaxSec=60".to_string()));
    }
}
