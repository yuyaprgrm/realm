use crate::error::{Error, Result};
use crate::sandbox::spec::BindMount;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, PartialEq, Eq)]
pub struct Package {
    /// output store path (may be unrealised)
    pub out: String,
    /// derivation that builds `out`, if the package is a derivation
    pub drv: Option<String>,
}

#[derive(Debug)]
pub struct WorkspaceCfg {
    /// Explicit host directory; `None` keeps the current directory default.
    pub host: Option<PathBuf>,
    pub read_only: bool,
}

#[derive(Debug)]
pub struct SandboxCfg {
    pub packages: Vec<Package>,
    pub env: HashMap<String, String>,
    pub network: Option<String>,
    pub mounts: Vec<BindMount>,
    pub workspace: Option<WorkspaceCfg>,
    pub require_cgroup: bool,
}

// Nix evaluates realm.nix and package derivations outside the sandbox.
const NORMALIZE: &str = r#"cfg:
let
  workspace =
    if (cfg ? workspace) && cfg.workspace != null then {
      host =
        if (cfg.workspace ? host) && cfg.workspace.host != null
        then builtins.toString cfg.workspace.host
        else null;
      mode = builtins.toString (cfg.workspace.mode or "rw");
    } else null;
  requireCgroup =
    if cfg ? requireCgroup then cfg.requireCgroup
    else if cfg ? require_cgroup then cfg.require_cgroup
    else false;
in {
  packages = map (p: {
    out = builtins.toString p;
    drv = if builtins.isAttrs p && (p.type or "") == "derivation" then p.drvPath else null;
  }) cfg.packages;
  env = cfg.env or {};
  network = cfg.network or null;
  mounts = map (m: {
    host = builtins.toString m.host;
    guest = builtins.toString m.guest;
    mode = builtins.toString m.mode;
  }) (cfg.mounts or []);
  inherit workspace requireCgroup;
}"#;

pub fn eval_sandbox(config: &std::path::Path, name: &str) -> Result<SandboxCfg> {
    if name.is_empty()
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_".contains(character))
    {
        return Err(Error::Nix(
            "sandbox name may contain only ASCII letters, digits, hyphens, and underscores".into(),
        ));
    }
    // Nix evaluates the standalone realm.nix function and computes package outputs.
    let transform =
        format!("config: let cfg = config {{ }}; in ({NORMALIZE}) (builtins.getAttr {name:?} cfg)");
    let out = Command::new("nix")
        .args([
            "--extra-experimental-features",
            "nix-command flakes",
            "eval",
            "--impure",
            "--json",
            "--file",
        ])
        .arg(config)
        .args(["--apply", &transform])
        .output()
        .map_err(|e| Error::Nix(e.to_string()))?;
    if !out.status.success() {
        return Err(Error::Nix(String::from_utf8_lossy(&out.stderr).into()));
    }
    parse_sandbox_json(&out.stdout)
}

fn parse_sandbox_json(bytes: &[u8]) -> Result<SandboxCfg> {
    let v: Value = serde_json::from_slice(bytes).map_err(|e| Error::Nix(e.to_string()))?;
    let obj = v
        .as_object()
        .ok_or_else(|| Error::Nix("sandbox must be object".into()))?;
    let packages = obj
        .get("packages")
        .and_then(|x| x.as_array())
        .ok_or_else(|| Error::Nix("packages must be array".into()))?;
    let mut pkgs = Vec::new();
    for (i, p) in packages.iter().enumerate() {
        let o = p
            .as_object()
            .ok_or_else(|| Error::Nix(format!("packages[{i}] must be object")))?;
        let out = o
            .get("out")
            .and_then(|x| x.as_str())
            .ok_or_else(|| Error::Nix(format!("packages[{i}].out must be string")))?
            .to_string();
        let drv = match o.get("drv") {
            None | Some(Value::Null) => None,
            Some(x) => Some(
                x.as_str()
                    .ok_or_else(|| Error::Nix(format!("packages[{i}].drv must be string or null")))?
                    .to_string(),
            ),
        };
        if out.is_empty() {
            return Err(Error::Nix(format!("packages[{i}].out must not be empty")));
        }
        pkgs.push(Package { out, drv });
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
    let network = match obj.get("network") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_str()
                .ok_or_else(|| Error::Nix("network must be a string".into()))?
                .to_string(),
        ),
    };
    let mut mounts = Vec::new();
    if let Some(m) = obj.get("mounts") {
        let entries = m
            .as_array()
            .ok_or_else(|| Error::Nix("mounts must be array".into()))?;
        for (i, entry) in entries.iter().enumerate() {
            let o = entry
                .as_object()
                .ok_or_else(|| Error::Nix(format!("mounts[{i}] must be object")))?;
            let host = o
                .get("host")
                .and_then(|x| x.as_str())
                .ok_or_else(|| Error::Nix(format!("mounts[{i}].host must be string")))?;
            let guest = o
                .get("guest")
                .and_then(|x| x.as_str())
                .ok_or_else(|| Error::Nix(format!("mounts[{i}].guest must be string")))?;
            let mode = o
                .get("mode")
                .and_then(|x| x.as_str())
                .ok_or_else(|| Error::Nix(format!("mounts[{i}].mode must be string")))?;
            let read_only = match mode {
                "ro" => true,
                "rw" => false,
                other => {
                    return Err(Error::Nix(format!(
                        "mounts[{i}].mode must be \"ro\" or \"rw\", got \"{other}\""
                    )));
                }
            };
            mounts.push(BindMount {
                host: PathBuf::from(host),
                guest: PathBuf::from(guest),
                read_only,
            });
        }
    }
    let workspace = match obj.get("workspace") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let w = value
                .as_object()
                .ok_or_else(|| Error::Nix("workspace must be an object".into()))?;
            let host = match w.get("host") {
                None | Some(Value::Null) => None,
                Some(host) => Some(PathBuf::from(host.as_str().ok_or_else(|| {
                    Error::Nix("workspace.host must be string or null".into())
                })?)),
            };
            let mode = w.get("mode").and_then(|x| x.as_str()).unwrap_or("rw");
            let read_only = match mode {
                "rw" => false,
                "ro" => true,
                other => {
                    return Err(Error::Nix(format!(
                        "workspace.mode must be \"ro\" or \"rw\", got \"{other}\""
                    )));
                }
            };
            Some(WorkspaceCfg { host, read_only })
        }
    };
    let require_cgroup = match obj.get("requireCgroup") {
        None | Some(Value::Null) => false,
        Some(v) => v
            .as_bool()
            .ok_or_else(|| Error::Nix("requireCgroup must be a boolean".into()))?,
    };

    Ok(SandboxCfg {
        packages: pkgs,
        env,
        network,
        mounts,
        workspace,
        require_cgroup,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_out_drv_objects() {
        let json = br#"{"packages":[
            {"out":"/nix/store/aaa-x","drv":"/nix/store/aaa-x.drv"},
            {"out":"/nix/store/bbb-y","drv":null}
        ],"env":{"FOO":"bar"}}"#;
        let cfg = parse_sandbox_json(json).unwrap();
        assert_eq!(
            cfg.packages,
            vec![
                Package {
                    out: "/nix/store/aaa-x".into(),
                    drv: Some("/nix/store/aaa-x.drv".into())
                },
                Package {
                    out: "/nix/store/bbb-y".into(),
                    drv: None
                },
            ]
        );
        assert_eq!(cfg.env.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(cfg.network, None);
    }

    #[test]
    fn parses_network_policy() {
        let json = br#"{"packages":[{"out":"/nix/store/x"}],"env":{},"network":"none"}"#;
        assert_eq!(
            parse_sandbox_json(json).unwrap().network.as_deref(),
            Some("none")
        );
        let json = br#"{"packages":[{"out":"/nix/store/x"}],"env":{},"network":"private"}"#;
        assert_eq!(
            parse_sandbox_json(json).unwrap().network.as_deref(),
            Some("private")
        );
        let json = br#"{"packages":[{"out":"/nix/store/x"}],"env":{},"network":"host"}"#;
        assert_eq!(
            parse_sandbox_json(json).unwrap().network.as_deref(),
            Some("host")
        );
        let json = br#"{"packages":[{"out":"/nix/store/x"}],"env":{}}"#;
        assert_eq!(parse_sandbox_json(json).unwrap().network, None);
    }

    #[test]
    fn parses_mounts() {
        let json = br#"{"packages":[],"env":{},"mounts":[
            {"host":"/host/ro","guest":"/sandbox/ro","mode":"ro"},
            {"host":"/host/rw","guest":"/sandbox/rw","mode":"rw"}
        ]}"#;
        let cfg = parse_sandbox_json(json).unwrap();
        assert_eq!(cfg.mounts.len(), 2);
        assert_eq!(cfg.mounts[0].host, PathBuf::from("/host/ro"));
        assert_eq!(cfg.mounts[0].guest, PathBuf::from("/sandbox/ro"));
        assert!(cfg.mounts[0].read_only);
        assert!(!cfg.mounts[1].read_only);

        let json = br#"{"packages":[],"env":{},"mounts":[
            {"host":"/host","guest":"/sandbox","mode":"rx"}
        ]}"#;
        assert!(
            parse_sandbox_json(json)
                .unwrap_err()
                .to_string()
                .contains("mode")
        );
    }

    #[test]
    fn parses_workspace_and_cgroup_policy() {
        let json = br#"{"packages":[],"env":{},"workspace":{"host":"/host/ws","mode":"ro"},"requireCgroup":true}"#;
        let cfg = parse_sandbox_json(json).unwrap();
        let workspace = cfg.workspace.as_ref().unwrap();
        assert_eq!(
            workspace.host.as_deref(),
            Some(PathBuf::from("/host/ws").as_path())
        );
        assert!(workspace.read_only);
        assert!(cfg.require_cgroup);

        let json = br#"{"packages":[],"env":{},"workspace":{"mode":"rw"},"requireCgroup":false}"#;
        let cfg = parse_sandbox_json(json).unwrap();
        let workspace = cfg.workspace.as_ref().unwrap();
        assert!(workspace.host.is_none());
        assert!(!workspace.read_only);
        assert!(!cfg.require_cgroup);

        let json = br#"{"packages":[],"env":{},"workspace":{"mode":"rx"}}"#;
        assert!(
            parse_sandbox_json(json)
                .unwrap_err()
                .to_string()
                .contains("workspace.mode")
        );
    }
}
