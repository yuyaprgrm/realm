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
pub enum NetworkMode {
    /// No network access.
    None,
    /// Private network namespace (isolated from host).
    Private,
    /// Share host network namespace.
    Host,
}

impl NetworkMode {
    /// Whether the sandbox has any network access beyond loopback semantics.
    pub fn has_network(self) -> bool {
        matches!(self, NetworkMode::Private | NetworkMode::Host)
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
    pub network: NetworkMode,
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
    fn network_mode_flags() {
        assert!(!NetworkMode::None.has_network());
        assert!(NetworkMode::Private.has_network());
        assert!(NetworkMode::Host.has_network());
    }
}
