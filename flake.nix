{
  description = "Rust development environment";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { nixpkgs, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              clippy
              rustc
              rustfmt
            ];
          };
        }
      );

      sandboxes = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          agent = {
            packages = [
              pkgs.bash
              pkgs.coreutils
              pkgs.git
              pkgs.ripgrep
              pkgs.fd
              pkgs.opencode
            ];
            env = {
              FOO = "bar";
            };
            # Network policy: "disabled" (default, fully offline), "isolated" (loopback only), "internet" (future veth/bridge/NAT)
            network = "disabled";
          };
        }
      );
    };
}
