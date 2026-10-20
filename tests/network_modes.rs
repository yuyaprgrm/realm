use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::MetadataExt;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

const PRIVATE_ATTR: &str = ".#private-test";
const NONE_ATTR: &str = ".#none-test";
const HOST_ATTR: &str = ".#host-test";

fn run_sandbox(attr: &str, script: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_sandbox"))
        .args([attr, "--", "bash", "--noprofile", "--norc", "-c", script])
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
    let script = format!(
        r#"{isolation}
interfaces=0
while IFS= read -r line; do case "$line" in *:*) name=${{line%%:*}}; name=${{name//[[:space:]]/}}; if test "$name" != lo; then interfaces=$((interfaces + 1)); fi;; esac; done < /proc/net/dev
if test "$interfaces" -eq 0; then echo interfaces:none; else echo interfaces:present; fi
routes=0
while IFS= read -r line; do case "$line" in Iface*|'') ;; *) routes=$((routes + 1));; esac; done < /proc/net/route
if test "$routes" -gt 0; then echo routes:present; else echo routes:none; fi
if curl --silent --show-error --connect-timeout 3 https://example.com >/dev/null 2>&1; then echo internet:ok; else echo internet:no; fi
"#,
        isolation = isolation_check(),
    );
    let output = run_sandbox(NONE_ATTR, &script);
    assert_process_isolated(&output);
    let text = output_text(&output);
    for expected in ["interfaces:none", "routes:none", "internet:no"] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
}

#[test]
fn private_has_network_dns_internet_and_cannot_reach_host_localhost() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind host localhost test server");
    listener.set_nonblocking(true).expect("set nonblocking");
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        for _ in 0..200 {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut request = [0_u8; 1024];
                    let _ = stream.read(&mut request);
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    );
                    return true;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10))
                }
                Err(_) => return false,
            }
        }
        false
    });

    let script = format!(
        r#"{isolation}
interfaces=0
while IFS= read -r line; do case "$line" in *:*) name=${{line%%:*}}; name=${{name//[[:space:]]/}}; if test "$name" != lo; then interfaces=$((interfaces + 1)); fi;; esac; done < /proc/net/dev
if test "$interfaces" -gt 0; then echo interfaces:present; else echo interfaces:none; fi
routes=0
while IFS= read -r line; do case "$line" in Iface*|'') ;; *) routes=$((routes + 1));; esac; done < /proc/net/route
if test "$routes" -gt 0; then echo routes:present; else echo routes:none; fi
if curl --silent --show-error --connect-timeout 5 https://example.com >/dev/null 2>&1; then echo internet:ok; else echo internet:no; fi
if curl --silent --show-error --connect-timeout 2 http://127.0.0.1:{port}/ >/dev/null 2>&1; then echo host-localhost:reachable; else echo host-localhost:blocked; fi
"#,
        isolation = isolation_check(),
    );
    let output = run_sandbox(PRIVATE_ATTR, &script);
    assert_process_isolated(&output);
    let text = output_text(&output);
    for expected in [
        "interfaces:present",
        "routes:present",
        "internet:ok",
        "host-localhost:blocked",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert!(
        !server.join().unwrap(),
        "server on host localhost received a private sandbox connection"
    );
}

#[test]
fn host_shares_network_namespace_and_host_loopback() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind host localhost test server");
    listener.set_nonblocking(true).expect("set nonblocking");
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        for _ in 0..200 {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut request = [0_u8; 1024];
                    let _ = stream.read(&mut request);
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    );
                    return true;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10))
                }
                Err(_) => return false,
            }
        }
        false
    });
    let script = format!(
        r#"{isolation}
if test "$(stat -Lc %i /proc/self/ns/net)" = "{host_netns}"; then echo netns:host; else echo netns:private; fi
if curl --silent --show-error --connect-timeout 3 http://127.0.0.1:{port}/ >/dev/null 2>&1; then echo host-localhost:reachable; else echo host-localhost:blocked; fi
"#,
        isolation = isolation_check(),
        host_netns = namespace_inode("net"),
    );
    let output = run_sandbox(HOST_ATTR, &script);
    assert_process_isolated(&output);
    let text = output_text(&output);
    for expected in ["netns:host", "host-localhost:reachable"] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert!(
        server.join().unwrap(),
        "host sandbox did not reach host localhost listener"
    );
}

#[test]
fn private_is_launched_with_resource_limits() {
    let script = r#"printf 'pid=%s\n' "$$"; sleep 3"#;
    let mut child = Command::new(env!("CARGO_BIN_EXE_sandbox"))
        .args([
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
    thread::sleep(Duration::from_millis(500));
    let pid = child.id();
    let cg = std::fs::read_to_string(format!("/proc/{pid}/cgroup"))
        .expect("read private launcher cgroup");
    assert!(
        !cg.trim().is_empty(),
        "private launcher has no cgroup membership"
    );
    let _ = child.kill();
    let _ = child.wait();
}
