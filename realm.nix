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
      netcat
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
