{ config, lib, pkgs, ... }: {
  options.services.attic-client = {
    relay.enable = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Enable relay.";
    };
    daemon = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = "Enable daemon.";
      };
      cache = lib.mkOption {
        type = lib.types.str;
        description = "Name of server and cache to upload to";
      };
    };
  };

  config = let
    cmd = "${pkgs.writeShellScriptBin "attic-queue-relay.sh" "${pkgs.attic}/bin/attic queue relay"}/bin/attic-queue-relay.sh";
    dir = "/var/lib/attic-client";
    userName = "attic-client";
    groupName = "attic-client";
  in {
    nix.extraOptions = lib.mkIf config.services.attic-client.relay.enable ''
      post-build-hook = ${cmd}
    '';

    users = lib.mkIf config.services.attic-client.daemon.enable {
      users."${userName}" = {
        isSystemUser = true;
        group = groupName;
        home = dir;
        createHome = true;
      };
      groups."${groupName}" = {};
    };

    environment.systemPackages = lib.mkIf (config.services.attic-client.daemon.enable || config.services.attic-client.relay.enable) [pkgs.attic];

    systemd.services.attic-client = lib.mkIf config.services.attic-client.daemon.enable {
      path = [pkgs.attic];
      serviceConfig = {
        User = userName;
        Group = groupName;
        WorkingDirectory = dir;
        ExecStart = "${pkgs.attic}/bin/attic queue daemon ${config.services.attic-client.daemon.cache}";
        Restart = "on-failure";
      };
      wantedBy = ["multi-user.target"];
      after = ["network.target"];
    };
  };
}
