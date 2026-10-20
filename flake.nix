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

              passt
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
            packages = with pkgs; [
              bash
              coreutils
              git
              ripgrep
              fd
              opencode
              curl
              cacert
            ];
            env = {
              FOO = "bar";
            };
            network = "private";
          };
          none-test = {
            packages = with pkgs; [
              bash
              coreutils
              curl
              procps
              cacert
            ];
            env = { };
            network = "none";
          };
          private-test = {
            packages = with pkgs; [
              bash
              coreutils
              curl
              procps
              cacert
            ];
            env = { };
            network = "private";
          };
          host-test = {
            packages = with pkgs; [
              bash
              coreutils
              curl
              procps
              cacert
            ];
            env = { };
            network = "host";
          };
        }
      );
    };
}
