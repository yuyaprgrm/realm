# Security Model

## Trust boundary

```
trusted host / caller
        │
        ├── realm.nix + Nixpkgs ──► nix eval / build / realization   (OUTSIDE sandbox)
        │                                  │
        │                                  ▼
        │                          closure + sandbox spec
        │                                  │
        │                                  ▼
        └── workspace (RW by default) ──► bwrap   (only the agent is contained)
                                           │
                                           ▼
                                  untrusted agent process tree
```

- **Outside the sandbox (trusted):** `nix eval`, `nix build`, realization
  (`nix-store --realise`), closure calculation (`nix path-info --recursive`),
  the selected `realm.nix`, the Nixpkgs package set it imports, and anything
  the host later executes.
- **Inside the sandbox (contained):** only the agent command/process tree
  launched by bwrap. The agent is untrusted relative to the host.

**Evaluating or building arbitrary untrusted Nix expressions is not a current
security goal.** `realm.nix` and the package set it imports execute outside the
agent sandbox. Supporting untrusted configuration would require sandboxing Nix
evaluation and builds separately.

## Workspace and configuration trust

The workspace is mounted read-write at `/workspace` by default. This is an
intentional capability: the agent is expected to edit the project. It has two
consequences:

1. Changes made by the agent persist on the host.
2. If the workspace contains `realm.nix`, build definitions, or other inputs to
   a later host-side Nix evaluation/build, the agent can change trusted input.

The resulting attack chain is:

```
agent writes /workspace/realm.nix (or an imported file)
        ↓
agent run ends; workspace remains on the host
        ↓
host runs realm / nix build again
        ↓
Nix evaluates or builds the changed configuration OUTSIDE the sandbox
        ↓
arbitrary host-side code execution
```

This is not a runtime escape from bwrap. It is a trust-boundary violation
between the sandbox and the next host-side evaluation. Callers must treat
workspace content as untrusted input when deciding what to evaluate or build.

Configuration can be separated from the workspace and/or protected:

```nix
{
  workspace = {
    host = "/home/me/projects/app"; # separate from the config location
    mode = "ro";                    # read-only workspace
  };
}
```

If the configuration file itself is inside a writable workspace, Realm mounts
the directory that holds it read-only, so the directory cannot be renamed, and
also overlays the configuration file read-only. Directories between the
workspace and that directory are re-bound read-write on themselves, making them
non-renamable mountpoints without making them read-only. When the file sits
directly in the workspace root, only the file overlay is added because
`/workspace` is already a mountpoint. This blocks rewriting the config file
during the run, but does **not** protect sibling files that the config imports.
For full protection, use `mode = "ro"` or keep the configuration outside the
workspace.

## What is protected

- **Namespaces:** user/pid/uts/ipc/net/cgroup, `--new-session`,
  `--disable-userns` where available. cgroup namespace creation is fail-closed
  (`--unshare-cgroup`), so the host cgroup hierarchy is not silently exposed.
- **Capabilities:** dropped (no new privileges), `CapEff` zero inside the
  sandbox.
- **Devices:** minimal `/dev` via bwrap (`--dev`); no host device nodes are
  exposed by the built-in sandbox.
- **Filesystem exposure:** no host `/home`, `/etc`, `/bin`, `/usr` bind mounts;
  generated guest `/etc` + compatibility symlinks; only closure paths and the
  workspace are bound by default. Host Nix store paths are read-only; new
  writes in `/nix/store` cannot reach the host store.
- **Explicit mounts:** additional host paths are exposed only when listed in
  `mounts`, with validated absolute paths and guest-path overlap checks (see
  below). Configuration is trusted, so an explicit mount can intentionally
  expose sensitive host paths.
- **Environment:** only `HOME`, `TMPDIR`, `PWD`, `PATH`, and declared `env`
  entries reach the agent. The bwrap process environment is cleared, so
  `/proc/1/environ` is not a channel for inherited host variables. `/proc` is
  a fresh procfs for the sandbox PID namespace.
- **Resources:** cgroup v2 limits via a transient systemd user scope, with a
  fail-closed rlimit/timeout fallback when systemd is unavailable (see below).
  `/tmp` and `/home/sandbox` tmpfs are size-limited.
- **Network:** `None` has no network namespace access, `Private` uses a
  pasta-backed private namespace, and `Host` shares the host network namespace.
  DNS/CA material is only prepared when network access is allowed.
- **Process isolation:** `--die-with-parent` prevents orphaned processes
  escaping cleanup.

## Mount specification

Mount destinations are treated as a path tree, not as special cases:

- Host paths and guest paths have separate rules. Guest paths are normalized
  lexically (`.`, `..`, repeated separators) and validated purely inside the
  sandbox tree. Host paths are normalized, canonicalized, and rejected if the
  declared path contains a symlink.
- Paths that escape `/` are rejected, as are paths equal to `/`.
- User mounts reject exact overlap, ancestor → descendant, and descendant →
  ancestor relationships with each other.
- User mounts must not equal a reserved sandbox mount point or be an ancestor of
  one. Mounting *below* a reserved point (for example
  `/home/sandbox/.config/foo`) is allowed because the reserved mount remains a
  boundary.
- Error messages name both paths and explain whether the conflict is an
  overlap, a reserved path replacement, an ancestor covering a reserved path,
  an inaccessible host path, or a symlink.

**Residual TOCTOU:** validation and bwrap execution are separate system calls.
A caller that can concurrently replace a validated host directory with a
symlink can race the mount. Realm currently rejects symlinks at validation time
and passes canonical host paths, but does not hold open file descriptors across
the exec. Host mount sources are therefore part of the trusted configuration;
do not point them at attacker-controlled paths that change concurrently.

## Environment isolation

Host environment variables are not inherited by the sandbox:

- The direct bwrap path calls `env_clear` on the bwrap process and bwrap uses
  `--clearenv` before adding the explicit `--setenv` entries.
- The systemd/pasta wrapper paths execute bwrap through `env -i --`, so even
  the variables needed to reach the systemd user manager are not visible from
  inside the sandbox.
- Only the generated guest `/etc` is mounted; host NSS configuration and host
  `/proc` are not exposed.
- A regression test sets host-only markers (`REALM_HOST_LEAK`, `SSH_AUTH_SOCK`,
  `DBUS_SESSION_BUS_ADDRESS`, `XDG_RUNTIME_DIR`) on the realm process and
  asserts that none appear in `/proc/1/environ` or any sandbox process.

## Resource limits and failure policy

| Control | systemd user scope | No systemd | Failure policy |
| --- | --- | --- | --- |
| Memory | `MemoryMax` (cgroup v2) | `RLIMIT_AS` (address space, not resident memory) | fail-closed if `setrlimit` fails |
| Task/process count | `TasksMax` (per cgroup) | `RLIMIT_NPROC` (per user ID, host-wide) | fail-closed if `setrlimit` fails |
| Wall-clock timeout | `RuntimeMaxSec` | coreutils `timeout` wrapper | fail-closed if `timeout` is missing |
| CPU quota | `CPUQuota` | not enforceable; only `RLIMIT_CPU` seconds | degraded, documented as resource control |
| `/tmp`, `/home/sandbox` size | bwrap `--size` tmpfs | bwrap `--size` tmpfs | enforced by bwrap in all modes |
| Private network | systemd scope required | refused | fail-closed |
| All cgroup limits | required when `requireCgroup = true` | refused | fail-closed |

Address-space and process-count limits are treated as DoS containment and are
fail-closed in the fallback. `RLIMIT_AS` caps address space, not resident
memory. `RLIMIT_NPROC` applies per user ID to the whole host, not per sandbox;
a true per-sandbox process cap requires cgroups (the systemd path). CPU quota
is a resource-control feature rather than a host-isolation boundary; without
systemd the `RLIMIT_CPU` cap applies but the percentage quota is not emulated.
`private` networking is fail-closed because pasta and the sandbox must share
the resource scope.

## Seccomp evaluation

No seccomp filter is installed in this version. The primary boundary is Linux
namespaces, capability drop, filesystem/network isolation, and
`--disable-userns`; `/proc/self/status` reports `Seccomp: 0`.

The runtime surface is broad: Nix store executables, bash/coreutils, git,
curl, Rust/clippy, clang, and opencode may use `clone3`, `io_uring`,
`userfaultfd`, `bpf`, `perf_event_open`, `keyctl`, `ptrace`, namespace and
mount syscalls during normal or diagnostic work. A fixed syscall allowlist is
therefore likely to break legitimate runtimes, and a deny-list requires
per-runtime testing to avoid surprising failures.

**Decision:** seccomp is defense-in-depth, not a mandatory property of this
version. If introduced later, it should use runtime presets (agent, Nix build,
debugging) with a conservative deny-list and an opt-in strict allowlist, plus
regression tests that exercise the supported commands and assert that denied
syscalls return `EPERM`. Until then, kernel attack surface reduction relies on
namespaces and the absence of capabilities rather than a syscall filter.

## What is not protected

- **Host-side Nix evaluation/build:** an attacker-controlled `realm.nix` or
  imported build input can execute arbitrary code during `nix eval`/`nix build`
  outside this sandbox.
- **Host trust of configuration:** the selected `realm.nix`, Nixpkgs package
  set, explicit mount sources, and Nix tooling are part of the trusted base.
- **Workspace persistence:** agent changes remain on the host after the run and
  may affect later tooling. See the attack chain above.
- **Concurrent host-path races:** see the mount TOCTOU note.
- **Side channels:** no claim is made about CPU cache, timing, or other
  side-channel attacks.
- **Kernel vulnerabilities:** the sandbox has no seccomp layer, so kernel
  vulnerabilities reachable from allowed syscalls are in scope.
- **Non-Linux platforms:** the runtime is Linux-only (see below).

## Code-level boundary markers

- **Configuration trust boundary:** `nix::build_spec` assembles the sandbox
  from trusted `realm.nix` evaluation results (outside sandbox). Callers must
  treat the configuration as trusted.
- **Mount boundary:** `nix::user_mounts` normalizes and validates host/guest
  paths as a tree; `nix::config_guard_mount` pins the config's directory and
  file read-only when they are inside a writable workspace.
- **Runtime boundary:** `sandbox::run` is the only entry point that launches the
  contained agent; `sandbox::cgroup::exec_with_limits`/bwrap enforce containment
  and the resource failure policy.
- **Environment boundary:** `sandbox::bwrap::build_command` clears the bwrap
  process environment; wrapper paths use `sandbox::cgroup::clean_bwrap_command`.

# Platform Support

- **Runtime:** The sandbox runtime requires **Linux** (bubblewrap + Linux
  namespaces/cgroups). On non-Linux hosts, `sandbox::run` returns a clear error.
- **Nix outputs:** `flake.nix` defines the Realm package, app, checks, and
  development shell. Sandbox definitions live in `realm.nix`, independently of
  those outputs.
