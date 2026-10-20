# realm

Nix-native sandbox runtime for executing agent workloads in an isolated Linux environment.

## Security model

The sandbox isolates only the **untrusted agent process tree**. Nix evaluation, build, realization, and closure calculation run **outside** the sandbox and trust the provided configuration.

See [SECURITY.md](SECURITY.md) for the complete trust boundary and threat model.

## Requirements

- **Linux only.** The runtime requires bubblewrap, Linux namespaces, and cgroup v2. On non-Linux hosts, the CLI returns a clear error.
- **Nix** (for evaluating the flake and obtaining runtime closures)
- **bubblewrap** (`bwrap`)
- **systemd** (optional but recommended: enables cgroup v2 limits via transient user scope)

## Usage

```bash
sandbox <FLAKE_REF> [-- command...]
```

The flake's `sandbox.<system>.<name>` output (or equivalent) defines packages and environment variables for the sandboxed session. The agent command runs under a hardened bwrap setup with resource limits.

## Platform support

- **Runtime sandboxing:** Linux only.
- **Nix flake outputs:** may include non-Linux targets (for evaluation/build convenience) — these do not imply runtime support on those platforms.
