pub mod backend;
pub mod bwrap;
pub mod cgroup;
pub mod etc;
pub mod network;
pub mod spec;

use crate::error::{Error, Result};
use crate::sandbox::backend::SandboxBackend;

pub fn run(flake_ref: &str, command: &[String]) -> Result<()> {
    if cfg!(not(target_os = "linux")) {
        return Err(Error::Spec(
            "sandbox runtime requires Linux (bwrap backend); Nix flake outputs may support other platforms but the runtime is Linux-only".into(),
        ));
    }
    let mut spec = crate::nix::build_spec(flake_ref)?;
    if !command.is_empty() {
        // first is executable if we want, else run via shell - but easier: run the command directly
        spec.executable = std::path::PathBuf::from(&command[0]);
        spec.args = command[1..].to_vec();
    }
    backend::BwrapBackend.exec(&spec)
}
