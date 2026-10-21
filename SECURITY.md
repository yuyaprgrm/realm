# Security Model

## Trust boundary

```
trusted sandbox configuration
           ↓
      nix eval/build          ← runs OUTSIDE the agent sandbox
           ↓
         realization & closure
           ↓
           bwrap               ← only the agent process tree is contained
           ↓
     untrusted agent
```

- **Outside the sandbox (trusted):** `nix eval`, `nix build`, realization (`nix-store --realise`), and closure calculation (`nix path-info --recursive`). These evaluate the caller-selected `realm.nix` and the Nixpkgs package set it imports.
- **Inside the sandbox (contained):** only the agent command/process tree launched by bwrap. The agent is treated as untrusted relative to the host.

**Evaluating or building arbitrary untrusted Nix expressions is not a current security goal.** `realm.nix` and the package set it imports execute outside the agent sandbox; supporting untrusted configuration would require sandboxing Nix evaluation and builds separately.

## What is protected (current)

- **Namespaces:** user/pid/uts/ipc/net/cgroup, `--new-session`, `--disable-userns` where available.
- **Capabilities:** dropped (no new privileges), `CapEff` zero inside the sandbox.
- **Devices:** minimal `/dev` via bwrap (`--dev`), no host device nodes bound.
- **Filesystem exposure:** no host `/home`, `/etc`, `/bin`, `/usr` bind mounts; generated guest `/etc` + compatibility symlinks; only closure paths + workspace bound read-only/read-write as needed. Additional host paths are exposed only when listed explicitly in `mounts` (`ro` or `rw`), with validated absolute paths, no symlinks or `..`, and no overlap with built-in or other guest mounts. Host Nix store paths are read-only; new writes in `/nix/store` cannot reach the host store.
- **Resources:** cgroup v2 limits via transient systemd user scope (memory, CPU, tasks/pids, and wall-clock time) with `setrlimit` fallback when systemd is unavailable; `/tmp` and `/home/sandbox` tmpfs size-limited.
- **Network:** `None` has no network namespace access, `Private` uses a pasta-backed private namespace, and `Host` shares the host network namespace. DNS/CA material is only prepared when network access is allowed. Private mode requires a working systemd user manager so its resource scope can contain pasta and the sandbox together.
- **Process isolation:** `--die-with-parent` prevents orphaned processes escaping cleanup.

## What is not protected (current)

- **Nix evaluation/build:** an attacker-controlled `realm.nix` can execute arbitrary code during `nix eval`/`nix build` outside this sandbox.
- **Host trust of configuration:** the selected `realm.nix`, Nixpkgs package set, and Nix tooling are part of the trusted base.
- **Side channels:** no claim is made about CPU cache, timing, or other side-channel attacks.
- **Non-Linux platforms:** the runtime backend is Linux-only (see below).

## Code-level boundary markers

- **Configuration trust boundary:** `nix::build_spec` assembles the sandbox from trusted `realm.nix` evaluation results (outside sandbox). Callers must treat the configuration as trusted.
- **Runtime boundary:** `sandbox::run` is the only entry point that launches the contained agent; `sandbox::cgroup::exec_with_limits`/bwrap enforce containment.
- **Extension point:** `sandbox::backend` defines `SandboxBackend` for future backends without moving Nix eval/build inside the current sandbox.

# Platform Support

- **Runtime:** The sandbox runtime requires **Linux** (bubblewrap + Linux namespaces/cgroups). On non-Linux hosts, `sandbox::run` returns a clear error.
- **Nix outputs:** `flake.nix` defines the Realm package, app, checks, and development shell. Sandbox definitions live in `realm.nix`, independently of those outputs.
- **Backends:** The backend is abstracted in `sandbox::backend` to allow future platform-specific implementations without conflating "Nix platform support" with "runtime sandbox capability".
