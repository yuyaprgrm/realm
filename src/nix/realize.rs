use crate::error::{Error, Result};
use std::process::Command;

pub fn realize_packages(packages: &[String]) -> Result<()> {
    for p in packages {
        // nix-store --realise <store-path> (idempotent)
        let out = Command::new("nix-store")
            .args(["--realise", p])
            .output()
            .map_err(|e| Error::Nix(e.to_string()))?;
        if !out.status.success() {
            return Err(Error::Nix(format!("realise failed for {}: {}", p, String::from_utf8_lossy(&out.stderr))));
        }
    }
    Ok(())
}
