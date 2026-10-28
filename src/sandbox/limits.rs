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
use std::collections::VecDeque;
use std::path::Path;
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

/// Process group of the running sandbox, as created by bwrap's
/// `--new-session`. That flag detaches the sandbox from the host terminal's
/// session, so the kernel no longer delivers SIGWINCH to it; Realm forwards
/// the signal to the whole group instead. Zero means no sandbox is running or
/// the group could not be resolved.
static SANDBOX_PGID: AtomicI32 = AtomicI32::new(0);

extern "C" fn forward_sigwinch(_signal: libc::c_int) {
    let pgid = SANDBOX_PGID.load(Ordering::Relaxed);
    if pgid > 0 {
        // Signal handlers must stay async-signal-safe: one atomic load and
        // kill(2), nothing else. No /proc walking, allocation or string
        // handling. If the group is already gone (ESRCH, sandbox exit race)
        // there is nothing to deliver; do not retry, just return.
        unsafe {
            libc::kill(-pgid, libc::SIGWINCH);
        }
    }
}

/// The fields of /proc/<pid>/stat that the group resolution needs.
struct ProcStat {
    state: u8,
    ppid: i32,
    pgrp: i32,
    sid: i32,
}

fn read_stat(pid: i32) -> Option<ProcStat> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close = stat.rfind(')')?;
    // After the (possibly space-containing) comm: state, ppid, pgrp, session.
    let fields: Vec<&str> = stat[close + 1..].split_whitespace().collect();
    Some(ProcStat {
        state: fields.first()?.bytes().next()?,
        ppid: fields.get(1)?.parse().ok()?,
        pgrp: fields.get(2)?.parse().ok()?,
        sid: fields.get(3)?.parse().ok()?,
    })
}

/// Snapshot of the process table: (pid, stat) pairs.
fn scan_procs() -> Vec<(i32, ProcStat)> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return vec![];
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_string_lossy().parse::<i32>().ok()?;
            if pid <= 0 {
                return None;
            }
            let stat = read_stat(pid)?;
            Some((pid, stat))
        })
        .collect()
}

/// Find the PGID of the process group bwrap created via `--new-session`.
///
/// bwrap calls setsid() in the clone child that becomes the sandbox (the PID-1
/// reaper) and then forks the command into the same group, so every sandboxed
/// process shares that group. The spawned process itself — bwrap's monitor, or
/// a wrapper such as pasta or the rlimit timeout backend — stays in Realm's
/// own process group and must never be selected: killing that group would
/// signal Realm too. So return the shallowest descendant that leads both its
/// own session and its own process group. pasta does not call setsid in the
/// command child and timeout only does setpgid (same session), so neither can
/// shadow the bwrap child. A sandbox process that later creates its own
/// session is always deeper than the bwrap child and does not shadow it either
/// — but note it then leaves the forwarded group and stops receiving SIGWINCH.
/// A job-control shell would only claim a group via tcsetpgrp on a controlling
/// terminal, and --new-session leaves the sandbox with none.
fn find_sandbox_pgid(root: i32, procs: &[(i32, ProcStat)]) -> Option<i32> {
    let mut queue = VecDeque::from([root]);
    let mut seen = vec![root];
    while let Some(pid) = queue.pop_front() {
        for (child, stat) in procs {
            if stat.ppid != pid || seen.contains(child) {
                continue;
            }
            if stat.pgrp == *child && stat.sid == *child {
                return Some(stat.pgrp);
            }
            seen.push(*child);
            queue.push_back(*child);
        }
    }
    None
}

/// How long to poll for bwrap's setsid child before giving up on forwarding.
/// A started sandbox resolves in milliseconds; the timeout only matters when
/// bwrap hangs during setup.
const PGID_RESOLVE_TIMEOUT: Duration = Duration::from_secs(2);

/// Resolve the sandbox process group once, right after spawn. This runs in
/// normal context (not in the signal handler), so /proc access is fine here.
/// Zero means "could not resolve": SIGWINCH is then simply not forwarded.
fn resolve_sandbox_pgid(root_pid: i32) -> i32 {
    let deadline = Instant::now() + PGID_RESOLVE_TIMEOUT;
    loop {
        if let Some(pgid) = find_sandbox_pgid(root_pid, &scan_procs()) {
            return pgid;
        }
        // bwrap gone (crashed or exited before we could resolve): wait()
        // reports the failure, and there is nothing left to forward to.
        if read_stat(root_pid).is_none_or(|stat| stat.state == b'Z') {
            return 0;
        }
        if Instant::now() >= deadline {
            // shortcut: bwrap's setsid child never showed up within the
            // timeout; skip forwarding instead of delaying startup further.
            return 0;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Run the sandbox, applying the configured resource limits.
pub fn exec(spec: &SandboxSpec, spinner: &mut crate::ui::Spinner) -> Result<ExitStatus> {
    let command = sandbox_command(spec)?;
    let mut command = wrap(spec, command)?;
    // The sandboxed process writes to the terminal from here on, so the
    // spinner must be gone before the process starts.
    spinner.stop();
    let mut child = command
        .spawn()
        .map_err(|error| Error::Bwrap(format!("spawn failed: {error}")))?;
    SANDBOX_PGID.store(resolve_sandbox_pgid(child.id() as i32), Ordering::Relaxed);
    // bwrap is always started with --new-session (see bwrap.rs), so there is
    // no path where the sandbox shares Realm's process group. Should one ever
    // be added, don't install this forwarder for it: the kernel delivers
    // SIGWINCH to the shared foreground group, and a group kill from here
    // would signal Realm itself.
    unsafe {
        libc::signal(
            libc::SIGWINCH,
            forward_sigwinch as *const () as libc::sighandler_t,
        );
    }
    let status = child.wait();
    SANDBOX_PGID.store(0, Ordering::Relaxed);
    unsafe {
        libc::signal(libc::SIGWINCH, libc::SIG_DFL);
    }
    status.map_err(|error| Error::Bwrap(format!("wait failed: {error}")))
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

    fn stat(pid: i32, ppid: i32, pgrp: i32, sid: i32) -> (i32, ProcStat) {
        (pid, ProcStat { state: b'S', ppid, pgrp, sid })
    }

    #[test]
    fn the_forwarded_group_is_the_descendant_session_leader() {
        // pasta/timeout -> bwrap monitor -> setsid child (reaper) -> command,
        // all identified by (pid, ppid, pgrp, sid).
        let procs = [
            stat(80, 1, 80, 80),    // pasta, Realm's own group
            stat(81, 80, 80, 80),   // bwrap monitor, still in Realm's group
            stat(82, 81, 82, 82),   // bwrap's setsid child = the sandbox group
            stat(83, 82, 82, 82),   // the command, in that group
            stat(84, 83, 84, 84),   // a later daemon with its own session
        ];
        assert_eq!(find_sandbox_pgid(80, &procs), Some(82));
    }

    #[test]
    fn a_new_process_group_within_the_same_session_is_not_the_group() {
        // GNU timeout's child: setpgid(0,0) but no setsid.
        let procs = [
            stat(80, 1, 80, 80),    // timeout, Realm's group
            stat(81, 80, 81, 80),   // bwrap monitor in its own group, same session
            stat(82, 81, 82, 82),   // bwrap's setsid child = the sandbox group
        ];
        assert_eq!(find_sandbox_pgid(80, &procs), Some(82));
    }

    #[test]
    fn a_tree_without_its_own_session_yields_no_group() {
        // A --new-session-less path would look like this; no group is chosen,
        // so the forwarder stays silent and the kernel delivers SIGWINCH itself.
        let procs = [
            stat(80, 1, 80, 80),
            stat(81, 80, 80, 80),
            stat(82, 81, 80, 80),
        ];
        assert_eq!(find_sandbox_pgid(80, &procs), None);
    }
}
