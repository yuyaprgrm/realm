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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkPolicy {
    Disabled,
    Enabled,
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
    pub uid: u32,
    pub gid: u32,
    /// Owns the generated guest `/etc` and compatibility symlink tree so the
    /// files stay on disk until `exec` has bound them.
    pub keep_alive: Vec<crate::sandbox::etc::GuestFs>,
}
