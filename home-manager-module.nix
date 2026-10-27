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

    enableBashIntegration = lib.hm.shell.mkBashIntegrationOption { inherit config; };

    enableZshIntegration = lib.hm.shell.mkZshIntegrationOption { inherit config; };

    enableFishIntegration = lib.hm.shell.mkFishIntegrationOption { inherit config; };
  };

  config = lib.mkIf cfg.enable (lib.mkMerge [
    {
      home.packages = [ cfg.package ];
    }

    (lib.mkIf cfg.enableBashIntegration {
      programs.bash.initExtra = lib.mkOrder 200 ''
        source <(${cfg.package}/bin/realm completion bash)
      '';
    })

    (lib.mkIf cfg.enableZshIntegration {
      programs.zsh.initContent = lib.mkOrder 200 ''
        source <(${cfg.package}/bin/realm completion zsh)
      '';
    })

    (lib.mkIf cfg.enableFishIntegration {
      programs.fish.interactiveShellInit = lib.mkOrder 200 ''
        ${cfg.package}/bin/realm completion fish | source
      '';
    })
  ]);
}