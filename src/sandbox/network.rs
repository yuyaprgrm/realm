//! Network helpers.
//!
//! NetworkMode (None/Private/Host) defines semantics. For `Private`,
//! we use `pasta` to provide user-mode networking without sharing the
//! host network namespace.

use crate::error::{Error, Result};
use std::ffi::OsStr;
use std::path::PathBuf;

/// Private IPv4 network configured by pasta for `NetworkMode::Private`.
pub const PASTA_ADDRESS: &str = "10.0.2.100/24";
pub const PASTA_GATEWAY: &str = "10.0.2.2";
/// Address exposed inside the pasta namespace as its DNS forwarding endpoint.
pub const PASTA_DNS_FORWARD: &str = "10.0.2.3";

fn find_pasta_in(path: Option<&OsStr>) -> Option<PathBuf> {
    which::which_in("pasta", path, ".").ok()
}

/// Ensure pasta is available for private networking.
pub fn require_pasta() -> Result<PathBuf> {
    require_pasta_in(std::env::var_os("PATH").as_deref())
}

fn require_pasta_in(path: Option<&OsStr>) -> Result<PathBuf> {
    find_pasta_in(path).ok_or_else(|| {
        Error::Bwrap(
            "network mode \"private\" requires the `pasta` executable on PATH; install passt/pasta and retry".into(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn pasta_is_discovered_only_from_the_provided_path() {
        let path = tempfile::tempdir().unwrap();
        let binary = path.path().join("pasta");
        std::fs::write(&binary, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(
            find_pasta_in(Some(path.path().as_os_str())).as_deref(),
            Some(binary.as_path())
        );
        let empty = tempfile::tempdir().unwrap();
        assert!(find_pasta_in(Some(empty.path().as_os_str())).is_none());
    }

    #[test]
    fn missing_pasta_has_a_clear_error() {
        let empty = tempfile::tempdir().unwrap();
        let error = require_pasta_in(Some(empty.path().as_os_str()))
            .unwrap_err()
            .to_string();
        assert!(error.contains("requires the `pasta` executable on PATH"));
    }
}
