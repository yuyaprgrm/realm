use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum Mount {
    ReadOnly { source: PathBuf, target: PathBuf },
    ReadWrite { source: PathBuf, target: PathBuf },
    Tmpfs { target: PathBuf },
    Proc { target: PathBuf },
    Dev { target: PathBuf },
    Dir { target: PathBuf },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkPolicy {
    /// No network namespace at all; fully offline (`--unshare-net`).
    Disabled,
    /// A private network namespace with loopback only; no host access.
    Isolated,
    /// A private network namespace that will be attached to an external
    /// bridge/NAT (managed by the host, e.g. NixOS). Until attached, this
    /// behaves like `Isolated`; the host must provide the veth/NAT path.
    Internet,
}

impl NetworkPolicy {
    /// Whether the sandbox may reach anything beyond an empty namespace.
    /// `Disabled` is fully offline; `Isolated`/`Internet` need resolver and
    /// CA files prepared for a network stack to be usable.
    pub fn has_network(self) -> bool {
        !matches!(self, NetworkPolicy::Disabled)
    }
}

/// Resource limits applied to the sandbox process tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceLimits {
    /// Total memory cap in bytes (cgroup `MemoryMax`).
    pub memory_bytes: u64,
    /// CPU quota as a percentage of one core (cgroup `CPUQuota`).
    pub cpu_percent: u32,
    /// Max number of processes/threads (cgroup `TasksMax`, RLIMIT_NPROC).
    pub max_pids: u32,
    /// Wall-clock timeout in seconds; `None` means no timeout.
    pub timeout_secs: Option<u64>,
    /// Max size in bytes of the writable `/tmp` tmpfs.
    pub tmpfs_bytes: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            memory_bytes: 2 * 1024 * 1024 * 1024, // 2 GiB
            cpu_percent: 100,                     // one core
            max_pids: 512,
            timeout_secs: None,
            tmpfs_bytes: 512 * 1024 * 1024, // 512 MiB
        }
    }
}

#[derive(Debug)]
pub struct SandboxSpec {
    pub mounts: Vec<Mount>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    pub path_bins: Vec<PathBuf>,
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub network: NetworkPolicy,
    pub limits: ResourceLimits,
    pub uid: u32,
    pub gid: u32,
    /// Owns the generated guest `/etc` and compatibility symlink tree so the
    /// files stay on disk until `exec` has bound them.
    pub keep_alive: Vec<crate::sandbox::etc::GuestFs>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_limits_are_sane() {
        let l = ResourceLimits::default();
        assert_eq!(l.memory_bytes, 2 * 1024 * 1024 * 1024);
        assert_eq!(l.max_pids, 512);
        assert_eq!(l.tmpfs_bytes, 512 * 1024 * 1024);
        assert!(l.timeout_secs.is_none());
    }

    #[test]
    fn network_policy_online_flag() {
        assert!(!NetworkPolicy::Disabled.has_network());
        assert!(NetworkPolicy::Isolated.has_network());
        assert!(NetworkPolicy::Internet.has_network());
    }
}
