use crate::error::{Error, Result};
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::Path;
use std::process::Command;

fn add_from_path_info(p: &str, closure: &mut HashSet<String>) -> Result<()> {
    let out = Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command flakes",
            "path-info",
            "--recursive",
            p,
        ])
        .output()
        .map_err(|e| Error::Nix(e.to_string()))?;
    if !out.status.success() {
        return Err(Error::Nix(format!(
            "path-info failed for {}: {}",
            p,
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    let s = String::from_utf8(out.stdout).map_err(|e| Error::Nix(e.to_string()))?;
    for line in s.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let tok = line.split_whitespace().next().unwrap_or(line);
        if tok.starts_with("/nix/store/") {
            closure.insert(tok.to_string());
        }
    }
    Ok(())
}

fn follow_symlinks(closure: &mut HashSet<String>) -> Result<()> {
    let mut seen_scan = HashSet::new();
    let mut q: VecDeque<String> = closure.iter().cloned().collect();
    for c in closure.iter().cloned() {
        q.push_back(c);
    }
    while let Some(cur) = q.pop_front() {
        if seen_scan.contains(&cur) {
            continue;
        }
        seen_scan.insert(cur.clone());
        let p = Path::new(&cur);
        if !p.exists() {
            continue;
        }
        // walk directory entries
        if let Ok(rd) = fs::read_dir(p) {
            for ent in rd.flatten() {
                let path = ent.path();
                // check symlink
                if let Ok(md) = fs::symlink_metadata(&path) {
                    if md.file_type().is_symlink() {
                        if let Ok(target) = fs::read_link(&path) {
                            if target.is_absolute() {
                                let ts = target.to_string_lossy();
                                if ts.starts_with("/nix/store/") {
                                    // normalize to the store path root? target might point to file inside store path
                                    // store paths are directories/files under /nix/store/<hash>-name; take the store path component
                                    let mut parts = ts.split('/');
                                    // skip empty, nix, store
                                    let _ = parts.next();
                                    let _ = parts.next();
                                    let _ = parts.next();
                                    if let Some(sp) = parts.next() {
                                        let store_root = format!("/nix/store/{}", sp);
                                        if !closure.contains(&store_root) {
                                            closure.insert(store_root.clone());
                                            add_from_path_info(&store_root, closure)?;
                                            q.push_back(store_root);
                                            continue;
                                        } else if !seen_scan.contains(&store_root) {
                                            q.push_back(store_root);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                // if dir, recurse? but read_dir is shallow; easier to also push dirs seen
                if let Ok(md) = fs::metadata(&path) {
                    if md.is_dir() {
                        // don't spam, only if not scanned? handled by seen_scan on store roots mainly
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn get_closure(packages: &[String]) -> Result<HashSet<String>> {
    let mut closure: HashSet<String> = HashSet::new();
    for p in packages {
        add_from_path_info(p, &mut closure)?;
    }
    // follow absolute symlinks inside closure
    follow_symlinks(&mut closure)?;
    Ok(closure)
}
