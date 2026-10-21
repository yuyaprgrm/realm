# realm

`realm` is a Nix-native Linux sandbox runtime for agent workloads. It reads
sandbox definitions from `realm.nix`; Nix evaluates the configuration and
realizes its package closure outside the sandbox. Bubblewrap contains only the
agent process tree. See [SECURITY.md](SECURITY.md) for the trust boundary and
threat model.

## Requirements

- Linux with user, PID, mount, cgroup, and network namespaces enabled.
- Nix with flakes enabled.
- A `nixpkgs` flake registry entry (the default Nix registry normally provides
  one) so Nix can evaluate package names in `realm.nix`.
- bubblewrap (`bwrap`). The Nix package and development shell provide it.
- A systemd user manager for `private` networking and full cgroup resource
  limits. Without it, `none`/`host` fall back to fail-closed rlimits and
  `timeout`; `private` refuses to run. Private mode also needs `pasta` on
  `PATH`; the Nix package provides it.

## Install and run

Install the published flake package into the current Nix profile:

```bash
nix profile install github:yuyaprgrm/realm
realm --help
```

The flake exposes `packages.<system>.realm` and `overlays.default`. Add the
overlay to Nixpkgs to use Realm as `pkgs.realm`:

```nix
{
  inputs.realm.url = "github:yuyaprgrm/realm";
  nixpkgs.overlays = [ inputs.realm.overlays.default ];
}
```

Then install it from NixOS or Home Manager with `pkgs.realm`, for example in
`environment.systemPackages`. The package wrapper adds Realm's runtime tools
(`nix`, bubblewrap, passt/pasta, and systemd) to `PATH`. `realm.nix` remains the
sandbox configuration read by the executable; it is not the package or a
NixOS/Home Manager module.

Run directly from a local checkout without installing:

```bash
nix run . -- --help
nix run . -- agent
nix run . -- agent -- bash -lc 'curl https://example.com'
```

By default, Realm reads `realm.nix` from the current directory. Select a named
sandbox with `realm <name>` and optionally pass another config file with
`--config` / `-c`. The file is a Nix function returning a set of sandbox
definitions. By default it gets Nixpkgs from the `nixpkgs` flake registry entry
and creates `pkgs` for the current system:

```nix
{ nixpkgs ? builtins.getFlake "nixpkgs"
, system ? builtins.currentSystem
}:
let pkgs = import nixpkgs.sourceInfo.outPath { inherit system; };
in
{
  agent = {
    packages = with pkgs; [ bash coreutils curl cacert ];
    env = { FOO = "bar"; };
    network = "private"; # "none", "private", or "host"
    mounts = [
      {
        host = "/host/path";
        guest = "/sandbox/path";
        mode = "ro"; # "ro" or "rw"
      }
    ];
  };
}
```

`mounts` explicitly exposes host paths inside the sandbox. Only the listed
paths are visible: `ro` mounts are read-only, `rw` mounts are writable, and
guest parent directories are created automatically. Host and guest paths must
be absolute and must not be `/`. Paths are normalized (`.`, `..`, repeated
separators) before validation, and paths that escape `/` are rejected. Host
paths must not contain symlinks. Guest targets must not overlap each other
(equal, ancestor, or descendant) and must not replace or cover the built-in
mount points (`/nix`, `/etc`, `/bin`, `/usr/bin`, `/workspace`, `/tmp`,
`/proc`, `/dev`, `/home`, `/home/sandbox`, plus closure paths); mounting
*under* a built-in tree, such as `/home/sandbox/.config`, is allowed. The
validation is path-tree based, not a special case for `/home`.

The sandbox environment is an explicit allowlist: Realm passes only `HOME`,
`TMPDIR`, `PWD`, `PATH`, and the `env` entries declared in `realm.nix`.
Host-only variables such as `SSH_AUTH_SOCK`, `DBUS_SESSION_BUS_ADDRESS`, and
`XDG_RUNTIME_DIR` are not inherited. `/proc` is a fresh procfs for the private
PID namespace, and bwrap's own process environment is cleared so
`/proc/1/environ` does not expose host values.

### Workspace and configuration trust

The workspace is mounted read-write at `/workspace` by default. This is
intentional: the agent may edit the project. It also means the agent can edit
`realm.nix` or other build inputs, which are evaluated by Nix on the host
during the *next* run. Host-side Nix evaluation/build is outside the sandbox
security boundary; see [SECURITY.md](SECURITY.md).

To separate the two trust inputs, configure the workspace explicitly and keep
the configuration outside it:

```nix
{
  agent = {
    # packages, env, network, mounts...
    workspace = {
      host = "/home/me/projects/app"; # defaults to the current directory
      mode = "rw";                    # or "ro"
    };
    requireCgroup = false;            # true = refuse to run without systemd scope
  };
}
```

`workspace.host` must be an absolute directory and follows the same host-path
rules as `mounts` (no symlinks).

When the configuration file is inside a writable workspace, Realm pins the
directory that holds it read-only inside the sandbox. That directory becomes a
mountpoint, so it cannot be renamed, and the configuration file is also overlaid
read-only. Directories between the workspace and that directory are made
mountpoints too, without becoming read-only, so an ancestor cannot be renamed
to move the guard away. When the file sits directly in the workspace root, only
the file overlay is needed because `/workspace` is already a mountpoint. This
prevents the agent from rewriting the configuration itself, but not other files
it imports. For full protection, use `mode = "ro"` or keep the configuration
outside the workspace.

Nix still evaluates package derivations, realizes them, and calculates their
runtime closure. The sandbox configuration no longer needs to be a flake output.
Use `realm --config ./ci/realm.nix agent` to select a different file.

`none` has no network interface or route. `private` gives the agent an isolated
network namespace with pasta-based DNS and Internet access; it cannot reach
services bound to host localhost. `host` shares the host network namespace.
Private mode runs inside a transient systemd user scope with defaults of 2 GiB
memory, 100% of one CPU, 512 tasks, and a one-hour wall-clock limit. A usable
systemd user manager is required for this mode.

Resource limits have an explicit failure policy:

- With a systemd user scope, memory, CPU quota, task count, and wall-clock
  time are enforced by cgroup v2.
- Without systemd, `none` and `host` use `setrlimit` for address space,
  process count, and CPU time, plus the coreutils `timeout` command for the
  wall-clock limit. If any of these cannot be installed, the sandbox refuses
  to start (fail-closed). `RLIMIT_AS` limits address space, not resident
  memory. CPU quota is not enforceable by rlimits and is degraded to the
  CPU-time cap. The process cap is `RLIMIT_NPROC`, which applies per user ID
  to the whole host (512 by default), not per sandbox; a true per-sandbox
  process cap needs cgroups, i.e. the systemd path.
- `private` always requires systemd and fails closed without it.
- Set `requireCgroup = true` to refuse all modes when no systemd scope is
  available.

Seccomp is not enabled in this version. The evaluation and rationale are in
[SECURITY.md](SECURITY.md#seccomp-evaluation).

Add `cacert` to the sandbox package list for HTTPS clients that need trusted CA
certificates. Pin or override the `nixpkgs` argument in `realm.nix` if you need a
different package set.

## Development

Enter the development environment, which includes Rust tools and the runtime
dependencies (`nix`, `bwrap`, `pasta`, and `systemd-run`):

```bash
nix develop
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The flake also supports the standard package workflows:

```bash
nix build
nix run . -- --help
nix profile install .
nix flake check
```

Network integration tests make real namespace and localhost checks and require a
Linux host that permits user namespace creation, localhost binding, Nix
evaluation, a `nixpkgs` registry entry, and a systemd user manager. `nix flake
check` runs them as part of the package checks; the tests print a `SKIP` reason
when the build sandbox does not provide these capabilities. In `nix develop`
they run against the host when the same capabilities are available.

The test suite always checks that `none` and `private` cannot reach a host-local
HTTP server while `host` can. It attempts external HTTPS in Private mode when
available; require that check explicitly with:

```bash
REALM_TEST_REQUIRE_EXTERNAL_HTTPS=1 cargo test --test network_modes private_has_network_dns_internet_and_cannot_reach_host_localhost
```
