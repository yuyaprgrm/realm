use crate::error::{Error, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Command;

#[derive(Debug)]
pub struct SandboxCfg {
    pub packages: Vec<String>,
    pub env: HashMap<String, String>,
}

pub fn eval_sandbox(flake_dir: &std::path::Path, system: &str, attr: &str) -> Result<SandboxCfg> {
    // nix eval --json <flake>#sandboxes.<system>.<name>
    let flake_ref = format!("{}#sandboxes.{}.{}", flake_dir.display(), system, attr);
    let out = Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command flakes",
            "eval",
            "--json",
            &flake_ref,
        ])
        .output()
        .map_err(|e| Error::Nix(e.to_string()))?;
    if !out.status.success() {
        return Err(Error::Nix(String::from_utf8_lossy(&out.stderr).into()));
    }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| Error::Nix(e.to_string()))?;
    // validate schema
    let obj = v.as_object().ok_or_else(|| Error::Nix("sandbox must be object".into()))?;
    let packages = obj
        .get("packages")
        .and_then(|x| x.as_array())
        .ok_or_else(|| Error::Nix("packages must be array".into()))?;
    let mut pkgs = Vec::new();
    for p in packages {
        let s = p
            .as_str()
            .ok_or_else(|| Error::Nix("package entries must be strings".into()))?;
        pkgs.push(s.to_string());
    }
    let mut env = HashMap::new();
    if let Some(e) = obj.get("env") {
        if let Some(em) = e.as_object() {
            for (k, v) in em {
                if let Some(s) = v.as_str() {
                    env.insert(k.clone(), s.to_string());
                } else {
                    return Err(Error::Nix("env values must be strings".into()));
                }
            }
        } else {
            return Err(Error::Nix("env must be object".into()));
        }
    }
    Ok(SandboxCfg { packages: pkgs, env })
}
