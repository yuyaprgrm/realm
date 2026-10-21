use crate::error::{Error, Result};
use crate::sandbox::bwrap;
use crate::sandbox::spec::{NetworkMode, SandboxSpec};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

/// How resource limits will be enforced for this launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LimitPlan {
    /// Transient systemd user scope: cgroup memory/CPU/pids/wall-clock limits.
    SystemdScope,
    /// `setrlimit` fallback plus an external wall-clock `timeout` wrapper.
    RlimitFallback,
}

/// Decide whether limits can be enforced. Security-relevant policies fail
/// closed instead of silently running with weaker limits.
fn select_limit_plan(spec: &SandboxSpec, systemd_available: bool) -> Result<LimitPlan> {
    if systemd_available {
        return Ok(LimitPlan::SystemdScope);
    }
    if spec.network == NetworkMode::Private {
        return Err(Error::Bwrap(
            "network mode \"private\" requires an available systemd user manager to enforce cgroup limits".into(),
        ));
    }
    if spec.limits.require_cgroup {
        return Err(Error::Bwrap(
            "cgroup resource limits are required (requireCgroup = true) but no systemd user manager is available".into(),
        ));
    }
    Ok(LimitPlan::RlimitFallback)
}

/// Run the sandbox, applying resource limits via a transient systemd scope
/// (`systemd-run --user --scope`) when available.
pub fn exec_with_limits(spec: &SandboxSpec) -> Result<()> {
    let bwrap = bwrap::build_command(spec)?;
    let plan = select_limit_plan(spec, systemd_user_scope_available())?;

    if spec.network == NetworkMode::Private {
        let pasta_path = crate::sandbox::network::require_pasta()?;
        let clean_bwrap = clean_bwrap_command(&bwrap)?;
        // pasta starts in the background and configures a user/network namespace
        // for its caller. Run it inside the scope, then have bwrap join that
        // namespace instead of creating a disconnected one of its own.
        let pasta = private_network_command(&pasta_path, &clean_bwrap);
        let mut cmd = systemd_wrapper(spec, &pasta);
        let err = cmd.exec();
        return Err(Error::Bwrap(format!("exec failed: {}", err)));
    }

    let mut cmd = match plan {
        LimitPlan::SystemdScope => {
            let clean_bwrap = clean_bwrap_command(&bwrap)?;
            systemd_wrapper(spec, &clean_bwrap)
        }
        LimitPlan::RlimitFallback => {
            let mut c = bwrap;
            apply_fallback_timeout(&mut c, spec)?;
            bwrap::apply_rlimits(&mut c, spec)?;
            c
        }
    };

    // exec() only returns on failure.
    let err = cmd.exec();
    Err(Error::Bwrap(format!("exec failed: {}", err)))
}

/// Execute bwrap with an empty environment. `build_command` already clears the
/// direct process environment; the wrapper paths need a helper because
/// systemd-run/pasta must first inherit the variables needed to reach the
/// systemd user manager (which are not safe to expose through /proc/1).
fn clean_bwrap_command(bwrap: &Command) -> Result<Command> {
    let env = which::which("env").map_err(|error| {
        Error::Bwrap(format!(
            "cannot sanitize the sandbox process environment: `env` was not found on PATH: {error}"
        ))
    })?;
    Ok(clean_bwrap_command_with(&env, bwrap))
}

fn clean_bwrap_command_with(env: &Path, bwrap: &Command) -> Command {
    let mut cmd = Command::new(env);
    cmd.arg("-i").arg("--");
    cmd.arg(bwrap.get_program());
    cmd.args(bwrap.get_args());
    cmd
}

/// Fallback wall-clock timeout when no systemd scope is available. `timeout`
/// runs outside bwrap and terminates the whole sandbox process tree.
fn apply_fallback_timeout(cmd: &mut Command, spec: &SandboxSpec) -> Result<()> {
    let Some(seconds) = spec.limits.timeout_secs else {
        return Ok(());
    };
    let timeout = which::which("timeout").map_err(|error| {
        Error::Bwrap(format!(
            "cannot enforce the configured wall-clock timeout without systemd: `timeout` was not found on PATH: {error}"
        ))
    })?;
    apply_fallback_timeout_with(&timeout, cmd, seconds);
    Ok(())
}

fn apply_fallback_timeout_with(timeout: &Path, cmd: &mut Command, seconds: u64) {
    let mut wrapped = Command::new(timeout);
    // The timeout process itself must not inherit host-only variables.
    wrapped.env_clear();
    wrapped
        .arg("--signal=TERM")
        .arg("--kill-after=5s")
        .arg(format!("{seconds}s"))
        .arg("--")
        .arg(cmd.get_program())
        .args(cmd.get_args());
    *cmd = wrapped;
}

fn private_network_command(pasta_path: &std::path::Path, bwrap: &Command) -> Command {
    let mut pasta = Command::new(pasta_path);
    pasta
        .arg("--foreground")
        .arg("--quiet")
        .arg("--config-net")
        .arg("--address")
        .arg(crate::sandbox::network::PASTA_ADDRESS)
        .arg("--gateway")
        .arg(crate::sandbox::network::PASTA_GATEWAY)
        .arg("--dns-forward")
        .arg(crate::sandbox::network::PASTA_DNS_FORWARD)
        .arg("--map-host-loopback")
        .arg("none")
        .arg("--tcp-ns")
        .arg("none")
        .arg("--udp-ns")
        .arg("none");
    pasta.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    let mut helper = Command::new("/bin/sh");
    helper.arg("-c").arg("exec \"$@\"").arg("realm-pasta");
    helper.arg(bwrap.get_program());
    helper.args(bwrap.get_args());
    pasta.arg(helper.get_program()).args(helper.get_args());
    pasta
}

/// Build the `systemd-run --user --scope ...` wrapper around a bwrap command.
fn systemd_wrapper(spec: &SandboxSpec, bwrap: &Command) -> Command {
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

    #[test]
    fn private_pasta_does_not_map_host_loopback() {
        let command =
            private_network_command(std::path::Path::new("/bin/pasta"), &Command::new("bwrap"));
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
    }

    fn wrapper_args_for(s: &SandboxSpec, program: &Command) -> Vec<String> {
        systemd_wrapper(s, program)
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn systemd_available_uses_the_cgroup_scope() {
        assert_eq!(
            select_limit_plan(&spec(), true).unwrap(),
            LimitPlan::SystemdScope
        );
    }

    #[test]
    fn missing_systemd_uses_the_documented_rlimit_fallback() {
        assert_eq!(
            select_limit_plan(&spec(), false).unwrap(),
            LimitPlan::RlimitFallback
        );
    }

    #[test]
    fn missing_systemd_fails_closed_for_private_network() {
        let mut s = spec();
        s.network = NetworkMode::Private;
        let error = select_limit_plan(&s, false).unwrap_err().to_string();
        assert!(error.contains("private"), "{error}");
        assert!(error.contains("systemd"), "{error}");
    }

    #[test]
    fn missing_systemd_fails_closed_when_cgroup_is_required() {
        let mut s = spec();
        s.limits.require_cgroup = true;
        let error = select_limit_plan(&s, false).unwrap_err().to_string();
        assert!(error.contains("requireCgroup"), "{error}");
    }

    #[test]
    fn wrapped_bwrap_is_executed_with_an_empty_environment() {
        let mut bwrap = Command::new("/usr/bin/bwrap");
        bwrap.arg("--unshare-pid");
        let clean = clean_bwrap_command_with(Path::new("/usr/bin/env"), &bwrap);
        let args = clean
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            vec!["-i", "--", "/usr/bin/bwrap", "--unshare-pid"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
        assert_eq!(clean.get_envs().count(), 0);
    }

    #[test]
    fn fallback_timeout_wraps_bwrap_without_inheriting_host_environment() {
        let mut bwrap = Command::new("/usr/bin/bwrap");
        bwrap.arg("--unshare-pid");
        apply_fallback_timeout_with(Path::new("/usr/bin/timeout"), &mut bwrap, 60);
        let args = bwrap
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
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
        assert_eq!(bwrap.get_envs().count(), 0);
    }
}
