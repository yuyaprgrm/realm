use crate::error::{Error, Result};
use crate::sandbox::bwrap;
use crate::sandbox::spec::SandboxSpec;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

/// Run the sandbox, applying resource limits via a transient systemd scope
/// (`systemd-run --user --scope`) when available.
pub fn exec_with_limits(spec: &SandboxSpec) -> Result<()> {
    let bwrap = bwrap::build_command(spec)?;

    if spec.network == crate::sandbox::spec::NetworkMode::Private {
        if !systemd_user_scope_available() {
            return Err(Error::Bwrap(
                "network mode \"private\" requires an available systemd user manager to enforce cgroup limits".into(),
            ));
        }
        let pasta_path = crate::sandbox::network::require_pasta()?;
        // pasta starts in the background and configures a user/network namespace
        // for its caller. Run it inside the scope, then have bwrap join that
        // namespace instead of creating a disconnected one of its own.
        let mut pasta = Command::new(pasta_path);
        pasta
            .arg("--foreground")
            .arg("--quiet")
            .arg("--config-net")
            .arg("--ipv4-only")
            .arg("--address")
            .arg(crate::sandbox::network::PASTA_ADDRESS)
            .arg("--gateway")
            .arg(crate::sandbox::network::PASTA_GATEWAY)
            .arg("--dns-forward")
            .arg(crate::sandbox::network::PASTA_DNS_FORWARD);
        pasta.stdout(Stdio::inherit()).stderr(Stdio::inherit());
        let mut helper = Command::new("/bin/sh");
        helper.arg("-c").arg("exec \"$@\"").arg("realm-pasta");
        helper.arg(bwrap.get_program());
        helper.args(bwrap.get_args());
        pasta.arg(helper.get_program()).args(helper.get_args());

        let mut cmd = systemd_wrapper(spec, &pasta);
        let err = cmd.exec();
        return Err(Error::Bwrap(format!("exec failed: {}", err)));
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::spec::{NetworkMode, ResourceLimits, SandboxSpec};

    fn spec() -> SandboxSpec {
        SandboxSpec {
            mounts: vec![],
            env: vec![],
            cwd: "/".into(),
            path_bins: vec![],
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
        assert!(args.contains(&"RuntimeMaxSec=3600".to_string()));
    }

    #[test]
    fn systemd_wrapper_includes_timeout_when_set() {
        let mut s = spec();
        s.limits.timeout_secs = Some(60);
        assert!(wrapper_args(&s).contains(&"RuntimeMaxSec=60".to_string()));
    }

    #[test]
    fn systemd_wrapper_keeps_pasta_inside_the_limited_scope() {
        let mut s = spec();
        s.network = NetworkMode::Private;
        s.limits.timeout_secs = Some(45);
        let pasta = Command::new("pasta");
        let args = wrapper_args_for(&s, &pasta);
        assert_eq!(args.last().map(String::as_str), Some("pasta"));
        assert!(args.contains(&"MemoryMax=2147483648".to_string()));
        assert!(args.contains(&"CPUQuota=100%".to_string()));
        assert!(args.contains(&"TasksMax=512".to_string()));
        assert!(args.contains(&"RuntimeMaxSec=45".to_string()));
    }

    fn wrapper_args_for(s: &SandboxSpec, program: &Command) -> Vec<String> {
        systemd_wrapper(s, program)
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }
}
