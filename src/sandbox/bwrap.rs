use std::os::unix::process::CommandExt;
use std::process::Command;

use crate::error::{Error, Result};
use crate::sandbox::spec::{Mount, NetworkPolicy, SandboxSpec};

pub fn exec(spec: &SandboxSpec) -> Result<()> {
    let bwrap = which::which("bwrap").map_err(|e| Error::Bwrap(e.to_string()))?;

    let mut cmd = Command::new(&bwrap);
    // isolation
    cmd.arg("--unshare-user")
        .arg("--unshare-pid")
        .arg("--unshare-uts")
        .arg("--unshare-ipc")
        .arg("--die-with-parent");

    match spec.network {
        NetworkPolicy::Disabled => {
            cmd.arg("--unshare-net");
        }
        NetworkPolicy::Enabled => {
            // share net with host (MVP)
        }
    }

    cmd.arg("--clearenv");

    // env
    for (k, v) in &spec.env {
        cmd.arg("--setenv").arg(k).arg(v);
    }

    // mounts - ensure parents exist via bwrap where helpful? bwrap handles most; we build dirs
    for m in &spec.mounts {
        match m {
            Mount::Dir { target } => {
                cmd.arg("--dir").arg(target);
            }
            Mount::Tmpfs { target } => {
                cmd.arg("--tmpfs").arg(target);
            }
            Mount::Proc { target } => {
                cmd.arg("--proc").arg(target);
            }
            Mount::Dev { target } => {
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

    // ensure we don't pass extras
    let err = cmd.exec();
    Err(Error::Bwrap(format!("exec failed: {}", err)))
}
