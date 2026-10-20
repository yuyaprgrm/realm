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

- **Outside the sandbox (trusted):** `nix eval`, `nix build`, realization (`nix-store --realise`), and closure calculation (`nix path-info --recursive`). These rely on the caller-provided flake reference and trusted configuration.
- **Inside the sandbox (contained):** only the agent command/process tree launched by bwrap. The agent is treated as untrusted relative to the host.

**Evaluating or building arbitrary untrusted flakes is not a current security goal.** Supporting untrusted flakes would require sandboxing Nix evaluation and build in a separate, hardened sandbox first.

## What is protected (current)

- **Namespaces:** user/pid/uts/ipc/net/cgroup, `--new-session`, `--disable-userns` where available.
- **Capabilities:** dropped (no new privileges), `CapEff` zero inside the sandbox.
- **Devices:** minimal `/dev` via bwrap (`--dev`), no host device nodes bound.
- **Filesystem exposure:** no host `/home`, `/etc`, `/bin`, `/usr` bind mounts; generated guest `/etc` + compatibility symlinks; only closure paths + workspace bound read-only/read-write as needed. Host Nix store paths are read-only; new writes in `/nix/store` cannot reach the host store.
- **Resources:** cgroup v2 limits via transient systemd user scope (memory, CPU, tasks/pids) with `setrlimit` fallback; `/tmp` and `/home/sandbox` tmpfs size-limited.
- **Network:** private network namespace per policy (`Disabled`/`Isolated`/`Internet`); DNS/CA material only prepared when network access is allowed.
- **Process isolation:** `--die-with-parent` prevents orphaned processes escaping cleanup.

## What is not protected (current)

- **Nix evaluation/build:** an attacker-controlled flake can execute arbitrary code during `nix eval`/`nix build` outside this sandbox.
- **Host trust of configuration:** the flake/attrset selected and Nix tooling are part of the trusted base.
- **Side channels:** no claim is made about CPU cache, timing, or other side-channel attacks.
- **Non-Linux platforms:** the runtime backend is Linux-only (see below).

## Code-level boundary markers

- **Configuration trust boundary:** `nix::build_spec` assembles the sandbox from trusted flake evaluation results (outside sandbox). Callers must treat the flake reference as trusted.
- **Runtime boundary:** `sandbox::run` is the only entry point that launches the contained agent; `sandbox::cgroup::exec_with_limits`/bwrap enforce containment.
- **Extension point:** `sandbox::backend` defines `SandboxBackend` for future backends without moving Nix eval/build inside the current sandbox.

# Platform Support

- **Runtime:** The sandbox runtime requires **Linux** (bubblewrap + Linux namespaces/cgroups). On non-Linux hosts, `sandbox::run` returns a clear error.
- **Nix outputs:** `flake.nix` may define multi-platform outputs (e.g. `devShells`, `sandboxes`) for convenience across platforms. Those are for Nix evaluation/build environments, **not** runtime sandbox execution.
- **Backends:** The backend is abstracted in `sandbox::backend` to allow future platform-specific implementations without conflating "Nix platform support" with "runtime sandbox capability".
