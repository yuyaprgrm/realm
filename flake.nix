{
  description = "Nix-native sandbox runtime for agent workloads";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { nixpkgs, ... }:
    let
      # Toggle installation of shell completion scripts in the package.
      enableShellCompletions = true;

      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;

      runtimePackages = pkgs: with pkgs; [
        bubblewrap
        coreutils
        nix
        passt
        systemd
      ];

      outputsBySystem = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; overlays = [ (import ./overlay.nix) ]; };
          realm = pkgs.callPackage ./package.nix { inherit enableShellCompletions; };
        in
        {
          packages.default = realm;
          apps.default = {
            type = "app";
            program = "${realm}/bin/realm";
            meta.description = "Nix-native sandbox runtime for agent workloads";
          };
          devShells.default = pkgs.mkShell {
            packages = with pkgs; [ cargo clippy rustc rustfmt ] ++ runtimePackages pkgs;
          };
          checks.default = realm;
        });
    in
    {
      overlays.default = import ./overlay.nix;
      packages = nixpkgs.lib.mapAttrs (_: value: value.packages) outputsBySystem;
      apps = nixpkgs.lib.mapAttrs (_: value: value.apps) outputsBySystem;
      devShells = nixpkgs.lib.mapAttrs (_: value: value.devShells) outputsBySystem;
      checks = nixpkgs.lib.mapAttrs (_: value: value.checks) outputsBySystem;
    };
}
