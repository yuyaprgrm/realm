{
  nixpkgs ? builtins.getFlake "nixpkgs",
  system ? builtins.currentSystem,
}:
let
  pkgs = import nixpkgs.sourceInfo.outPath { inherit system; };
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
      cargo
      rustc
      rustfmt
      clippy
      clang
      mold
      gnused
      gnugrep
      bubblewrap
      passt
    ];
    env = {
      FOO = "bar";
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
        host = "/home/yuya/.config/opencode";
        guest = "/home/sandbox/.config/opencode";
        mode = "ro";
      }
      {
        host = "/home/yuya/.local/share/opencode";
        guest = "/home/sandbox/.local/share/opencode";
        mode = "rw";
      }
      {
        host = "/home/yuya/.cache/opencode";
        guest = "/home/sandbox/.cache/opencode";
        mode = "rw";
      }
    ];
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
    resources = {
      enable = true;
      backend = "rlimit";
    };
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
    resources = {
      enable = true;
      backend = "cgroup";
    };
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
    resources = {
      enable = true;
      backend = "rlimit";
    };
  };
}
