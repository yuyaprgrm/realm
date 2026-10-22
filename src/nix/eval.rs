use crate::error::{Error, Result};
use crate::sandbox::spec::{BindMount, ResourceBackend};
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

/// Resource limits declared in the configuration.
///
/// Every value is optional: an omitted limit uses the built-in default, and
/// the default never depends on the host environment.
#[derive(Debug, PartialEq, Eq)]
pub struct ResourceCfg {
    /// Apply the resource limits at all.
    pub enable: bool,
    /// Backend that enforces the limits.
    pub backend: ResourceBackend,
    /// Memory limit in bytes.
    pub memory_bytes: Option<u64>,
    /// CPU quota in percent of one core.
    pub cpu_percent: Option<u32>,
    /// Maximum number of tasks.
    pub max_pids: Option<u32>,
    /// Wall-clock timeout in seconds.
    pub timeout_secs: Option<u64>,
}

#[derive(Debug)]
pub struct SandboxCfg {
    pub packages: Vec<Package>,
    /// Output store paths propagated by each listed package, in package order.
    pub propagated: Vec<Vec<String>>,
    pub env: HashMap<String, String>,
    pub network: Option<String>,
    pub mounts: Vec<BindMount>,
    pub workspace: Option<WorkspaceCfg>,
    pub resources: ResourceCfg,
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
  resources =
    let r = if (cfg ? resources) && cfg.resources != null then cfg.resources else { };
    in if builtins.isAttrs r then {
      enable = r.enable or true;
      backend = r.backend or "cgroup";
      memory = r.memory or null;
      cpu = r.cpu or null;
      tasks = r.tasks or null;
      timeout = r.timeout or null;
    } else throw "resources must be an attribute set";
in {
  packages = map (p: {
    out = builtins.toString p;
    drv = if builtins.isAttrs p && (p.type or "") == "derivation" then p.drvPath else null;
  }) cfg.packages;
  propagated = map (p: map (x: x.outPath) (p.propagatedBuildInputs or [])) cfg.packages;
  env = cfg.env or {};
  network = cfg.network or null;
  mounts = map (m: {
    host = builtins.toString m.host;
    guest = builtins.toString m.guest;
    mode = builtins.toString m.mode;
  }) (cfg.mounts or []);
  inherit workspace resources;
  # `requireCgroup` was replaced by `resources.backend`; it is only read to
  # print a deprecation warning.
  requireCgroup =
    if cfg ? requireCgroup then cfg.requireCgroup
    else if cfg ? require_cgroup then cfg.require_cgroup
    else null;
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
    let mut propagated = Vec::new();
    if let Some(p) = obj.get("propagated") {
        let groups = p
            .as_array()
            .ok_or_else(|| Error::Nix("propagated must be array".into()))?;
        for (i, group) in groups.iter().enumerate() {
            let entries = group
                .as_array()
                .ok_or_else(|| Error::Nix(format!("propagated[{i}] must be array")))?;
            let mut outs = Vec::new();
            for (j, out) in entries.iter().enumerate() {
                let out = out
                    .as_str()
                    .ok_or_else(|| Error::Nix(format!("propagated[{i}][{j}] must be string")))?;
                outs.push(out.to_string());
            }
            propagated.push(outs);
        }
    } else {
        propagated.resize(pkgs.len(), Vec::new());
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
    let resources = parse_resources(obj.get("resources"))?;
    if let Some(value) = obj.get("requireCgroup")
        && !value.is_null()
    {
        eprintln!("{}", require_cgroup_deprecation());
    }

    Ok(SandboxCfg {
        packages: pkgs,
        propagated,
        env,
        network,
        mounts,
        workspace,
        resources,
    })
}

/// Notice printed when the removed `requireCgroup` option is still used.
fn require_cgroup_deprecation() -> &'static str {
    "warning: `requireCgroup` is deprecated and ignored; use `resources.backend = \"cgroup\"` \
     to require cgroup limits, or `resources.backend = \"rlimit\"` to use the rlimit backend"
}

/// Parse the `resources` block. Absent or empty blocks keep the documented
/// defaults: limits enabled, enforced by the cgroup backend.
fn parse_resources(value: Option<&Value>) -> Result<ResourceCfg> {
    let default = ResourceCfg {
        enable: true,
        backend: ResourceBackend::Cgroup,
        memory_bytes: None,
        cpu_percent: None,
        max_pids: None,
        timeout_secs: None,
    };
    let Some(value) = value else {
        return Ok(default);
    };
    if value.is_null() {
        return Ok(default);
    }
    let object = value
        .as_object()
        .ok_or_else(|| Error::Nix("resources must be an object".into()))?;
    Ok(ResourceCfg {
        enable: parse_enable(object.get("enable"))?,
        backend: parse_backend(object.get("backend"))?,
        memory_bytes: parse_memory(object.get("memory"))?,
        cpu_percent: parse_cpu(object.get("cpu"))?,
        max_pids: parse_tasks(object.get("tasks"))?,
        timeout_secs: parse_timeout(object.get("timeout"))?,
    })
}

fn parse_enable(value: Option<&Value>) -> Result<bool> {
    match value {
        None | Some(Value::Null) => Ok(true),
        Some(value) => value
            .as_bool()
            .ok_or_else(|| Error::Nix("resources.enable must be a boolean".into())),
    }
}

fn parse_backend(value: Option<&Value>) -> Result<ResourceBackend> {
    let Some(value) = value else {
        return Ok(ResourceBackend::Cgroup);
    };
    if value.is_null() {
        return Ok(ResourceBackend::Cgroup);
    }
    let name = value
        .as_str()
        .ok_or_else(|| Error::Nix("resources.backend must be a string".into()))?;
    ResourceBackend::parse(name).ok_or_else(|| {
        Error::Nix(format!(
            "resources.backend must be \"{}\" or \"{}\", got \"{name}\"",
            ResourceBackend::Cgroup.name(),
            ResourceBackend::Rlimit.name()
        ))
    })
}

fn parse_memory(value: Option<&Value>) -> Result<Option<u64>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let bytes = match value {
        Value::Number(number) => number.as_u64().ok_or_else(|| {
            Error::Nix("resources.memory must be a positive number of bytes".into())
        })?,
        Value::String(text) => parse_size(text).ok_or_else(|| {
            Error::Nix(format!(
                "resources.memory must be a byte count or a size like \"4G\", got \"{text}\""
            ))
        })?,
        _ => {
            return Err(Error::Nix(
                "resources.memory must be a byte count or a size string".into(),
            ));
        }
    };
    if bytes == 0 {
        return Err(Error::Nix(
            "resources.memory must be greater than zero".into(),
        ));
    }
    Ok(Some(bytes))
}

fn parse_cpu(value: Option<&Value>) -> Result<Option<u32>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let percent = match value {
        Value::Number(number) => number.as_u64().ok_or_else(|| {
            Error::Nix("resources.cpu must be a percentage between 1 and 100000".into())
        })?,
        Value::String(text) => parse_percent(text).ok_or_else(|| {
            Error::Nix(format!(
                "resources.cpu must be a percentage like \"200%\", got \"{text}\""
            ))
        })?,
        _ => {
            return Err(Error::Nix(
                "resources.cpu must be a percentage like \"200%\"".into(),
            ));
        }
    };
    if !(1..=100_000).contains(&percent) {
        return Err(Error::Nix(
            "resources.cpu must be a percentage between 1 and 100000".into(),
        ));
    }
    Ok(Some(percent as u32))
}

fn parse_tasks(value: Option<&Value>) -> Result<Option<u32>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let tasks = value
        .as_u64()
        .ok_or_else(|| Error::Nix("resources.tasks must be a positive integer".into()))?;
    if tasks == 0 || tasks > u32::MAX as u64 {
        return Err(Error::Nix(
            "resources.tasks must be a positive integer".into(),
        ));
    }
    Ok(Some(tasks as u32))
}

fn parse_timeout(value: Option<&Value>) -> Result<Option<u64>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let seconds = match value {
        Value::Number(number) => number.as_u64().ok_or_else(|| {
            Error::Nix("resources.timeout must be a positive number of seconds".into())
        })?,
        Value::String(text) => parse_duration(text).ok_or_else(|| {
            Error::Nix(format!(
                "resources.timeout must be a duration like \"1h\", got \"{text}\""
            ))
        })?,
        _ => {
            return Err(Error::Nix(
                "resources.timeout must be a duration like \"1h\"".into(),
            ));
        }
    };
    if seconds == 0 {
        return Err(Error::Nix(
            "resources.timeout must be greater than zero".into(),
        ));
    }
    Ok(Some(seconds))
}

/// Parse a byte size. Suffixes are binary multiples: `1K` is 1024 bytes.
fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let (digits, suffix) = split_number(text);
    let value: u64 = digits.parse().ok()?;
    let multiplier = match suffix.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" | "ki" | "kib" => 1024,
        "m" | "mb" | "mi" | "mib" => 1024 * 1024,
        "g" | "gb" | "gi" | "gib" => 1024 * 1024 * 1024,
        "t" | "tb" | "ti" | "tib" => 1024u64 * 1024 * 1024 * 1024,
        _ => return None,
    };
    value.checked_mul(multiplier)
}

/// Parse a CPU quota percentage. The `%` suffix is optional.
fn parse_percent(text: &str) -> Option<u64> {
    let text = text.trim();
    let digits = text.strip_suffix('%').unwrap_or(text).trim();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// Parse a duration. Plain numbers are seconds; `s`, `m`, `h` and `d` select
/// seconds, minutes, hours and days.
fn parse_duration(text: &str) -> Option<u64> {
    let text = text.trim();
    let (digits, suffix) = split_number(text);
    let value: u64 = digits.parse().ok()?;
    let multiplier = match suffix.trim().to_ascii_lowercase().as_str() {
        "" | "s" | "sec" | "secs" | "second" | "seconds" => 1,
        "m" | "min" | "mins" | "minute" | "minutes" => 60,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
        "d" | "day" | "days" => 86400,
        _ => return None,
    };
    value.checked_mul(multiplier)
}

/// Split a leading number from its unit suffix.
fn split_number(text: &str) -> (&str, &str) {
    let end = text
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(text.len());
    text.split_at(end)
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
        assert_eq!(
            cfg.propagated,
            vec![Vec::<String>::new(), Vec::<String>::new()]
        );
    }

    #[test]
    fn parses_propagated_outputs_per_package() {
        let json = br#"{"packages":[
            {"out":"/nix/store/aaa-x"},
            {"out":"/nix/store/bbb-y"}
        ],"propagated":[
            ["/nix/store/ccc-z","/nix/store/ddd-w"],
            []
        ],"env":{}}"#;
        let cfg = parse_sandbox_json(json).unwrap();
        assert_eq!(
            cfg.propagated,
            vec![
                vec![
                    "/nix/store/ccc-z".to_string(),
                    "/nix/store/ddd-w".to_string()
                ],
                vec![],
            ]
        );
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
        // The removed `requireCgroup` option is ignored; the cgroup backend is
        // the default and requires cgroup limits.
        assert_eq!(cfg.resources.backend, ResourceBackend::Cgroup);
        assert!(cfg.resources.enable);

        let json = br#"{"packages":[],"env":{},"workspace":{"mode":"rw"},"requireCgroup":false}"#;
        let cfg = parse_sandbox_json(json).unwrap();
        let workspace = cfg.workspace.as_ref().unwrap();
        assert!(workspace.host.is_none());
        assert!(!workspace.read_only);
        assert_eq!(cfg.resources.backend, ResourceBackend::Cgroup);

        let json = br#"{"packages":[],"env":{},"workspace":{"mode":"rx"}}"#;
        assert!(
            parse_sandbox_json(json)
                .unwrap_err()
                .to_string()
                .contains("workspace.mode")
        );
    }

    #[test]
    fn resources_default_to_enabled_cgroup_limits() {
        let cfg = parse_sandbox_json(br#"{"packages":[],"env":{}}"#).unwrap();
        assert!(cfg.resources.enable);
        assert_eq!(cfg.resources.backend, ResourceBackend::Cgroup);
        assert_eq!(cfg.resources.memory_bytes, None);
        assert_eq!(cfg.resources.cpu_percent, None);
        assert_eq!(cfg.resources.max_pids, None);
        assert_eq!(cfg.resources.timeout_secs, None);
    }

    #[test]
    fn parses_the_resources_block() {
        let json = br#"{"packages":[],"env":{},"resources":{
            "enable":true,"backend":"rlimit","memory":"4G","cpu":"200%","tasks":512,"timeout":"1h"
        }}"#;
        let cfg = parse_sandbox_json(json).unwrap();
        assert!(cfg.resources.enable);
        assert_eq!(cfg.resources.backend, ResourceBackend::Rlimit);
        assert_eq!(cfg.resources.memory_bytes, Some(4 * 1024 * 1024 * 1024));
        assert_eq!(cfg.resources.cpu_percent, Some(200));
        assert_eq!(cfg.resources.max_pids, Some(512));
        assert_eq!(cfg.resources.timeout_secs, Some(3600));
    }

    #[test]
    fn resources_can_be_disabled() {
        let json = br#"{"packages":[],"env":{},"resources":{"enable":false}}"#;
        let cfg = parse_sandbox_json(json).unwrap();
        assert!(!cfg.resources.enable);
        assert_eq!(cfg.resources.backend, ResourceBackend::Cgroup);
    }

    #[test]
    fn rejects_invalid_resource_settings() {
        for (json, expected) in [
            (
                br#"{"packages":[],"env":{},"resources":{"backend":"systemd"}}"#.as_slice(),
                "resources.backend",
            ),
            (
                br#"{"packages":[],"env":{},"resources":{"enable":"yes"}}"#.as_slice(),
                "resources.enable",
            ),
            (
                br#"{"packages":[],"env":{},"resources":{"memory":"4 gigs"}}"#.as_slice(),
                "resources.memory",
            ),
            (
                br#"{"packages":[],"env":{},"resources":{"memory":0}}"#.as_slice(),
                "resources.memory",
            ),
            (
                br#"{"packages":[],"env":{},"resources":{"cpu":"two hundred"}}"#.as_slice(),
                "resources.cpu",
            ),
            (
                br#"{"packages":[],"env":{},"resources":{"tasks":0}}"#.as_slice(),
                "resources.tasks",
            ),
            (
                br#"{"packages":[],"env":{},"resources":{"timeout":"90x"}}"#.as_slice(),
                "resources.timeout",
            ),
            (
                br#"{"packages":[],"env":{},"resources":{"timeout":0}}"#.as_slice(),
                "resources.timeout",
            ),
            (
                br#"{"packages":[],"env":{},"resources":[]}"#.as_slice(),
                "resources must be an object",
            ),
        ] {
            let error = parse_sandbox_json(json).unwrap_err().to_string();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn parses_plain_numbers_and_unit_variants() {
        let json = br#"{"packages":[],"env":{},"resources":{
            "memory":1024,"cpu":150,"tasks":8,"timeout":90
        }}"#;
        let cfg = parse_sandbox_json(json).unwrap();
        assert_eq!(cfg.resources.memory_bytes, Some(1024));
        assert_eq!(cfg.resources.cpu_percent, Some(150));
        assert_eq!(cfg.resources.max_pids, Some(8));
        assert_eq!(cfg.resources.timeout_secs, Some(90));

        let json = br#"{"packages":[],"env":{},"resources":{
            "memory":"512Mi","cpu":"50%","timeout":"30m"
        }}"#;
        let cfg = parse_sandbox_json(json).unwrap();
        assert_eq!(cfg.resources.memory_bytes, Some(512 * 1024 * 1024));
        assert_eq!(cfg.resources.cpu_percent, Some(50));
        assert_eq!(cfg.resources.timeout_secs, Some(1800));
    }
}
