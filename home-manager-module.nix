{ config, lib, pkgs, ... }:

let
  cfg = config.programs.realm;
in
{
  options.programs.realm = {
    enable = lib.mkEnableOption "realm";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.realm;
      defaultText = lib.literalExpression "pkgs.realm";
      description = "The realm package to install.";
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];
  };
}