self:
{ pkgs, lib, config, options, ... }:

let
  defaultUser = "webhooklistener";
  defaultGroup = "webhooklistener";

  cfg = config.services.webhook-listener;
in {
  options = with lib; {
    services.webhook-listener = {
      enable = mkEnableOption "Webhook listener";

      package = mkOption {
        description = "Package containing `webhook-listener` binary.";
        type = types.package;
        default = self.packages.${pkgs.system}.webhook-listener;
      };

      user = mkOption {
        description = ''
          The user to run the qBittorrent service as. This is also the
          user that will run the command.

          The user is not automatically created if it is changed from the default value.
        '';
        type = types.str;
        default = defaultUser;
      };

      group = mkOption {
        description = ''
          The group to run the webhook listener service as. This is
          also the group that will run the command.

          The group is not automatically created if it is changed from the default value.
        '';
        type = types.str;
        default = defaultGroup;
      };

      commands = mkOption {
        description = "List of event/command pairs, which will be matched against events from GitHub";
        type = with types; listOf (submodule {
          options = {
            event = mkOption {
              description = ''
                An event from the GitHub API.

                See [the GitHub documentation](https://docs.github.com/en/webhooks/webhook-events-and-payloads) for event types and data.
              '';
              type = types.str;
              example = "push";
            };

            command = mkOption {
              description = "The command to run upon receiving webhook event from GitHub.";
              type = types.str;
              example = "run-ci-or-something";
            };

            args = mkOption {
              description = "Additional arguments to be supplied to `command`.";
              type = with types; listOf str;
              default = [];
              example = [ "--some-option" ];
            };
          };
        });
      };

      secret-path = mkOption {
        description = "Path to file containing the secret given to GitHub.";
        type = types.path;
        example = "/run/github_secret.txt";
      };

      socket-path = mkOption {
        description = ''
          Path of socket file where the server will be listening.

          You should set up a redirect with your reverse proxy such
          that a POST request from GitHub (i.e. to the webhook url you
          give to GitHub) is translated to a request to `/` on this socket.
        '';
        type = types.path;
        readOnly = true;
      };

      max-idle-time = mkOption {
        description = ''
          Maximum time the server should wait for a new connection before exiting.

          In conjunction with socket-activation, this ensures the server isn't
          using any ressources in the (typically) long periods of time between
          requests.

          The server will never exit, if this option is set to `null`.
        '';
        type = with types; nullOr str;
        default = null;
        example = "20min";
      };
    };
  };

  config = lib.mkIf cfg.enable {
    # Create the user/group if required.
    users.users = lib.mkIf (cfg.user == defaultUser) {
      ${defaultUser} = {
        description = "Runs ${options.services.webhook-listener.enable.description}";
        group = cfg.group;
        isSystemUser = true;
      };
    };
    users.groups = lib.mkIf (cfg.group == defaultGroup) {
      ${defaultGroup} = {};
    };

    # Create socket for server.
    services.webhook-listener.socket-path = "/run/webhook-listener.sock";
    systemd.sockets."webhook-listener" = {
      unitConfig = {
        Description = "Socket for receiving webhook requests from GitHub";
        PartOf = [ "webhook-listener.service" ];
      };

      socketConfig = {
        ListenStream = config.services.webhook-listener.socket-path;
      };

      wantedBy = [ "sockets.target" ];
    };

    # Create the listening server
    systemd.services.webhook-listener = {
      unitConfig = {
        Description = "listening for webhook requests from GitHub";
        After = [ "network.target" "webhook-listener.socket" ];
        # Otherwise unit would need to create socket itself if started manually.
        Requires = [ "webhook-listener.socket" ];
      };

      serviceConfig =
        let
          config = {
            "secret_path" = cfg.secret-path;
            "commands" = cfg.commands;
            "max_idle_time" = cfg.max-idle-time;
          };

          config-file = pkgs.writers.writeJSON "config.json" config;
        in
        {
          Type = "simple";
          User = cfg.user;
          Group = cfg.group;
          ExecStart = "${cfg.package}/bin/webhook-listener ${config-file}";
        };
    };
  };
}
