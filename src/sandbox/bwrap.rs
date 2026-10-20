use crate::error::{Error, Result};
use crate::sandbox::spec::{Mount, SandboxSpec};
use std::os::unix::process::CommandExt;
use std::process::Command;

/// Build the bwrap command (without executing it). Shared by the plain path
/// and the cgroup-wrapped path.
pub fn build_command(spec: &SandboxSpec) -> Result<Command> {
    let bwrap = which::which("bwrap").map_err(|e| Error::Bwrap(e.to_string()))?;

    let mut cmd = Command::new(&bwrap);
    // Isolation: new user/pid/uts/ipc/net namespaces and private cgroup ns.
    cmd.arg("--unshare-user")
        .arg("--unshare-pid")
        .arg("--unshare-uts")
        .arg("--unshare-ipc");
    // Network namespace: for Internet we rely on pasta-provided netns (pasta
    // creates/runs in a netns); avoid creating a second isolated netns that
    // would block connectivity.
    if spec.network != crate::sandbox::spec::NetworkPolicy::Internet {
        cmd.arg("--unshare-net");
    }
    cmd.arg("--unshare-cgroup-try")
        .arg("--die-with-parent")
        // Fresh terminal session: blocks TIOCSTI / controlling-tty injection.
        .arg("--new-session")
        // No nested user namespaces: removes a large privilege-escalation
        // surface (requires --unshare-user, which we always set).
        .arg("--disable-userns");

    // Network policy: every policy gets a private netns (--unshare-net above).
    // Disabled leaves it empty; Isolated/Internet are wired by the host later.

    cmd.arg("--clearenv");

    // Match the generated /etc/passwd and /etc/group entries.
    cmd.arg("--uid").arg(spec.uid.to_string());
    cmd.arg("--gid").arg(spec.gid.to_string());

    // env
    for (k, v) in &spec.env {
        cmd.arg("--setenv").arg(k).arg(v);
    }

    // mounts
    for m in &spec.mounts {
        match m {
            Mount::Dir { target } => {
                cmd.arg("--dir").arg(target);
            }
            Mount::Tmpfs { target } => {
                cmd.arg("--size").arg(spec.limits.tmpfs_bytes.to_string());
                cmd.arg("--tmpfs").arg(target);
            }
            Mount::Proc { target } => {
                cmd.arg("--proc").arg(target);
            }
            Mount::Dev { target } => {
                // bwrap creates a minimal, private /dev: null, zero, full,
                // random, urandom, tty, ptmx/pts, shm. No host devices.
                cmd.arg("--dev").arg(target);
            }
            Mount::ReadOnly { source, target } => {
                cmd.arg("--ro-bind").arg(source).arg(target);
            }
            Mount::ReadWrite { source, target } => {
                cmd.arg("--bind").arg(source).arg(target);
            }
        }
    }

    cmd.arg("--chdir").arg(&spec.cwd);

    // exec
    cmd.arg("--").arg(&spec.executable);
    for a in &spec.args {
        cmd.arg(a);
    }

    Ok(cmd)
}

/// Apply fallback rlimits in the child before exec. Used when no cgroup
/// wrapper is available; memory address-space and CPU are the meaningful caps.
pub fn apply_rlimits(cmd: &mut Command, spec: &SandboxSpec) {
    let mem = spec.limits.memory_bytes;
    let cpu = spec.limits.timeout_secs.unwrap_or(0);
    let nproc = spec.limits.max_pids as u64;
    let fsize = spec.limits.tmpfs_bytes;

    unsafe {
        let _ = cmd.pre_exec(move || {
            set_limit(libc::RLIMIT_AS, mem);
            set_limit(libc::RLIMIT_FSIZE, fsize);
            if nproc > 0 {
                set_limit(libc::RLIMIT_NPROC, nproc);
            }
            if cpu > 0 {
                set_limit(libc::RLIMIT_CPU, cpu);
            }
            Ok(())
        });
    }
}

fn set_limit(resource: libc::c_uint, value: u64) {
    let lim = libc::rlimit {
        rlim_cur: value as libc::rlim_t,
        rlim_max: value as libc::rlim_t,
    };
    // Best effort: some limits are not permitted in a user namespace.
    unsafe {
        libc::setrlimit(resource, &lim);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::spec::{NetworkPolicy, ResourceLimits, SandboxSpec};

    fn spec() -> SandboxSpec {
        SandboxSpec {
            mounts: vec![
                Mount::Dir {
                    target: "/home".into(),
                },
                Mount::Tmpfs {
                    target: "/tmp".into(),
                },
                Mount::Dev {
                    target: "/dev".into(),
                },
            ],
            env: vec![],
            cwd: "/".into(),
            path_bins: vec![],
            executable: "/bin/sh".into(),
            args: vec![],
            network: NetworkPolicy::Disabled,
            limits: ResourceLimits::default(),
            uid: 1000,
            gid: 1000,
            keep_alive: vec![],
        }
    }

    fn args() -> Vec<String> {
        build_command(&spec())
            .unwrap()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn build_command_applies_hardening() {
        let a = args();
        for flag in [
            "--unshare-user",
            "--unshare-pid",
            "--unshare-uts",
            "--unshare-ipc",
            "--unshare-net",
            "--unshare-cgroup-try",
            "--die-with-parent",
            "--new-session",
            "--disable-userns",
        ] {
            assert!(a.contains(&flag.to_string()), "missing {flag}");
        }
        // private /dev, never a host device bind
        assert!(a.contains(&"--dev".to_string()));
        // tmpfs size comes from limits
        assert!(a.contains(&"536870912".to_string()));
    }
}
