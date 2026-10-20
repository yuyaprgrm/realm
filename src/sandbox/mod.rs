pub mod backend;
pub mod bwrap;
pub mod cgroup;
pub mod etc;
pub mod network;
pub mod spec;

use crate::error::{Error, Result};
use crate::sandbox::backend::SandboxBackend;

pub fn run(config: &std::path::Path, sandbox_name: &str, command: &[String]) -> Result<()> {
    if cfg!(not(target_os = "linux")) {
        return Err(Error::Spec(
            "Realm requires Linux (bubblewrap and Linux namespaces); its Nix package may be evaluated on other platforms".into(),
        ));
    }
    let mut spec = crate::nix::build_spec(config, sandbox_name)?;
    if !command.is_empty() {
        // first is executable if we want, else run via shell - but easier: run the command directly
        spec.executable = std::path::PathBuf::from(&command[0]);
        spec.args = command[1..].to_vec();
    }
    backend::BwrapBackend.exec(&spec)
}
