# realm

`realm` is a Nix-native Linux sandbox runtime for agent workloads. It reads
sandbox definitions from `realm.nix`; Nix evaluates the configuration and
realizes its package closure outside the sandbox. Bubblewrap contains only the
agent process tree. See [SECURITY.md](SECURITY.md) for the trust boundary and
threat model.

## Requirements

- Linux with user, PID, mount, and network namespaces enabled.
- Nix with flakes enabled.
- A `nixpkgs` flake registry entry (the default Nix registry normally provides
  one) so Nix can evaluate package names in `realm.nix`.
- bubblewrap (`bwrap`). The Nix package and development shell provide it.
- A systemd user manager for `private` networking and cgroup resource limits.
  Private mode also needs `pasta` on `PATH`; the Nix package provides it.

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
  };
}
```

Nix still evaluates package derivations, realizes them, and calculates their
runtime closure. The sandbox configuration no longer needs to be a flake output.
Use `realm --config ./ci/realm.nix agent` to select a different file.

`none` has no network interface or route. `private` gives the agent an isolated
network namespace with pasta-based DNS and Internet access; it cannot reach
services bound to host localhost. `host` shares the host network namespace.
Private mode runs inside a transient systemd user scope with defaults of 2 GiB
memory, 100% of one CPU, 512 tasks, and a one-hour wall-clock limit. A usable
systemd user manager is required for this mode.

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
evaluation, a `nixpkgs` registry entry, and a systemd user manager.

The test suite always checks that `none` and `private` cannot reach a host-local
HTTP server while `host` can. It attempts external HTTPS in Private mode when
available; require that check explicitly with:

```bash
REALM_TEST_REQUIRE_EXTERNAL_HTTPS=1 cargo test --test network_modes private_has_network_dns_internet_and_cannot_reach_host_localhost
```
