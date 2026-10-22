//! rlimit resource limits.
//!
//! The limits are applied with `setrlimit` in the sandbox process tree and a
//! coreutils `timeout` wrapper for the wall-clock limit. This backend is
//! used only when it is selected explicitly; it is never a fallback for the
//! cgroup backend.

use crate::error::{Error, Result};
use crate::sandbox::spec::SandboxSpec;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

/// Apply the rlimit limits and return the command to run.
pub fn exec(spec: &SandboxSpec, mut command: Command) -> Result<Command> {
    if let Some(seconds) = spec.limits.timeout_secs {
        apply_timeout(&mut command, seconds)?;
    }
    apply_rlimits(&mut command, spec)?;
    Ok(command)
}

/// Wrap the sandbox command in the coreutils `timeout` command. The timeout
/// runs outside the sandbox and terminates the whole process tree.
fn apply_timeout(command: &mut Command, seconds: u64) -> Result<()> {
    let timeout = which::which("timeout").map_err(|error| {
        Error::Bwrap(format!(
            "cannot enforce the configured wall-clock timeout: `timeout` was not found on PATH: {error}"
        ))
    })?;
    apply_timeout_with(&timeout, command, seconds);
    Ok(())
}

fn apply_timeout_with(timeout: &Path, command: &mut Command, seconds: u64) {
    let mut wrapped = Command::new(timeout);
    // The timeout process itself must not inherit host-only variables.
    wrapped.env_clear();
    wrapped
        .arg("--signal=TERM")
        .arg("--kill-after=5s")
        .arg(format!("{seconds}s"))
        .arg("--")
        .arg(command.get_program())
        .args(command.get_args());
    *command = wrapped;
}

/// Apply the rlimits in the child before exec. Address space, process count,
/// and CPU-time are mandatory in this mode: if a limit cannot be installed,
/// exec fails instead of running with silently weaker limits (fail-closed).
fn apply_rlimits(command: &mut Command, spec: &SandboxSpec) -> Result<()> {
    let mem = spec.limits.memory_bytes;
    let cpu = spec.limits.timeout_secs.unwrap_or(0);
    let nproc = spec.limits.max_pids as u64;

    unsafe {
        command.pre_exec(move || {
            set_limit(libc::RLIMIT_AS, mem)?;
            set_limit(libc::RLIMIT_NPROC, nproc)?;
            set_limit(libc::RLIMIT_CPU, cpu)?;
            Ok(())
        });
    }
    Ok(())
}

/// Install a hard upper bound for `resource`.
///
/// `0` means "not configured" and is ignored. If the inherited hard limit is
/// lower than the requested value the lower limit is kept; the maximum is
/// never raised, so the sandbox cannot undo the cap later.
fn set_limit(resource: libc::c_uint, value: u64) -> std::io::Result<()> {
    if value == 0 {
        return Ok(());
    }
    let mut current = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(resource, &mut current) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let requested = value as libc::rlim_t;
    let max = match current.rlim_max {
        libc::RLIM_INFINITY => requested,
        hard if hard < requested => hard,
        _ => requested,
    };
    let lim = libc::rlimit {
        rlim_cur: max,
        rlim_max: max,
    };
    if unsafe { libc::setrlimit(resource, &lim) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
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

    #[test]
    fn the_timeout_wraps_the_sandbox_command() {
        let mut command = Command::new("/usr/bin/bwrap");
        command.arg("--unshare-pid");
        apply_timeout_with(Path::new("/usr/bin/timeout"), &mut command, 60);
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            vec![
                "--signal=TERM",
                "--kill-after=5s",
                "60s",
                "--",
                "/usr/bin/bwrap",
                "--unshare-pid"
            ]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>()
        );
        assert_eq!(command.get_envs().count(), 0);
    }

    #[test]
    fn rlimits_stop_a_child_that_exceeds_them() {
        let mut s = spec();
        s.limits.timeout_secs = Some(1);
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg("while :; do :; done");
        apply_rlimits(&mut cmd, &s).unwrap();
        let status = cmd.status().unwrap();
        assert!(
            !status.success(),
            "child exceeded the CPU limit but still succeeded"
        );
    }
}
