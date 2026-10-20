//! Network backend selection and helpers.
//!
//! The network policy (Disabled/Isolated/Internet) is distinct from the
//! mechanism used to provide connectivity (None/Pasta). For `Internet`,
//! we use `pasta` to connect the sandbox's private network namespace to
//! the host's networking without sharing the host network namespace.

use crate::error::{Error, Result};
use crate::sandbox::spec::NetworkPolicy;
use std::path::PathBuf;

/// Network connectivity backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkBackend {
    /// No external connectivity.
    None,
    /// Connect private netns to host networking via pasta.
    Pasta,
}

/// Resolve which backend to use for the given policy.
pub fn backend_for(policy: NetworkPolicy) -> NetworkBackend {
    match policy {
        NetworkPolicy::Disabled | NetworkPolicy::Isolated => NetworkBackend::None,
        NetworkPolicy::Internet => NetworkBackend::Pasta,
    }
}

/// Find pasta binary in PATH or common Nix store locations.
pub fn find_pasta() -> Option<PathBuf> {
    if let Ok(p) = which::which("pasta") {
        return Some(p);
    }
    if let Ok(_p) = which::which("passt") {
        // passt provides pasta in some builds; but we specifically want pasta
    }
    // Common Nix store paths seen in this environment
    let candidates = &[
        "/nix/store/dcyh5h3mz00xcwxg9w2mgzd1fby2psx6-passt-2026_07_16.090d739/bin/pasta",
    ];
    for c in candidates {
        let p = PathBuf::from(c);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Ensure pasta is available if backend requires it.
pub fn require_pasta() -> Result<PathBuf> {
    find_pasta().ok_or_else(|| {
        Error::Bwrap("network policy \"internet\" requires pasta, but pasta was not found".into())
    })
}
