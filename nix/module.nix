{ lib, config, pkgs, ... }:
let
  cfg = config.services.kbdd;
in
{
  options.services.kbdd = {
    enable = lib.mkEnableOption "kbdd keyboard backlight daemon";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.kbdd;
      defaultText = "pkgs.kbdd";
      description = "The kbdd package to run.";
    };

    extraArgs = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [];
      description = "Extra args passed to kbdd.";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];

    systemd.services.kbdd = {
      description = "kbdd keyboard backlight daemon";
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
