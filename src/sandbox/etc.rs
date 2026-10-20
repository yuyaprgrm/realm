use crate::error::Result;
use crate::sandbox::spec::{Mount, NetworkMode};
use std::path::{Path, PathBuf};

/// Generated sandbox-local `/etc` and shebang compatibility symlinks.
///
/// The host `/etc`, `/bin` and `/usr/bin` are never exposed. Instead a small
/// set of files is written into a private temp dir and bind-mounted read-only
/// into the sandbox. The temp dir must outlive `exec`, so it is owned here.
#[derive(Debug)]
pub struct GuestFs {
    root: tempfile::TempDir,
    network: NetworkMode,
    closure: Vec<String>,
    uid: u32,
    gid: u32,
}

impl GuestFs {
    pub fn new(network: NetworkMode, closure: Vec<String>, uid: u32, gid: u32) -> Result<Self> {
        Ok(Self {
            root: tempfile::tempdir()?,
            network,
            closure,
            uid,
            gid,
        })
    }

    fn write(&self, rel: &str, contents: &str) -> Result<()> {
        let p = self.root.path().join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(p, contents)?;
        Ok(())
    }

    /// Mounts implementing `/etc`, `/bin/sh` and `/usr/bin/env`.
    pub fn mounts(&self) -> Result<Vec<Mount>> {
        self.write_etc()?;
        self.write_compat_links()?;
        let root = self.root.path();
        let mut mounts = vec![
            Mount::ReadOnly {
                source: root.join("etc"),
                target: PathBuf::from("/etc"),
            },
            Mount::ReadOnly {
                source: root.join("bin"),
                target: PathBuf::from("/bin"),
            },
            Mount::ReadOnly {
                source: root.join("usr/bin"),
                target: PathBuf::from("/usr/bin"),
            },
        ];
        // CA certs only when networking is enabled and a CA store path exists.
        if self.network.has_network()
            && let Some(ca) = self.find_ca_bundle()
        {
            let ssl = root.join("etc/ssl/certs");
            self.write_symlink_file(&ssl, &ca)?;
            mounts.push(Mount::ReadOnly {
                source: root.join("etc/ssl"),
                target: PathBuf::from("/etc/ssl"),
            });
        }
        Ok(mounts)
    }

    fn write_etc(&self) -> Result<()> {
        // NSS uses these to resolve the sandbox user/group without touching
        // the host; `id`/`whoami` rely on them.
        self.write(
            "etc/passwd",
            &format!(
                "root:x:0:0:root:/root:/bin/sh\nsandbox:x:{uid}:{gid}:sandbox:/home/sandbox:/bin/sh\n",
                uid = self.uid,
                gid = self.gid,
            ),
        )?;
        self.write(
            "etc/group",
            &format!("root:x:0:\nsandbox:x:{gid}:\n", gid = self.gid,),
        )?;
        // Keep name resolution local; no host /etc/hosts is exposed.
        self.write(
            "etc/hosts",
            "127.0.0.1 localhost sandbox\n::1 localhost sandbox\n",
        )?;
        self.write("etc/hostname", "sandbox\n")?;
        // passwd/group must stay file-only so we never contact host NSS.
        // hosts may fall back to DNS, which is only reachable when enabled.
        self.write(
            "etc/nsswitch.conf",
            "passwd: files\ngroup: files\nshadow: files\nhosts: files dns\n",
        )?;
        if self.network.has_network() {
            let resolv_conf = match self.network {
                NetworkMode::Private => format!(
                    "nameserver {}\n",
                    crate::sandbox::network::PASTA_DNS_FORWARD
                ),
                NetworkMode::Host => host_nameservers(),
                NetworkMode::None => String::new(),
            };
            self.write("etc/resolv.conf", &resolv_conf)?;
        }
        Ok(())
    }

    /// `/bin/sh` and `/usr/bin/env` pointing at a shell/coreutils from the
    /// closure, so `#!/bin/sh` and `#!/usr/bin/env bash` scripts work without
    /// exposing the host `/bin` or `/usr/bin`.
    fn write_compat_links(&self) -> Result<()> {
        let bin = self.root.path().join("bin");
        let usr_bin = self.root.path().join("usr/bin");
        std::fs::create_dir_all(&bin)?;
        std::fs::create_dir_all(&usr_bin)?;
        if let Some(sh) = self.find_in_closure(&["/bin/bash", "/bin/sh"]) {
            std::os::unix::fs::symlink(&sh, bin.join("sh"))?;
        }
        if let Some(env) = self.find_in_closure(&["/bin/env"]) {
            std::os::unix::fs::symlink(&env, usr_bin.join("env"))?;
        }
        Ok(())
    }

    /// First closure path that contains any of `candidates` (e.g. `/bin/bash`).
    fn find_in_closure(&self, candidates: &[&str]) -> Option<PathBuf> {
        for p in &self.closure {
            for c in candidates {
                let full = Path::new(p).join(c.trim_start_matches('/'));
                if full.exists() {
                    return Some(full);
                }
            }
        }
        None
    }

    /// A CA bundle in the closure, if any (`.../etc/ssl/certs/ca-bundle.crt`).
    fn find_ca_bundle(&self) -> Option<PathBuf> {
        const CANDIDATES: &[&str] = &[
            "etc/ssl/certs/ca-bundle.crt",
            "etc/ssl/certs/ca-certificates.crt",
            "etc/ssl/certs/ca-bundle.trust.crt",
        ];
        self.closure.iter().map(PathBuf::from).find_map(|p| {
            CANDIDATES
                .iter()
                .map(|c| p.join(c))
                .find(|full| full.exists())
        })
    }

    fn write_symlink_file(&self, dir: &Path, target: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        std::os::unix::fs::symlink(target, dir.join("ca-certificates.crt"))?;
        Ok(())
    }
}

/// Copy only nameserver directives for Host mode, without exposing other host
/// resolver configuration such as search domains or options.
fn host_nameservers() -> String {
    std::fs::read_to_string("/etc/resolv.conf")
        .unwrap_or_default()
        .lines()
        .filter(|line| line.trim_start().starts_with("nameserver "))
        .map(|line| format!("{}\n", line.trim()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(mounts: &[Mount]) -> Vec<String> {
        mounts
            .iter()
            .map(|m| match m {
                Mount::ReadOnly { target, .. } => target.display().to_string(),
                other => panic!("unexpected mount {other:?}"),
            })
            .collect()
    }

    #[test]
    fn writes_host_free_etc() {
        let g = GuestFs::new(NetworkMode::None, vec![], 1000, 1000).unwrap();
        let mounts = g.mounts().unwrap();
        assert_eq!(targets(&mounts), vec!["/etc", "/bin", "/usr/bin"]);

        let passwd = std::fs::read_to_string(g.root.path().join("etc/passwd")).unwrap();
        assert!(passwd.contains("sandbox:x:1000:1000:"));
        // network disabled: no resolver, no CA certs
        assert!(!g.root.path().join("etc/resolv.conf").exists());
        assert!(!g.root.path().join("etc/ssl").exists());
    }

    #[test]
    fn links_shell_from_closure() {
        // fake a store entry containing bin/bash and bin/env
        let store = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(store.path().join("bin")).unwrap();
        std::fs::write(store.path().join("bin/bash"), "").unwrap();
        std::fs::write(store.path().join("bin/env"), "").unwrap();
        let closure = vec![store.path().display().to_string()];

        let g = GuestFs::new(NetworkMode::None, closure, 1000, 1000).unwrap();
        let _ = g.mounts().unwrap();
        assert_eq!(
            std::fs::read_link(g.root.path().join("bin/sh")).unwrap(),
            store.path().join("bin/bash")
        );
        assert_eq!(
            std::fs::read_link(g.root.path().join("usr/bin/env")).unwrap(),
            store.path().join("bin/env")
        );
    }

    #[test]
    fn no_resolv_conf_copied_or_created() {
        let g = GuestFs::new(NetworkMode::Private, vec![], 1000, 1000).unwrap();
        let _ = g.mounts().unwrap();
        // Private DNS uses pasta's synthetic forwarding address.
        assert_eq!(
            std::fs::read_to_string(g.root.path().join("etc/resolv.conf")).unwrap(),
            format!(
                "nameserver {}\n",
                crate::sandbox::network::PASTA_DNS_FORWARD
            )
        );
    }
}
