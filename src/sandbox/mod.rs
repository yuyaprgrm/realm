pub mod bwrap;
pub mod cgroup;
pub mod etc;
pub mod spec;

use crate::error::Result;

pub fn run(flake_ref: &str, command: &[String]) -> Result<()> {
    let mut spec = crate::nix::build_spec(flake_ref)?;
    if !command.is_empty() {
        // first is executable if we want, else run via shell - but easier: run the command directly
        spec.executable = std::path::PathBuf::from(&command[0]);
        spec.args = command[1..].to_vec();
    }
    crate::sandbox::cgroup::exec_with_limits(&spec)
}
