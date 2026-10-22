pub mod bwrap;
pub mod cgroup;
pub mod etc;
pub mod limits;
pub mod network;
pub mod rlimit;
pub mod spec;

use crate::error::{Error, Result};

pub fn run(config: &std::path::Path, sandbox_name: &str, command: &[String]) -> Result<i32> {
    if cfg!(not(target_os = "linux")) {
        return Err(Error::Spec(
            "Realm requires Linux (bubblewrap and Linux namespaces); its Nix package may be evaluated on other platforms".into(),
        ));
    }
    // The preparation can take a while; tell the user that Realm is working.
    // The spinner is dropped (and its line erased) on every exit path.
    let mut spinner = crate::ui::Spinner::start();
    let mut spec = crate::nix::build_spec(config, sandbox_name)?;
    if !command.is_empty() {
        // first is executable if we want, else run via shell - but easier: run the command directly
        spec.executable = std::path::PathBuf::from(&command[0]);
        spec.args = command[1..].to_vec();
    }
    let status = crate::sandbox::limits::exec(&spec, &mut spinner)?;
    Ok(status.code().unwrap_or(1))
}
