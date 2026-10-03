{self}: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.vellum;
  tomlFormat = pkgs.formats.toml {};
in {
  imports = [(import ./options.nix {inherit self;})];

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [cfg.package];
    environment.etc."xdg/vellum/config.toml" = lib.mkIf (cfg.settings != {}) {
      source = tomlFormat.generate "vellum-config.toml" cfg.settings;
    };
    systemd.user.services.vellum = {
      description = "Vellum screen annotation overlay";
      after = ["graphical-session.target"];
      partOf = ["graphical-session.target"];
      wantedBy = ["graphical-session.target"];
      serviceConfig = {
        Type = "exec";
        ExecStart = "${cfg.package}/bin/vellum";
        Restart = "on-failure";
      };
    };
  };
}
