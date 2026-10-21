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
    if !out_paths.is_empty() && closure_paths.is_empty() {
        return Err(crate::error::Error::Nix(
            "package closure could not be read: nix path-info returned no /nix/store paths".into(),
        ));
    }

    // Workspace and configuration are separate trust decisions. The workspace
    // defaults to the current directory for compatibility, but can be moved
    // away from the configuration and/or mounted read-only.
    let cwd = std::env::current_dir()?.canonicalize()?;
    if cwd == std::path::Path::new("/") {
        return Err(crate::error::Error::Spec(
            "workspace must not be the filesystem root; run realm from a project directory or set workspace.host".into(),
        ));
    }
    let (workspace_host, workspace_read_only) = match &sandbox_cfg.workspace {
        Some(workspace) => {
            let host = match &workspace.host {
                Some(host) => normalized_host_path(host, "workspace")?,
                None => cwd,
            };
            if !host.is_dir() {
                return Err(crate::error::Error::Spec(format!(
                    "workspace host path {} is not a directory",
                    host.display()
                )));
            }
            (host, workspace.read_only)
        }
        None => (cwd, false),
    };
    build_bwrap_spec(
        &sandbox_cfg,
        &closure_paths,
        workspace_host,
        workspace_read_only,
        &config,
    )
}

fn build_bwrap_spec(
    cfg: &SandboxCfg,
    closure_paths: &std::collections::HashSet<String>,
    workspace_host: std::path::PathBuf,
    workspace_read_only: bool,
    config: &std::path::Path,
) -> Result<SandboxSpec> {
    use crate::error::Error;
    use crate::sandbox::etc::GuestFs;
    use crate::sandbox::spec::{Mount, NetworkMode, ResourceLimits, SandboxSpec};
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
    let limits = ResourceLimits {
        require_cgroup: cfg.require_cgroup,
        ..ResourceLimits::default()
    };
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
    for p in closure_paths {
        let pb = PathBuf::from(p);
        mounts.push(Mount::ReadOnly {
            source: pb.clone(),
            target: pb,
        });
    }
    // sandbox-local /etc, /bin/sh and /usr/bin/env (host /etc,/bin,/usr never exposed)
    let package_roots = cfg.packages.iter().map(|p| p.out.clone()).collect();
    let guest = GuestFs::new(network, package_roots, uid, gid)?;
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
    // Fresh procfs for the private PID namespace. Combined with clearing the
    // bwrap process environment, /proc/<pid>/environ cannot expose host state.
    mounts.push(Mount::Proc {
        target: PathBuf::from("/proc"),
    });
    mounts.push(Mount::Dev {
        target: PathBuf::from("/dev"),
    });
    // Workspace is intentionally writable by default, but the mode and host
    // path are explicit policy. The sandbox configuration file is a separate
    // trust input; when it lives inside the workspace, pin its parent
    // directory read-only so the agent can neither rewrite the file nor rename
    // the directory that holds it.
    mounts.push(if workspace_read_only {
        Mount::ReadOnly {
            source: workspace_host.clone(),
            target: PathBuf::from("/workspace"),
        }
    } else {
        Mount::ReadWrite {
            source: workspace_host.clone(),
            target: PathBuf::from("/workspace"),
        }
    });
    if !workspace_read_only {
        mounts.extend(config_guard_mount(&workspace_host, config));
    }

    // env
    let mut env = Vec::new();
    env.push(("HOME".to_string(), "/home/sandbox".to_string()));
    env.push(("TMPDIR".to_string(), "/tmp".to_string()));
    env.push(("PWD".to_string(), "/workspace".to_string()));
    // build PATH from the listed packages, then their propagated outputs
    let mut path_bins: Vec<PathBuf> = Vec::new();
    for pkg in &cfg.packages {
        path_bins.push(PathBuf::from(&pkg.out).join("bin"));
    }
    for group in &cfg.propagated {
        for out in group {
            path_bins.push(PathBuf::from(out).join("bin"));
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
    // Built-in mount targets act as mount boundaries: user mounts may nest
    // under them but must not replace or cover them.
    let reserved: Vec<PathBuf> = mounts.iter().map(|m| m.target().to_path_buf()).collect();
    mounts.extend(user_mounts(&cfg.mounts, &reserved)?);

    Ok(SandboxSpec {
        mounts,
        env,
        cwd: PathBuf::from("/workspace"),
        executable: exe,
        args: vec!["-i".to_string()],
        network,
        limits,
        uid,
        gid,
        keep_alive: vec![guest],
    })
}

/// Normalize a declared guest or host path as a path tree.
///
/// Redundant separators and `.` components disappear; `..` is resolved
/// lexically before any validation. Attempts to escape the filesystem root are
/// rejected. Guest paths stay purely lexical (they refer to the sandbox tree);
/// host paths are canonicalized separately below.
fn normalize_declared_path(path: &std::path::Path, kind: &str) -> Result<std::path::PathBuf> {
    use crate::error::Error;
    use std::path::{Component, Path, PathBuf};

    if path.as_os_str().is_empty() {
        return Err(Error::Spec(format!("{kind} path must not be empty")));
    }
    if !path.is_absolute() {
        return Err(Error::Spec(format!(
            "{kind} path must be absolute: {}",
            path.display()
        )));
    }
    let mut normalized = PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                if normalized == Path::new("/") {
                    return Err(Error::Spec(format!(
                        "{kind} path escapes the filesystem root: {}",
                        path.display()
                    )));
                }
                normalized.pop();
            }
            Component::Prefix(_) => {
                return Err(Error::Spec(format!(
                    "{kind} path is not a Linux path: {}",
                    path.display()
                )));
            }
        }
    }
    if normalized == Path::new("/") {
        return Err(Error::Spec(format!(
            "{kind} path must not be the filesystem root"
        )));
    }
    Ok(normalized)
}

fn normalized_host_path(path: &std::path::Path, kind: &str) -> Result<std::path::PathBuf> {
    use crate::error::Error;

    let declared = normalize_declared_path(path, kind)?;
    // Resolve the original path with normal OS semantics and compare it with
    // the lexical tree. This rejects symlinks even when a `..` component would
    // otherwise remove the symlink from the normalized path.
    let canonical = std::fs::canonicalize(path).map_err(|error| {
        Error::Spec(format!(
            "{kind} host path {} is not accessible: {error}",
            declared.display()
        ))
    })?;
    if canonical != declared {
        return Err(Error::Spec(format!(
            "{kind} host path {} resolves to {}; symlinks and non-lexical `..` traversal are not allowed",
            declared.display(),
            canonical.display()
        )));
    }
    Ok(canonical)
}

/// Pin the configuration's directory read-only when it is part of the
/// writable workspace. The directory becomes a mountpoint, so it cannot be
/// renamed, and the file cannot be rewritten. Ancestors between the workspace
/// and that directory are re-bound read-write on themselves: they stay
/// writable but also become mountpoints, so renaming one cannot move the
/// guard away. A final file-level overlay keeps the config itself read-only.
fn config_guard_mount(workspace_host: &std::path::Path, config: &std::path::Path) -> Vec<Mount> {
    use std::path::{Path, PathBuf};

    let Ok(relative) = config.strip_prefix(workspace_host) else {
        return Vec::new();
    };
    if relative.as_os_str().is_empty() || relative == Path::new(".") {
        return Vec::new();
    }
    let Some(parent) = config.parent() else {
        return Vec::new();
    };
    let relative_parent = relative.parent().unwrap_or(Path::new(""));
    let file_guard = Mount::ReadOnly {
        source: config.to_path_buf(),
        target: PathBuf::from("/workspace").join(relative),
    };
    // A config directly in the workspace root already sits below a mountpoint
    // (`/workspace`), so only the file needs the extra read-only overlay.
    if relative_parent.as_os_str().is_empty() {
        return vec![file_guard];
    }

    let components = relative_parent.components().collect::<Vec<_>>();
    let mut mounts = Vec::new();
    let mut ancestor = PathBuf::new();
    for component in &components[..components.len().saturating_sub(1)] {
        ancestor.push(component);
        mounts.push(Mount::ReadWrite {
            source: workspace_host.join(&ancestor),
            target: PathBuf::from("/workspace").join(&ancestor),
        });
    }
    mounts.push(Mount::ReadOnly {
        source: parent.to_path_buf(),
        target: PathBuf::from("/workspace").join(relative_parent),
    });
    mounts.push(file_guard);
    mounts
}

fn paths_overlap(a: &std::path::Path, b: &std::path::Path) -> bool {
    a == b || a.starts_with(b) || b.starts_with(a)
}

/// Build the mount list for user-declared bind mounts.
///
/// `reserved` holds the guest targets of the built-in sandbox mounts. A user
/// mount may sit under such a boundary (e.g. `/home/sandbox/.config/foo` when
/// `/home` and `/home/sandbox` are reserved) but must not replace the boundary
/// itself nor cover it with an ancestor path. Overlapping user mounts among
/// themselves stay rejected so parent/child mount intent is never ambiguous.
fn user_mounts(bindings: &[BindMount], reserved: &[std::path::PathBuf]) -> Result<Vec<Mount>> {
    use crate::error::Error;
    use std::collections::HashSet;
    use std::path::Path;

    let mut normalized = Vec::with_capacity(bindings.len());
    for binding in bindings {
        // Host and guest paths have separate responsibilities: the host side
        // is resolved against the real filesystem (no symlinks), while the
        // guest side is a purely lexical path in the sandbox mount tree.
        let host = normalized_host_path(&binding.host, "mount")?;
        let guest = normalize_declared_path(&binding.guest, "mount guest")?;
        for boundary in reserved {
            if guest == *boundary {
                return Err(Error::Spec(format!(
                    "mount guest path {} is already a reserved sandbox mount point",
                    guest.display()
                )));
            }
            if boundary.starts_with(&guest) {
                return Err(Error::Spec(format!(
                    "mount guest path {} would cover reserved sandbox path {}; mount below the reserved path instead",
                    guest.display(),
                    boundary.display()
                )));
            }
        }
        normalized.push((host, guest, binding.read_only));
    }

    for (i, (_, guest, _)) in normalized.iter().enumerate() {
        for (_, other, _) in normalized.iter().skip(i + 1) {
            if paths_overlap(guest, other) {
                return Err(Error::Spec(format!(
                    "mount guest paths {} and {} overlap (one is equal to, an ancestor of, or a descendant of the other)",
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

    /// Built-in mount targets as produced by `build_bwrap_spec` (closure
    /// paths excluded; they only add deeper `/nix/store/...` boundaries).
    fn reserved_targets() -> Vec<PathBuf> {
        [
            "/nix",
            "/nix/store",
            "/etc",
            "/bin",
            "/usr/bin",
            "/workspace",
            "/tmp",
            "/proc",
            "/dev",
            "/home",
            "/home/sandbox",
        ]
        .iter()
        .map(PathBuf::from)
        .collect()
    }

    #[test]
    fn creates_guest_parents_and_preserves_modes() {
        let root = tempfile::tempdir().unwrap();
        let ro = root.path().join("ro");
        let rw = root.path().join("rw");
        std::fs::create_dir(&ro).unwrap();
        std::fs::create_dir(&rw).unwrap();

        let mounts = user_mounts(
            &[
                bind(&ro, "/sandbox/ro", true),
                bind(&rw, "/sandbox/rw", false),
            ],
            &reserved_targets(),
        )
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
    fn allows_mounts_under_reserved_paths() {
        let root = tempfile::tempdir().unwrap();
        let cfg = root.path().join("cfg");
        let data = root.path().join("data");
        std::fs::create_dir(&cfg).unwrap();
        std::fs::create_dir(&data).unwrap();

        let mounts = user_mounts(
            &[
                bind(&cfg, "/home/sandbox/.config/probe", false),
                bind(&data, "/workspace/data", true),
            ],
            &reserved_targets(),
        )
        .unwrap();
        let targets = mounts.iter().map(Mount::target).collect::<Vec<_>>();
        for expected in [
            "/home",
            "/home/sandbox",
            "/home/sandbox/.config",
            "/home/sandbox/.config/probe",
            "/workspace",
            "/workspace/data",
        ] {
            assert!(
                targets.contains(&std::path::Path::new(expected)),
                "missing {expected} in {targets:?}"
            );
        }
        assert!(matches!(
            mounts.iter().find(|m| m.target() == std::path::Path::new("/home/sandbox/.config/probe")),
            Some(Mount::ReadWrite { source, target })
                if source == &cfg && target == std::path::Path::new("/home/sandbox/.config/probe")
        ));
        assert!(matches!(
            mounts.iter().find(|m| m.target() == std::path::Path::new("/workspace/data")),
            Some(Mount::ReadOnly { source, target })
                if source == &data && target == std::path::Path::new("/workspace/data")
        ));
    }

    #[test]
    fn rejects_mounts_replacing_reserved_paths() {
        let root = tempfile::tempdir().unwrap();
        for guest in [
            "/home",
            "/home/sandbox",
            "/tmp",
            "/workspace",
            "/etc",
            "/nix/store",
            "/usr",
        ] {
            let error =
                user_mounts(&[bind(root.path(), guest, true)], &reserved_targets()).unwrap_err();
            assert!(error.to_string().contains("reserved"), "{error}");
        }
    }

    #[test]
    fn rejects_relative_and_traversal_paths() {
        let root = tempfile::tempdir().unwrap();
        for guest in ["relative", "/sandbox/../etc", "/"] {
            let error =
                user_mounts(&[bind(root.path(), guest, true)], &reserved_targets()).unwrap_err();
            assert!(error.to_string().contains("mount"), "{error}");
        }
        let error = user_mounts(
            &[BindMount {
                host: PathBuf::from("relative"),
                guest: PathBuf::from("/sandbox/data"),
                read_only: true,
            }],
            &reserved_targets(),
        )
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
            let error = user_mounts(
                &[
                    bind(root.path(), guests.0, true),
                    bind(root.path(), guests.1, false),
                ],
                &reserved_targets(),
            )
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

        let error =
            user_mounts(&[bind(&link, "/sandbox/data", true)], &reserved_targets()).unwrap_err();
        assert!(error.to_string().contains("symlink"), "{error}");

        // A `..` must not hide a preceding symlink from validation.
        let nested = root.path().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir(root.path().join("a/x")).unwrap();
        let link = root.path().join("link2");
        std::os::unix::fs::symlink(&nested, &link).unwrap();
        let error = user_mounts(
            &[bind(&root.path().join("link2/../x"), "/sandbox/data", true)],
            &reserved_targets(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("symlink"), "{error}");
    }

    #[test]
    fn normalizes_guest_paths_before_validation() {
        let root = tempfile::tempdir().unwrap();
        let mounts = user_mounts(
            &[bind(root.path(), "/sandbox/./a//b/../c", true)],
            &reserved_targets(),
        )
        .unwrap();
        let targets = mounts.iter().map(Mount::target).collect::<Vec<_>>();
        assert!(targets.contains(&std::path::Path::new("/sandbox/a/c")));
        assert!(!targets.contains(&std::path::Path::new("/sandbox/b")));

        // Normalization happens before overlap detection.
        let error = user_mounts(
            &[
                bind(root.path(), "/sandbox/a/./x", true),
                bind(root.path(), "/sandbox/a/x//y/..", false),
            ],
            &reserved_targets(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("overlap"), "{error}");
    }

    #[test]
    fn normalized_paths_cannot_escape_the_root() {
        let root = tempfile::tempdir().unwrap();
        for guest in ["/../etc", "/sandbox/../../etc"] {
            let error =
                user_mounts(&[bind(root.path(), guest, true)], &reserved_targets()).unwrap_err();
            assert!(error.to_string().contains("escapes"), "{error}");
        }
    }

    #[test]
    fn rejection_messages_explain_the_conflict() {
        let root = tempfile::tempdir().unwrap();
        let error = user_mounts(&[bind(root.path(), "/home", true)], &reserved_targets())
            .unwrap_err()
            .to_string();
        assert!(error.contains("/home"), "{error}");
        assert!(error.contains("reserved"), "{error}");

        let error = user_mounts(
            &[bind(root.path(), "/sandbox", true)],
            &[PathBuf::from("/sandbox/child")],
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("/sandbox/child"), "{error}");
        assert!(error.contains("cover"), "{error}");
    }

    #[test]
    fn configuration_inside_a_writable_workspace_pins_its_directory() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        let config = workspace.join("nix/realm.nix");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config, "").unwrap();

        let guard = config_guard_mount(&workspace, &config);
        assert_eq!(guard.len(), 2);
        assert!(matches!(
            &guard[0],
            Mount::ReadOnly { source, target }
                if source == config.parent().unwrap()
                    && target == std::path::Path::new("/workspace/nix")
        ));
        assert!(matches!(
            &guard[1],
            Mount::ReadOnly { source, target }
                if source == &config
                    && target == std::path::Path::new("/workspace/nix/realm.nix")
        ));

        let deep = workspace.join("a/b/c/realm.nix");
        std::fs::create_dir_all(deep.parent().unwrap()).unwrap();
        std::fs::write(&deep, "").unwrap();
        let guard = config_guard_mount(&workspace, &deep);
        assert_eq!(guard.len(), 4);
        assert!(matches!(
            &guard[0],
            Mount::ReadWrite { source, target }
                if source == &workspace.join("a") && target == std::path::Path::new("/workspace/a")
        ));
        assert!(matches!(
            &guard[1],
            Mount::ReadWrite { source, target }
                if source == &workspace.join("a/b")
                    && target == std::path::Path::new("/workspace/a/b")
        ));
        assert!(matches!(
            &guard[2],
            Mount::ReadOnly { source, target }
                if source == &workspace.join("a/b/c")
                    && target == std::path::Path::new("/workspace/a/b/c")
        ));

        let direct = workspace.join("realm.nix");
        std::fs::write(&direct, "").unwrap();
        let guard = config_guard_mount(&workspace, &direct);
        assert_eq!(guard.len(), 1);
        assert!(matches!(
            &guard[0],
            Mount::ReadOnly { source, target }
                if source == &direct && target == std::path::Path::new("/workspace/realm.nix")
        ));

        assert!(config_guard_mount(&workspace, &root.path().join("other.nix")).is_empty());
    }
}
