pub mod closure;
pub mod eval;
pub mod realize;

use crate::error::Result;
use crate::nix::eval::SandboxCfg;
use crate::sandbox::spec::{BindMount, Mount, SandboxSpec};

pub fn build_spec(config: &std::path::Path, sandbox_name: &str) -> Result<SandboxSpec> {
    let config = if config.is_absolute() {
        config.to_path_buf()
    } else {
        std::env::current_dir()?.join(config)
    };
    let config = config.canonicalize().map_err(|error| {
        crate::error::Error::Nix(format!(
            "cannot read Realm configuration {}: {error}",
            config.display()
        ))
    })?;
    let sandbox_cfg = eval::eval_sandbox(&config, sandbox_name)?;
    realize::realize_packages(&sandbox_cfg.packages)?;
    let out_paths: Vec<String> = sandbox_cfg.packages.iter().map(|p| p.out.clone()).collect();
    let closure_paths = closure::get_closure(&out_paths)?;
    build_bwrap_spec(&sandbox_cfg, &closure_paths, std::env::current_dir()?)
}

fn build_bwrap_spec(
    cfg: &SandboxCfg,
    closure_paths: &std::collections::HashSet<String>,
    cwd: std::path::PathBuf,
) -> Result<SandboxSpec> {
    use crate::error::Error;
    use crate::sandbox::etc::GuestFs;
    use crate::sandbox::spec::{Mount, NetworkMode, ResourceLimits, SandboxSpec};
    use std::collections::HashSet;
    use std::path::PathBuf;

    let network = match cfg.network.as_deref() {
        None => NetworkMode::None,
        Some("none") => NetworkMode::None,
        Some("private") => NetworkMode::Private,
        Some("host") => NetworkMode::Host,
        Some(other) => {
            return Err(Error::Nix(format!(
                "unknown network mode \"{other}\"; allowed: none, private, host"
            )));
        }
    };
    let limits = ResourceLimits::default();
    let uid = 1000;
    let gid = 1000;

    let mut mounts = Vec::new();
    // dirs
    mounts.push(Mount::Dir {
        target: PathBuf::from("/nix"),
    });
    mounts.push(Mount::Dir {
        target: PathBuf::from("/nix/store"),
    });
    // bind closure paths readonly
    let mut seen = HashSet::new();
    for p in closure_paths {
        let pb = PathBuf::from(p);
        if seen.contains(p) {
            continue;
        }
        seen.insert(p.clone());
        mounts.push(Mount::ReadOnly {
            source: pb.clone(),
            target: pb,
        });
    }
    // sandbox-local /etc, /bin/sh and /usr/bin/env (host /etc,/bin,/usr never exposed)
    let guest = GuestFs::new(network, closure_paths.iter().cloned().collect(), uid, gid)?;
    mounts.extend(guest.mounts()?);
    // dirs
    mounts.push(Mount::Dir {
        target: PathBuf::from("/workspace"),
    });
    mounts.push(Mount::Dir {
        target: PathBuf::from("/tmp"),
    });
    mounts.push(Mount::Tmpfs {
        target: PathBuf::from("/tmp"),
    });
    mounts.push(Mount::Proc {
        target: PathBuf::from("/proc"),
    });
    mounts.push(Mount::Dev {
        target: PathBuf::from("/dev"),
    });
    // workspace rw
    mounts.push(Mount::ReadWrite {
        source: cwd.clone(),
        target: PathBuf::from("/workspace"),
    });

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

    mounts.push(Mount::Dir {
        target: PathBuf::from("/home"),
    });
    mounts.push(Mount::Dir {
        target: PathBuf::from("/home/sandbox"),
    });
    mounts.push(Mount::Tmpfs {
        target: PathBuf::from("/home/sandbox"),
    });
    mounts.extend(user_mounts(&cfg.mounts)?);

    Ok(SandboxSpec {
        mounts,
        env,
        cwd: PathBuf::from("/workspace"),
        path_bins,
        executable: exe,
        args: vec!["-i".to_string()],
        network,
        limits,
        uid,
        gid,
        keep_alive: vec![guest],
    })
}

// User mounts must not replace or nest under the built-in isolation trees.
const RESERVED_GUEST_PATHS: &[&str] = &[
    "/nix",
    "/etc",
    "/bin",
    "/usr",
    "/workspace",
    "/tmp",
    "/proc",
    "/dev",
    "/home",
];

fn normalize_declared_path(path: &std::path::Path, kind: &str) -> Result<std::path::PathBuf> {
    use crate::error::Error;
    use std::path::{Component, Path, PathBuf};

    if path.as_os_str().is_empty() {
        return Err(Error::Spec(format!("mount {kind} path must not be empty")));
    }
    if !path.is_absolute() {
        return Err(Error::Spec(format!(
            "mount {kind} path must be absolute: {}",
            path.display()
        )));
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => normalized.push("/"),
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(Error::Spec(format!(
                    "mount {kind} path must not contain \"..\": {}",
                    path.display()
                )));
            }
            Component::Prefix(_) => {
                return Err(Error::Spec(format!(
                    "mount {kind} path is not a Linux path: {}",
                    path.display()
                )));
            }
        }
    }
    if normalized == Path::new("/") {
        return Err(Error::Spec(format!(
            "mount {kind} path must not be the filesystem root"
        )));
    }
    Ok(normalized)
}

fn normalized_host_path(path: &std::path::Path) -> Result<std::path::PathBuf> {
    use crate::error::Error;

    let declared = normalize_declared_path(path, "host")?;
    let canonical = std::fs::canonicalize(&declared).map_err(|error| {
        Error::Spec(format!(
            "mount host path {} is not accessible: {error}",
            declared.display()
        ))
    })?;
    if canonical != declared {
        return Err(Error::Spec(format!(
            "mount host path {} resolves to {}; symlinks are not allowed",
            declared.display(),
            canonical.display()
        )));
    }
    Ok(canonical)
}

fn paths_overlap(a: &std::path::Path, b: &std::path::Path) -> bool {
    a == b || a.starts_with(b) || b.starts_with(a)
}

fn user_mounts(bindings: &[BindMount]) -> Result<Vec<Mount>> {
    use crate::error::Error;
    use std::collections::HashSet;
    use std::path::Path;

    let mut normalized = Vec::with_capacity(bindings.len());
    for binding in bindings {
        let host = normalized_host_path(&binding.host)?;
        let guest = normalize_declared_path(&binding.guest, "guest")?;
        for reserved in RESERVED_GUEST_PATHS {
            let reserved = Path::new(reserved);
            if paths_overlap(&guest, reserved) {
                return Err(Error::Spec(format!(
                    "mount guest path {} overlaps reserved sandbox path {}",
                    guest.display(),
                    reserved.display()
                )));
            }
        }
        normalized.push((host, guest, binding.read_only));
    }

    for (i, (_, guest, _)) in normalized.iter().enumerate() {
        for (_, other, _) in normalized.iter().skip(i + 1) {
            if paths_overlap(guest, other) {
                return Err(Error::Spec(format!(
                    "mount guest paths {} and {} overlap",
                    guest.display(),
                    other.display()
                )));
            }
        }
    }

    let mut mounts = Vec::new();
    let mut created_dirs = HashSet::new();
    for (host, guest, read_only) in normalized {
        let mut parents = guest.ancestors().skip(1).collect::<Vec<_>>();
        parents.reverse();
        for parent in parents {
            if parent == Path::new("/") {
                continue;
            }
            if created_dirs.insert(parent.to_path_buf()) {
                mounts.push(Mount::Dir {
                    target: parent.to_path_buf(),
                });
            }
        }
        mounts.push(if read_only {
            Mount::ReadOnly {
                source: host,
                target: guest,
            }
        } else {
            Mount::ReadWrite {
                source: host,
                target: guest,
            }
        });
    }
    Ok(mounts)
}

#[cfg(test)]
mod mount_tests {
    use super::*;
    use std::path::PathBuf;

    fn bind(host: &std::path::Path, guest: &str, read_only: bool) -> BindMount {
        BindMount {
            host: host.to_path_buf(),
            guest: PathBuf::from(guest),
            read_only,
        }
    }

    #[test]
    fn creates_guest_parents_and_preserves_modes() {
        let root = tempfile::tempdir().unwrap();
        let ro = root.path().join("ro");
        let rw = root.path().join("rw");
        std::fs::create_dir(&ro).unwrap();
        std::fs::create_dir(&rw).unwrap();

        let mounts = user_mounts(&[
            bind(&ro, "/sandbox/ro", true),
            bind(&rw, "/sandbox/rw", false),
        ])
        .unwrap();
        let args = mounts.iter().map(|m| format!("{m:?}")).collect::<Vec<_>>();
        assert_eq!(args.len(), 3, "{args:?}");
        assert!(
            matches!(mounts[0], Mount::Dir { ref target } if target == std::path::Path::new("/sandbox"))
        );
        assert!(
            matches!(mounts[1], Mount::ReadOnly { ref source, ref target } if source == &ro && target == std::path::Path::new("/sandbox/ro"))
        );
        assert!(
            matches!(mounts[2], Mount::ReadWrite { ref source, ref target } if source == &rw && target == std::path::Path::new("/sandbox/rw"))
        );
    }

    #[test]
    fn rejects_relative_and_traversal_paths() {
        let root = tempfile::tempdir().unwrap();
        for guest in ["relative", "/sandbox/../etc", "/", "/workspace/data"] {
            let error = user_mounts(&[bind(root.path(), guest, true)]).unwrap_err();
            assert!(error.to_string().contains("mount"), "{error}");
        }
        let error = user_mounts(&[BindMount {
            host: PathBuf::from("relative"),
            guest: PathBuf::from("/sandbox/data"),
            read_only: true,
        }])
        .unwrap_err();
        assert!(error.to_string().contains("mount"), "{error}");
    }

    #[test]
    fn rejects_duplicate_and_parent_child_guest_paths() {
        let root = tempfile::tempdir().unwrap();
        for guests in [
            ("/sandbox/a", "/sandbox/a"),
            ("/sandbox", "/sandbox/a"),
            ("/sandbox/a", "/sandbox/a/b"),
        ] {
            let error = user_mounts(&[
                bind(root.path(), guests.0, true),
                bind(root.path(), guests.1, false),
            ])
            .unwrap_err();
            assert!(error.to_string().contains("overlap"), "{error}");
        }
    }

    #[test]
    fn rejects_symlinked_host_paths() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real");
        let link = root.path().join("link");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let error = user_mounts(&[bind(&link, "/sandbox/data", true)]).unwrap_err();
        assert!(error.to_string().contains("symlink"), "{error}");
    }
}
