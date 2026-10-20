//! Sandbox backend abstraction.
//!
//! The current implementation uses bubblewrap (bwrap) on Linux only.
//! This module exists as an extension point: future backends (e.g. Linux
//! namespaces via different mechanisms, or platform-specific sandboxes)
//! should implement the same contract without changing the Nix evaluation/
//! realization boundary.
//!
//! ## Trust boundary (by design)
//! `nix eval`, `nix build`, realization (`nix-store --realise`), and closure
//! calculation (`nix path-info --recursive`) run **outside** the agent sandbox
//! and rely on trusted configuration (the flake/attr path provided by the
//! caller or environment). Only the **untrusted agent process tree** launched
//! via `bwrap` is contained.
//!
//! Evaluating arbitrary untrusted flakes is **not** a current security goal.
//! Doing so would require sandboxing Nix evaluation/build in a separate,
//! hardened environment.
//!
//! ## Platform support
//! Runtime execution is **Linux-only** because it depends on bwrap and
//! Linux namespaces/cgroups. Nix flake outputs may be defined for other
//! platforms for evaluation/build purposes, but `sandbox::run` will reject
//! non-Linux hosts with a clear error.

use crate::error::Result;
use crate::sandbox::spec::SandboxSpec;

/// Trait for sandbox backends.
///
/// Implementors are responsible for constructing the OS sandbox and
/// exec'ing the process tree described by `SandboxSpec`.
pub trait SandboxBackend {
    fn exec(&self, spec: &SandboxSpec) -> Result<()>;
}

/// The default backend is bubblewrap on Linux.
#[derive(Debug, Default)]
pub struct BwrapBackend;

impl SandboxBackend for BwrapBackend {
    fn exec(&self, spec: &SandboxSpec) -> Result<()> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = spec;
            return Err(crate::error::Error::Spec(
                "bwrap backend is only available on Linux".into(),
            ));
        }
        #[cfg(target_os = "linux")]
        {
            crate::sandbox::cgroup::exec_with_limits(spec)
        }
    }
}
