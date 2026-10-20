use crate::error::{Error, Result};
use crate::nix::eval::Package;
use std::path::Path;
use std::process::Command;

/// Store paths still needing realisation: the derivation when available
/// (builds the output), the bare output path otherwise (substitution only).
fn missing_targets(packages: &[Package]) -> Vec<String> {
    let mut targets = Vec::new();
    for p in packages {
        if Path::new(&p.out).exists() {
            continue;
        }
        let t = p.drv.as_deref().unwrap_or(&p.out);
        if !targets.iter().any(|x| x == t) {
            targets.push(t.to_string());
        }
    }
    targets
}

pub fn realize_packages(packages: &[Package]) -> Result<()> {
    let targets = missing_targets(packages);
    if targets.is_empty() {
        return Ok(());
    }
    // one batched nix-store call; already-realised outputs are skipped above
    let out = Command::new("nix-store")
        .arg("--realise")
        .args(&targets)
        .output()
        .map_err(|e| Error::Nix(e.to_string()))?;

    let missing: Vec<&str> = packages
        .iter()
        .filter(|p| !Path::new(&p.out).exists())
        .map(|p| p.out.as_str())
        .collect();
    if !out.status.success() || !missing.is_empty() {
        let failed = if missing.is_empty() {
            targets.join(", ")
        } else {
            missing.join(", ")
        };
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stderr = stderr.trim();
        let detail = if stderr.is_empty() {
            "nix-store --realise gave no error output"
        } else {
            stderr
        };
        return Err(Error::Nix(format!(
            "realise failed for {}: {}",
            failed, detail
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(out: &str, drv: Option<&str>) -> Package {
        Package {
            out: out.into(),
            drv: drv.map(Into::into),
        }
    }

    #[test]
    fn skips_existing_and_prefers_drv() {
        let existing = tempfile::tempdir().unwrap();
        let existing_path = existing.path().to_str().unwrap();
        let packages = vec![
            pkg(existing_path, Some("/nix/store/aaa-done.drv")),
            pkg("/nix/store/aaa-new", Some("/nix/store/aaa-new.drv")),
            pkg("/nix/store/bbb-new", None),
        ];
        assert_eq!(
            missing_targets(&packages),
            vec!["/nix/store/aaa-new.drv", "/nix/store/bbb-new"]
        );
    }

    #[test]
    fn dedupes_targets() {
        let packages = vec![pkg("/nix/store/dup", None), pkg("/nix/store/dup", None)];
        assert_eq!(missing_targets(&packages), vec!["/nix/store/dup"]);
    }
}
