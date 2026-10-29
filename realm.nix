{
  nixpkgs ? builtins.getFlake "nixpkgs",
  system ? builtins.currentSystem,
}:
let
  pkgs = import nixpkgs.sourceInfo.outPath { inherit system; };
  realm = builtins.getFlake "git+file:///home/yuya/projects/20_sandbox/realm/";
  home = realm.lib.home;
in
{
  agent = {
    packages = with pkgs; [
      bash

      coreutils
      git
      ripgrep
      fd
      curl
      cacert
      gnused
      gnugrep

      cargo
      rustc
      rustfmt
      clippy
      clang
      mold

      nix

      opencode
    ];
    env = {
      CARGO_HOME = "/home/sandbox/.cargo";
    };
    network = "private";
    resources = {
      enable = true;
      backend = "cgroup";
      memory = "2G";
      cpu = "100%";
      tasks = 512;
      timeout = "1h";
    };
    mounts = [
      {
        host = home.globalPath ".cargo";
        guest = "/home/sandbox/.cargo";
        mode = "rw";
      }
      {
        host = home.globalPath ".config/opencode";
        guest = "/home/sandbox/.config/opencode";
        mode = "rw";
      }
      {
        host = home.globalPath ".local/share/opencode";
        guest = "/home/sandbox/.local/share/opencode";
        mode = "rw";
      }
      {
        host = home.globalPath ".cache/opencode";
        guest = "/home/sandbox/.cache/opencode";
        mode = "rw";
      }
    ];
  };
}
