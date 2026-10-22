mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

const PRIVATE_ATTR: &str = "private-test";
const NONE_ATTR: &str = "none-test";
const HOST_ATTR: &str = "host-test";

/// Sandboxes with an explicit resource backend.
///
/// The `none` and `host` sandboxes use the rlimit backend so that they run
/// on hosts without a systemd user manager. The `private` sandbox uses the
/// cgroup backend: pasta and the sandbox share the systemd scope that
/// carries the limits.
struct Fixture {
    _root: tempfile::TempDir,
    config: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("create network fixture");
        let config = root.path().join("realm.nix");
        std::fs::write(
            &config,
            r#"{ nixpkgs ? builtins.getFlake "nixpkgs", system ? builtins.currentSystem }:
let pkgs = import nixpkgs.sourceInfo.outPath { inherit system; };
in
{
  none-test = {
    packages = with pkgs; [ bash coreutils curl procps cacert ];
    env = { };
    network = "none";
    resources = { enable = true; backend = "rlimit"; };
  };
  private-test = {
    packages = with pkgs; [ bash coreutils curl procps cacert ];
    env = { };
    network = "private";
    resources = { enable = true; backend = "cgroup"; };
  };
  host-test = {
    packages = with pkgs; [ bash coreutils curl procps cacert ];
    env = { };
    network = "host";
    resources = { enable = true; backend = "rlimit"; };
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
}

struct HostLocalServer {
    port: u16,
    stop: Sender<()>,
    join: Option<thread::JoinHandle<bool>>,
}

impl HostLocalServer {
    fn start() -> Self {
        let listener =
            TcpListener::bind(("127.0.0.1", 0)).expect("bind host localhost test server");
        listener.set_nonblocking(true).expect("set nonblocking");
        let port = listener.local_addr().expect("read server address").port();
        let (stop, stopped) = mpsc::channel();
        let join = thread::spawn(move || {
            loop {
                if stopped.try_recv().is_ok() {
                    return false;
                }
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut request = [0_u8; 1024];
                        let _ = stream.read(&mut request);
                        let _ = stream.write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                        );
                        return true;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => return false,
                }
            }
        });
        Self {
            port,
            stop,
            join: Some(join),
        }
    }

    fn finish(mut self) -> bool {
        let _ = self.stop.send(());
        self.join.take().unwrap().join().expect("join HTTP server")
    }
}

impl Drop for HostLocalServer {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn run_sandbox(fixture: &Fixture, attr: &str, script: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_realm"))
        .args([
            "--config",
            fixture.config.to_str().unwrap(),
            attr,
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

fn output_text(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn assert_process_isolated(output: &std::process::Output) {
    let text = output_text(output);
    assert!(
        output.status.success(),
        "sandbox failed: {text}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        text.contains("pidns:isolated"),
        "PID namespace was not isolated: {text}"
    );
    assert!(
        text.contains("mountns:isolated"),
        "mount namespace was not isolated: {text}"
    );
    assert!(
        text.contains("userns:isolated"),
        "user namespace was not isolated: {text}"
    );
    assert!(
        text.contains("dev:isolated"),
        "device namespace was not isolated: {text}"
    );
    assert!(
        text.contains("root:isolated"),
        "filesystem root was not isolated: {text}"
    );
}

fn namespace_inode(name: &str) -> String {
    std::fs::metadata(format!("/proc/self/ns/{name}"))
        .expect("read host namespace metadata")
        .ino()
        .to_string()
}

fn isolation_check() -> String {
    let host_root = std::fs::metadata("/").expect("read host root metadata");
    let host_dev = std::fs::metadata("/dev").expect("read host /dev metadata");
    format!(
        r#"
if test "$(stat -Lc %i /proc/self/ns/pid)" = "{}"; then echo pidns:shared; else echo pidns:isolated; fi
if test "$(stat -Lc %i /proc/self/ns/mnt)" = "{}"; then echo mountns:shared; else echo mountns:isolated; fi
if test "$(stat -Lc %i /proc/self/ns/user)" = "{}"; then echo userns:shared; else echo userns:isolated; fi
if test "$(stat -Lc %d:%i /)" = "{}:{}"; then echo root:shared; else echo root:isolated; fi
if test "$(stat -Lc %d:%i /dev)" = "{}:{}"; then echo dev:shared; else echo dev:isolated; fi
"#,
        namespace_inode("pid"),
        namespace_inode("mnt"),
        namespace_inode("user"),
        host_root.dev(),
        host_root.ino(),
        host_dev.dev(),
        host_dev.ino(),
    )
}

#[test]
fn none_has_no_interfaces_routes_dns_or_internet() {
    if !common::sandbox_available("network_modes::none") {
        return;
    }
    let server = HostLocalServer::start();
    let script = format!(
        r#"{isolation}
interfaces=0
while IFS= read -r line; do case "$line" in *:*) name=${{line%%:*}}; name=${{name//[[:space:]]/}}; if test "$name" != lo; then interfaces=$((interfaces + 1)); fi;; esac; done < /proc/net/dev
if test "$interfaces" -eq 0; then echo interfaces:none; else echo interfaces:present; fi
routes=0
while IFS= read -r line; do case "$line" in Iface*|'') ;; *) routes=$((routes + 1));; esac; done < /proc/net/route
if test "$routes" -gt 0; then echo routes:present; else echo routes:none; fi
if curl --silent --show-error --connect-timeout 3 https://example.com >/dev/null 2>&1; then echo internet:ok; else echo internet:no; fi
if curl --silent --show-error --connect-timeout 2 http://127.0.0.1:{port}/ >/dev/null 2>&1; then echo host-localhost:reachable; else echo host-localhost:blocked; fi
"#,
        isolation = isolation_check(),
        port = server.port,
    );
    let fixture = Fixture::new();
    let output = run_sandbox(&fixture, NONE_ATTR, &script);
    assert_process_isolated(&output);
    let text = output_text(&output);
    for expected in [
        "interfaces:none",
        "routes:none",
        "internet:no",
        "host-localhost:blocked",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert!(
        !server.finish(),
        "host localhost server received a None sandbox connection"
    );
}

#[test]
fn private_has_network_dns_internet_and_cannot_reach_host_localhost() {
    if !common::private_network_available("network_modes::private") {
        return;
    }
    let server = HostLocalServer::start();
    let require_external_https = std::env::var_os("REALM_TEST_REQUIRE_EXTERNAL_HTTPS").is_some();

    let script = format!(
        r#"{isolation}
if test "$(stat -Lc %i /proc/self/ns/net)" = "{host_netns}"; then echo netns:host; else echo netns:private; fi
interfaces=0
while IFS= read -r line; do case "$line" in *:*) name=${{line%%:*}}; name=${{name//[[:space:]]/}}; if test "$name" != lo; then interfaces=$((interfaces + 1)); fi;; esac; done < /proc/net/dev
if test "$interfaces" -gt 0; then echo interfaces:present; else echo interfaces:none; fi
default_route=0
while read -r iface destination rest; do if test "$destination" = 00000000; then default_route=1; fi; done < /proc/net/route
if test "$default_route" -eq 1; then echo default-route:present; else echo default-route:none; fi
if curl --silent --show-error --connect-timeout 5 --max-time 8 https://example.com >/dev/null 2>&1; then echo external-https:ok; else echo external-https:unavailable; fi
if curl --silent --show-error --connect-timeout 2 http://127.0.0.1:{port}/ >/dev/null 2>&1; then echo host-localhost:reachable; else echo host-localhost:blocked; fi
"#,
        isolation = isolation_check(),
        host_netns = namespace_inode("net"),
        port = server.port,
    );
    let fixture = Fixture::new();
    let output = run_sandbox(&fixture, PRIVATE_ATTR, &script);
    assert_process_isolated(&output);
    let text = output_text(&output);
    for expected in [
        "netns:private",
        "interfaces:present",
        "default-route:present",
        "host-localhost:blocked",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert!(
        !server.finish(),
        "server on host localhost received a private sandbox connection"
    );
    if require_external_https {
        assert!(
            text.contains("external-https:ok"),
            "external HTTPS was required but unavailable: {text}"
        );
    }
}

#[test]
fn host_shares_network_namespace_and_host_loopback() {
    if !common::sandbox_available("network_modes::host") {
        return;
    }
    let server = HostLocalServer::start();
    let script = format!(
        r#"{isolation}
if test "$(stat -Lc %i /proc/self/ns/net)" = "{host_netns}"; then echo netns:host; else echo netns:private; fi
if curl --silent --show-error --connect-timeout 3 http://127.0.0.1:{port}/ >/dev/null 2>&1; then echo host-localhost:reachable; else echo host-localhost:blocked; fi
"#,
        isolation = isolation_check(),
        host_netns = namespace_inode("net"),
        port = server.port,
    );
    let fixture = Fixture::new();
    let output = run_sandbox(&fixture, HOST_ATTR, &script);
    assert_process_isolated(&output);
    let text = output_text(&output);
    for expected in ["netns:host", "host-localhost:reachable"] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert!(
        server.finish(),
        "host sandbox did not reach host localhost listener"
    );
}

#[test]
fn private_is_launched_with_resource_limits() {
    if !common::private_network_available("network_modes::limits") {
        return;
    }
    let script = "sleep 10";
    let fixture = Fixture::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_realm"))
        .args([
            "--config",
            fixture.config.to_str().unwrap(),
            PRIVATE_ATTR,
            "--",
            "bash",
            "--noprofile",
            "--norc",
            "-c",
            script,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start private sandbox");
    let unit = format!("realm-sandbox-{}.scope", child.id());
    let mut relative = String::new();
    for _ in 0..100 {
        let cgroup = Command::new("systemctl")
            .args([
                "--user",
                "show",
                &unit,
                "--property=ControlGroup",
                "--value",
            ])
            .output()
            .expect("query private scope cgroup");
        assert!(
            cgroup.status.success(),
            "systemctl failed: {}",
            String::from_utf8_lossy(&cgroup.stderr)
        );
        relative = String::from_utf8_lossy(&cgroup.stdout).trim().to_string();
        if !relative.is_empty() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(
        relative.ends_with(".scope"),
        "invalid systemd cgroup: {relative}"
    );
    let cgroup_dir = std::path::Path::new("/sys/fs/cgroup").join(relative.trim_start_matches('/'));
    assert!(
        cgroup_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with(".scope"),
        "private launcher is not inside a systemd scope: {relative}"
    );
    assert_eq!(
        std::fs::read_to_string(cgroup_dir.join("memory.max"))
            .unwrap()
            .trim(),
        "2147483648"
    );
    assert_eq!(
        std::fs::read_to_string(cgroup_dir.join("pids.max"))
            .unwrap()
            .trim(),
        "512"
    );
    let cpu_max = std::fs::read_to_string(cgroup_dir.join("cpu.max")).unwrap();
    let mut cpu_fields = cpu_max.split_whitespace();
    let quota: u64 = cpu_fields.next().unwrap().parse().unwrap();
    let period: u64 = cpu_fields.next().unwrap().parse().unwrap();
    assert!(quota <= period, "CPU quota exceeds 100%: {cpu_max}");

    let runtime = Command::new("systemctl")
        .args([
            "--user",
            "show",
            &unit,
            "--property=RuntimeMaxUSec",
            "--value",
        ])
        .output()
        .expect("query systemd scope runtime limit");
    assert!(
        runtime.status.success(),
        "systemctl failed: {}",
        String::from_utf8_lossy(&runtime.stderr)
    );
    let runtime = String::from_utf8_lossy(&runtime.stdout);
    assert!(
        runtime.trim() == "3600000000" || runtime.trim() == "1h",
        "unexpected RuntimeMaxSec: {runtime}"
    );
    let _ = child.kill();
    let _ = child.wait();
}
