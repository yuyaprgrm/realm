use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    root: tempfile::TempDir,
    config: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("create mount fixture");
        for dir in ["ro", "rw", "outside"] {
            std::fs::create_dir(root.path().join(dir)).expect("create fixture directory");
        }
        std::fs::write(root.path().join("ro/read.txt"), "ro-host\n").unwrap();
        std::fs::write(root.path().join("rw/read.txt"), "rw-host\n").unwrap();
        std::fs::write(root.path().join("outside/secret.txt"), "host-secret\n").unwrap();

        let mounts = [("ro", "/sandbox/ro", "ro"), ("rw", "/sandbox/rw", "rw")]
            .into_iter()
            .map(|(dir, guest, mode)| {
                format!(
                    "{{ host = {}; guest = {}; mode = {}; }}",
                    nix_string(&root.path().join(dir)),
                    nix_string(Path::new(guest)),
                    nix_string(Path::new(mode)),
                )
            })
            .collect::<Vec<_>>()
            .join("\n      ");

        let config = root.path().join("realm.nix");
        std::fs::write(
            &config,
            format!(
                r#"{{ nixpkgs ? builtins.getFlake "nixpkgs", system ? builtins.currentSystem }}:
let pkgs = import nixpkgs.sourceInfo.outPath {{ inherit system; }};
in
{{
  mounts-test = {{
    packages = with pkgs; [ bash coreutils ];
    env = {{ }};
    network = "none";
    mounts = [
      {mounts}
    ];
  }};
}}
"#
            ),
        )
        .unwrap();

        Self { root, config }
    }

    fn run(&self, script: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_realm"))
            .args([
                "--config",
                self.config.to_str().unwrap(),
                "mounts-test",
                "--",
                "bash",
                "--noprofile",
                "--norc",
                "-c",
                script,
            ])
            .output()
            .expect("start sandbox binary")
    }
}

fn nix_string(path: &Path) -> String {
    format!("\"{}\"", path.to_string_lossy().replace('"', "\\\""))
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
fn read_only_mount_is_readable_but_not_writable() {
    let fixture = Fixture::new();
    let output = fixture.run(
        r#"
set -eu
test -d /sandbox
test -d /sandbox/ro
test "$(cat /sandbox/ro/read.txt)" = "ro-host"
test ! -e /sandbox/outside
test ! -e /sandbox/outside/secret.txt
if printf changed > /sandbox/ro/read.txt 2>/dev/null; then echo ro:existing-writable; else echo ro:existing-readonly; fi
if printf new > /sandbox/ro/created.txt 2>/dev/null; then echo ro:new-writable; else echo ro:new-readonly; fi
echo ro:done
"#,
    );
    let stdout = assert_success(&output);
    assert!(stdout.contains("ro:existing-readonly"), "{stdout}");
    assert!(stdout.contains("ro:new-readonly"), "{stdout}");
    assert!(stdout.contains("ro:done"), "{stdout}");
    assert_eq!(
        std::fs::read_to_string(fixture.root.path().join("ro/read.txt")).unwrap(),
        "ro-host\n"
    );
    assert!(!fixture.root.path().join("ro/created.txt").exists());
}

#[test]
fn read_write_mount_writes_reach_the_host_and_undeclared_paths_stay_hidden() {
    let fixture = Fixture::new();
    let output = fixture.run(
        r#"
set -eu
test -d /sandbox
test -d /sandbox/rw
test "$(cat /sandbox/rw/read.txt)" = "rw-host"
test ! -e /sandbox/outside
test ! -e /sandbox/outside/secret.txt
printf 'changed\n' > /sandbox/rw/read.txt
printf 'created\n' > /sandbox/rw/created.txt
echo rw:done
"#,
    );
    let stdout = assert_success(&output);
    assert!(stdout.contains("rw:done"), "{stdout}");
    assert_eq!(
        std::fs::read_to_string(fixture.root.path().join("rw/read.txt")).unwrap(),
        "changed\n"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.root.path().join("rw/created.txt")).unwrap(),
        "created\n"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.root.path().join("outside/secret.txt")).unwrap(),
        "host-secret\n"
    );
}
