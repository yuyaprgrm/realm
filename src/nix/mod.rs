pub mod closure;
pub mod eval;
pub mod realize;

use crate::error::Result;
use crate::nix::eval::SandboxCfg;
use crate::sandbox::spec::SandboxSpec;

pub fn build_spec(flake_ref: &str) -> Result<SandboxSpec> {
    let (flake_dir, attr) = parse_flake_ref(flake_ref)?;
    let system = current_system()?;
    let sandbox_cfg = eval::eval_sandbox(&flake_dir, &system, &attr)?;
    realize::realize_packages(&sandbox_cfg.packages)?;
    let out_paths: Vec<String> = sandbox_cfg.packages.iter().map(|p| p.out.clone()).collect();
    let closure_paths = closure::get_closure(&out_paths)?;
    Ok(build_bwrap_spec(
        &sandbox_cfg,
        &closure_paths,
        std::env::current_dir()?,
    )?)
}

fn parse_flake_ref(flake_ref: &str) -> Result<(std::path::PathBuf, String)> {
    if let Some((left, right)) = flake_ref.split_once('#') {
        let dir = if left.is_empty() {
            ".".to_string()
        } else if left == "." {
            ".".to_string()
        } else {
            // allow path/to/flake or ./path/to/flake
            left.to_string()
        };
        let p = std::path::PathBuf::from(dir);
        Ok((p.canonicalize().unwrap_or(p), right.to_string()))
    } else {
        Err(crate::error::Error::Nix("flake ref must be <flake>#<name>".into()))
    }
}

fn current_system() -> Result<String> {
    let out = std::process::Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command flakes",
            "eval",
            "--raw",
            "--expr",
            "builtins.currentSystem",
        ])
        .output()
        .map_err(|e| crate::error::Error::Nix(e.to_string()))?;
    if !out.status.success() {
        // try fallback
        let out2 = std::process::Command::new("uname")
            .args(["-m"])
            .output()
            .map_err(|e| crate::error::Error::Nix(e.to_string()))?;
        if !out2.status.success() {
            return Err(crate::error::Error::Nix(String::from_utf8_lossy(&out.stderr).into()));
        }
        let m = String::from_utf8(out2.stdout).unwrap_or_default().trim().to_string();
        let sys = if m == "x86_64" {
            "x86_64-linux".to_string()
        } else if m == "aarch64" {
            "aarch64-linux".to_string()
        } else {
            return Err(crate::error::Error::Nix("unknown arch".into()));
        };
        return Ok(sys);
    }
    Ok(String::from_utf8(out.stdout).map_err(|e| crate::error::Error::Nix(e.to_string()))?)
}

fn build_bwrap_spec(
    cfg: &SandboxCfg,
    closure_paths: &std::collections::HashSet<String>,
    cwd: std::path::PathBuf,
) -> Result<SandboxSpec> {
    use crate::sandbox::etc::GuestFs;
    use crate::sandbox::spec::{Mount, NetworkPolicy, SandboxSpec};
    use std::collections::HashSet;
    use std::path::PathBuf;

    let network = NetworkPolicy::Disabled;
    let uid = 1000;
    let gid = 1000;

    let mut mounts = Vec::new();
    // dirs
    mounts.push(Mount::Dir { target: PathBuf::from("/nix") });
    mounts.push(Mount::Dir { target: PathBuf::from("/nix/store") });
    // bind closure paths readonly
    let mut seen = HashSet::new();
    for p in closure_paths {
        let pb = PathBuf::from(p);
        if seen.contains(p) { continue; }
        seen.insert(p.clone());
        mounts.push(Mount::ReadOnly { source: pb.clone(), target: pb });
    }
    // sandbox-local /etc, /bin/sh and /usr/bin/env (host /etc,/bin,/usr never exposed)
    let guest = GuestFs::new(network.clone(), closure_paths.iter().cloned().collect(), uid, gid)?;
    mounts.extend(guest.mounts()?);
    // dirs
    mounts.push(Mount::Dir { target: PathBuf::from("/workspace") });
    mounts.push(Mount::Dir { target: PathBuf::from("/tmp") });
    mounts.push(Mount::Tmpfs { target: PathBuf::from("/tmp") });
    mounts.push(Mount::Proc { target: PathBuf::from("/proc") });
    mounts.push(Mount::Dev { target: PathBuf::from("/dev") });
    // workspace rw
    mounts.push(Mount::ReadWrite { source: cwd.clone(), target: PathBuf::from("/workspace") });

    // env
    let mut env = Vec::new();
    env.push(("HOME".to_string(), "/home/sandbox".to_string()));
    env.push(("TMPDIR".to_string(), "/tmp".to_string()));
    env.push(("PWD".to_string(), "/workspace".to_string()));
    // build PATH from top-level package bin dirs
    let mut path_bins: Vec<PathBuf> = Vec::new();
    for pkg in &cfg.packages {
        let b = PathBuf::from(&pkg.out).join("bin");
        if b.exists() {
            path_bins.push(b);
        } else {
            // still allow adding if it might exist; bwrap will fail later if not critical - but better be strict? MVP be lenient
            path_bins.push(b);
        }
    }
    let path_str = path_bins
        .iter()
        .map(|p| p.to_string_lossy())
        .collect::<Vec<_>>()
        .join(":");
    env.push(("PATH".to_string(), path_str));
    // user env overrides
    for (k, v) in &cfg.env {
        env.push((k.clone(), v.clone()));
    }

    // default shell: bash from a package if available, else the guest /bin/sh
    let mut exe = PathBuf::from("/bin/sh");
    for pkg in &cfg.packages {
        let bash = PathBuf::from(&pkg.out).join("bin").join("bash");
        if bash.exists() {
            exe = bash;
            break;
        }
    }

    mounts.push(Mount::Dir { target: PathBuf::from("/home") });
    mounts.push(Mount::Dir { target: PathBuf::from("/home/sandbox") });
    mounts.push(Mount::Tmpfs { target: PathBuf::from("/home/sandbox") });

    Ok(SandboxSpec {
        mounts,
        env,
        cwd: PathBuf::from("/workspace"),
        path_bins,
        executable: exe,
        args: vec!["-i".to_string()],
        network,
        uid,
        gid,
        keep_alive: vec![guest],
    })
}
