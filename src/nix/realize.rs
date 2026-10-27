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
    let missing_packages: Vec<&Package> = packages
        .iter()
        .filter(|package| !std::path::Path::new(&package.out).exists())
        .collect();
    let total = targets.len().max(missing_packages.len()).max(1);
    for (index, package) in missing_packages.iter().enumerate() {
        crate::ui::progress(
            index + 1,
            total,
            &format!("installing dependency {}", store_path_name(&package.out)),
        );
    }
    let out = Command::new("nix-store")
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

fn store_path_name(path: &str) -> &str {
    let base = std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path);
    base.split_once('-').map(|(_, name)| name).unwrap_or(base)
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
    #[test]
    fn store_path_name_strips_store_hash() {
        assert_eq!(store_path_name("/nix/store/abc123-bash-5.2"), "bash-5.2");
        assert_eq!(store_path_name("/nix/store/abc"), "abc");
    }
}
