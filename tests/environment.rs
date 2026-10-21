use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture {
    _root: tempfile::TempDir,
    config: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("create environment fixture");
        let config = root.path().join("realm.nix");
        std::fs::write(
            &config,
            r#"{ nixpkgs ? builtins.getFlake "nixpkgs", system ? builtins.currentSystem }:
let pkgs = import nixpkgs.sourceInfo.outPath { inherit system; };
in
{
  env-test = {
    packages = with pkgs; [ bash coreutils gnugrep ];
    env = { REALM_EXPLICIT = "visible"; };
    network = "none";
  };
}
"#,
        )
        .unwrap();
        Self {
            _root: root,
            config,
        }
    }

    fn run(&self, script: &str, host_marker: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_realm"))
            .args([
                "--config",
                self.config.to_str().unwrap(),
                "env-test",
                "--",
                "bash",
                "--noprofile",
                "--norc",
                "-c",
                script,
            ])
            .env("REALM_HOST_LEAK", host_marker)
            .env("SSH_AUTH_SOCK", "/host/ssh-agent.sock")
            .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/host/bus")
            .env("XDG_RUNTIME_DIR", "/run/user/1000")
            .output()
            .expect("start sandbox binary")
    }
}

fn assert_success(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "sandbox failed: {stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

#[test]
fn host_environment_is_not_visible_in_the_sandbox_or_proc_one() {
    let fixture = Fixture::new();
    let host_marker = format!("host-secret-{}", std::process::id());
    let output = fixture.run(
        r#"
set -eu
leaks=0
for environ in /proc/[0-9]*/environ; do
  if tr '\0' '\n' < "$environ" 2>/dev/null | grep -q '^REALM_HOST_LEAK='; then
    echo "leak:$environ"
    leaks=$((leaks + 1))
  fi
done
for name in SSH_AUTH_SOCK DBUS_SESSION_BUS_ADDRESS XDG_RUNTIME_DIR; do
  if tr '\0' '\n' < /proc/1/environ | grep -q "^$name="; then
    echo "leak:$name"
    leaks=$((leaks + 1))
  fi
done
test "$HOME" = /home/sandbox
test "$TMPDIR" = /tmp
test "$PWD" = /workspace
test "$REALM_EXPLICIT" = visible
test "$leaks" -eq 0
echo env:clean
"#,
        &host_marker,
    );
    let stdout = assert_success(&output);
    assert!(stdout.contains("env:clean"), "{stdout}");
    assert!(!stdout.contains("leak:"), "{stdout}");
}
