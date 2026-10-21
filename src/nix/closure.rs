use crate::error::{Error, Result};
use std::collections::HashSet;
use std::process::Command;

/// Runtime closure of `roots`, taken from Nix's own closure metadata.
///
/// `nix path-info --recursive` returns exactly the referenced store paths
/// (identical to `nix-store --query --requisites`), so no manual symlink
/// traversal is needed: Nix already records every runtime reference.
pub fn get_closure(roots: &[String]) -> Result<HashSet<String>> {
    if roots.is_empty() {
        return Ok(HashSet::new());
    }
    // one batched call for all roots
    let out = Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command flakes",
            "path-info",
            "--recursive",
        ])
        .args(roots)
        .output()
        .map_err(|e| Error::Nix(e.to_string()))?;
    if !out.status.success() {
        return Err(Error::Nix(format!(
            "path-info failed for {}: {}",
            roots.join(", "),
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    let s = String::from_utf8(out.stdout).map_err(|e| Error::Nix(e.to_string()))?;
    Ok(parse_paths(&s))
}

/// Parse `nix path-info` output: one store path per line, then optional columns.
fn parse_paths(text: &str) -> HashSet<String> {
    let mut closure = HashSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let tok = line.split_whitespace().next().unwrap_or(line);
        if tok.starts_with("/nix/store/") {
            closure.insert(tok.to_string());
        }
    }
    closure
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_roots_needs_no_call() {
        assert!(get_closure(&[]).unwrap().is_empty());
    }

    #[test]
    fn parses_store_paths_from_path_info_lines() {
        // mirrors `nix path-info --recursive` output: path + tab columns
        let lines = "/nix/store/aaa-x\t{\"narSize\":123}\n/nix/store/bbb-y\t{\"narSize\":4}\n";
        let closure = parse_paths(lines);
        assert_eq!(closure.len(), 2);
        assert!(closure.contains("/nix/store/aaa-x"));
    }
}
