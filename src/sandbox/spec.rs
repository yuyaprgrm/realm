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

impl Mount {
    /// Guest path where this mount is attached.
    pub fn target(&self) -> &std::path::Path {
        match self {
            Mount::ReadOnly { target, .. }
            | Mount::ReadWrite { target, .. }
            | Mount::Tmpfs { target }
            | Mount::Proc { target }
            | Mount::Dev { target }
            | Mount::Dir { target } => target,
        }
    }
}

/// Explicit host path bind mounted into the sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindMount {
    pub host: PathBuf,
    pub guest: PathBuf,
    pub read_only: bool,
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

/// Mechanism that enforces the resource limits of a sandbox.
///
/// The backend is selected explicitly. Only the selected backend is used:
/// there is no fallback to a weaker mechanism, and a backend that cannot be
/// applied fails the launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceBackend {
    /// cgroup v2 limits applied through a transient systemd user scope.
    Cgroup,
    /// `setrlimit` limits plus a coreutils `timeout` wrapper.
    Rlimit,
}

impl ResourceBackend {
    /// Configuration name of the backend.
    pub fn name(self) -> &'static str {
        match self {
            ResourceBackend::Cgroup => "cgroup",
            ResourceBackend::Rlimit => "rlimit",
        }
    }

    /// Parse a configured backend name.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "cgroup" => Some(ResourceBackend::Cgroup),
            "rlimit" => Some(ResourceBackend::Rlimit),
            _ => None,
        }
    }
}

/// Resource limits applied to the sandbox process tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceLimits {
    /// Apply the resource limits at all. When disabled, no limit is enforced.
    pub enabled: bool,
    /// Backend that enforces the limits. Only this backend is used.
    pub backend: ResourceBackend,
    /// Memory limit in bytes. The cgroup backend applies `MemoryMax`; the
    /// rlimit backend enforces `RLIMIT_AS`, which caps address space rather
    /// than resident memory.
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
            enabled: true,
            backend: ResourceBackend::Cgroup,
            memory_bytes: 2 * 1024 * 1024 * 1024, // 2 GiB
            cpu_percent: 100,                     // one core
            max_pids: 512,
            timeout_secs: Some(60 * 60),    // one hour
            tmpfs_bytes: 512 * 1024 * 1024, // 512 MiB
        }
    }
}

#[derive(Debug)]
pub struct SandboxSpec {
    pub mounts: Vec<Mount>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
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
        assert!(l.enabled);
        assert_eq!(l.backend, ResourceBackend::Cgroup);
        assert_eq!(l.memory_bytes, 2 * 1024 * 1024 * 1024);
        assert_eq!(l.max_pids, 512);
        assert_eq!(l.tmpfs_bytes, 512 * 1024 * 1024);
        assert_eq!(l.timeout_secs, Some(60 * 60));
    }

    #[test]
    fn backend_names_round_trip() {
        assert_eq!(
            ResourceBackend::parse("cgroup"),
            Some(ResourceBackend::Cgroup)
        );
        assert_eq!(
            ResourceBackend::parse("rlimit"),
            Some(ResourceBackend::Rlimit)
        );
        assert_eq!(ResourceBackend::parse("Cgroup"), None);
        assert_eq!(ResourceBackend::parse(""), None);
        assert_eq!(ResourceBackend::Cgroup.name(), "cgroup");
        assert_eq!(ResourceBackend::Rlimit.name(), "rlimit");
    }

    #[test]
    fn network_mode_flags() {
        assert!(!NetworkMode::None.has_network());
        assert!(NetworkMode::Private.has_network());
        assert!(NetworkMode::Host.has_network());
    }
}
