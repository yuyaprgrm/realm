{
  description = "Nix-native sandbox runtime for agent workloads";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;

      runtimePackages = pkgs: with pkgs; [
        bubblewrap
        coreutils
        nix
        passt
        systemd
      ];

      mkRealm = { pkgs }:
        let
          lib = pkgs.lib;
          runtimePath = lib.makeBinPath (runtimePackages pkgs);
        in
        pkgs.rustPlatform.buildRustPackage {
          pname = "realm";
          version = "0.1.0";
          src = lib.cleanSource ./.;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = with pkgs; [ clippy makeWrapper rustfmt ];
          doCheck = true;
          checkPhase = ''
            runHook preCheck
            export PATH=${runtimePath}:$PATH
            cargo fmt --all -- --check
            cargo clippy --locked --all-targets -- -D warnings
            cargo test --locked --lib
            cargo test --locked --test network_modes --no-run
            runHook postCheck
          '';
          postInstall = ''
            wrapProgram $out/bin/realm --prefix PATH : ${runtimePath}
          '';
          meta = {
            description = "Nix-native sandbox runtime for agent workloads";
            mainProgram = "realm";
            platforms = systems;
          };
        };

      outputsBySystem = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
          realm = mkRealm { inherit pkgs; };
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
      packages = nixpkgs.lib.mapAttrs (_: value: value.packages) outputsBySystem;
      apps = nixpkgs.lib.mapAttrs (_: value: value.apps) outputsBySystem;
      devShells = nixpkgs.lib.mapAttrs (_: value: value.devShells) outputsBySystem;
      checks = nixpkgs.lib.mapAttrs (_: value: value.checks) outputsBySystem;
    };
}
