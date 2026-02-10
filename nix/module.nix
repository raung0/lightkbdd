{ lib, config, pkgs, ... }:
let
  cfg = config.services.lightkbdd;
in
{
  options.services.lightkbdd = {
    enable = lib.mkEnableOption "lightkbdd keyboard backlight daemon";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.lightkbdd;
      defaultText = "pkgs.lightkbdd";
      description = "The lightkbdd package to run.";
    };

    extraArgs = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [];
      description = "Extra args passed to lightkbdd.";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];

    systemd.services.lightkbdd = {
      description = "lightkbdd keyboard backlight daemon";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ];

      serviceConfig = {
        ExecStart = lib.concatStringsSep " " (
          [ "${lib.getExe cfg.package}" ] ++ cfg.extraArgs
        );

        Restart = "on-failure";
        RestartSec = 2;

        User = "root";
        NoNewPrivileges = true;

        StandardOutput = "journal";
        StandardError = "journal";
      };
    };
  };
}
