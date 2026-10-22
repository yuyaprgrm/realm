//! Resource limit enforcement.
//!
//! Exactly one backend enforces the limits, selected by `resources.backend`.
//! There is no fallback to a weaker mechanism: when the selected backend
//! cannot be applied, the sandbox refuses to start.

use crate::error::{Error, Result};
use crate::sandbox::bwrap;
use crate::sandbox::cgroup;
use crate::sandbox::network;
use crate::sandbox::rlimit;
use crate::sandbox::spec::{NetworkMode, ResourceBackend, SandboxSpec};
use std::path::Path;
use std::process::{Command, ExitStatus};

/// Run the sandbox, applying the configured resource limits.
pub fn exec(spec: &SandboxSpec, spinner: &mut crate::ui::Spinner) -> Result<ExitStatus> {
    let command = sandbox_command(spec)?;
    let mut command = wrap(spec, command)?;
    // The sandboxed process writes to the terminal from here on, so the
    // spinner must be gone before the process starts.
    spinner.stop();
    command
        .status()
        .map_err(|error| Error::Bwrap(format!("spawn failed: {error}")))
}

/// Wrap the sandbox command for the configured backend.
///
/// Only the selected backend is used. Disabled limits leave the command
/// untouched, and a backend that cannot be applied fails the launch.
fn wrap(spec: &SandboxSpec, command: Command) -> Result<Command> {
    if !spec.limits.enabled {
        return Ok(command);
    }
    match spec.limits.backend {
        ResourceBackend::Cgroup => cgroup::exec(spec, command),
        ResourceBackend::Rlimit => rlimit::exec(spec, command),
    }
}

/// Build the sandbox command: bwrap, wrapped in pasta for private networking.
fn sandbox_command(spec: &SandboxSpec) -> Result<Command> {
    let bwrap = bwrap::build_command(spec)?;
    if spec.network == NetworkMode::Private {
        let pasta = network::require_pasta()?;
        Ok(pasta_command(&pasta, &bwrap))
    } else {
        Ok(bwrap)
    }
}

/// Run bwrap inside a pasta-managed private network namespace.
///
/// pasta inherits an empty environment so that host-only variables never
/// reach the sandbox process.
fn pasta_command(pasta_path: &Path, bwrap: &Command) -> Command {
    let mut pasta = Command::new(pasta_path);
    pasta.env_clear();
    pasta
        .arg("--foreground")
        .arg("--quiet")
        .arg("--config-net")
        .arg("--address")
        .arg(network::PASTA_ADDRESS)
        .arg("--gateway")
        .arg(network::PASTA_GATEWAY)
        .arg("--dns-forward")
        .arg(network::PASTA_DNS_FORWARD)
        .arg("--map-host-loopback")
        .arg("none")
        .arg("--tcp-ns")
        .arg("none")
        .arg("--udp-ns")
        .arg("none");
    pasta.arg(bwrap.get_program());
    pasta.args(bwrap.get_args());
    pasta
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::spec::{ResourceBackend, ResourceLimits, SandboxSpec};

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
    fn disabled_limits_leave_the_command_untouched() {
        let mut s = spec();
        s.limits.enabled = false;
        let command = wrap(&s, Command::new("/usr/bin/bwrap")).unwrap();
        assert_eq!(command.get_program(), Path::new("/usr/bin/bwrap"));
    }

    #[test]
    fn the_rlimit_backend_wraps_the_command_with_a_timeout() {
        let mut s = spec();
        s.limits.backend = ResourceBackend::Rlimit;
        let command = wrap(&s, Command::new("/usr/bin/bwrap")).unwrap();
        assert!(
            command.get_program().to_string_lossy().ends_with("timeout"),
            "the rlimit backend does not use the timeout wrapper"
        );
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        // The default one-hour wall-clock timeout.
        assert!(args.contains(&"3600s".to_string()), "{args:?}");
        assert_eq!(args.last().map(String::as_str), Some("/usr/bin/bwrap"));
    }

    #[test]
    fn private_pasta_does_not_map_host_loopback() {
        let command = pasta_command(Path::new("/bin/pasta"), &Command::new("/usr/bin/bwrap"));
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--map-host-loopback", "none"])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--address", "10.0.2.100/24"])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--gateway", "10.0.2.2"])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--dns-forward", "10.0.2.3"])
        );
        assert!(args.windows(2).any(|pair| pair == ["--tcp-ns", "none"]));
        assert!(args.windows(2).any(|pair| pair == ["--udp-ns", "none"]));
        assert!(!args.iter().any(|arg| arg == "--ipv4-only"));
        // The sandbox must not inherit host-only variables.
        assert_eq!(command.get_envs().count(), 0);
        assert_eq!(args.last().map(String::as_str), Some("/usr/bin/bwrap"));
    }
}
