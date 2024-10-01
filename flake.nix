{
  description = "A NixOS service to start a systemd unit on GitHub pushes";

  inputs = {
    # We need unstable for support for running NixOS tests on MacOS.
    # See: <https://github.com/NixOS/nixpkgs/commit/b8698cd8d62c42cf3e2b3a95224c57173b73e494>
    nixpkgs.url = "github:NixOS/nixpkgs/master";
  };

  outputs = { self, nixpkgs }:
    let
      # Generate a version number based on flake modification.
      lastModifiedDate = self.lastModifiedDate or self.lastModified or "19700101";
      version = "${builtins.substring 0 8 lastModifiedDate}-${self.shortRev or "dirty"}";

      # Helper function to generate an attrset '{ x86_64-linux = f "x86_64-linux"; ... }'.
      supportedSystems = [ "aarch64-darwin" "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs supportedSystems (system: f system);

      # Nixpkgs instantiated for supported system types.
      nixpkgsFor = forAllSystems (system: import nixpkgs {
        inherit system; overlays = builtins.attrValues self.overlays;
      });
    in
    {
      overlays.default = final: prev: {
        webhook-listener = final.callPackage
          ({ rustPlatform }:
            rustPlatform.buildRustPackage {
              pname = "webhook-listener";
              inherit version;
              src = ./.;
              cargoLock.lockFile = ./Cargo.lock;

              # Tests in systemd_socket are extremely finicky, so they cannot be run in parallel with other unit tests.
              checkPhase = ''
                cargo test -- --test-threads=1
              '';
            }
          )
          { };
      };

      packages = forAllSystems (system: rec {
        inherit (nixpkgsFor.${system}) webhook-listener;
        default = webhook-listener;
      });

      devShells = forAllSystems (system: {
        default = nixpkgsFor.${system}.mkShell {
          inputsFrom = [ self.packages.${system}.webhook-listener ];

          packages = [
            # Systemfd is useful for testing systemd socket activation.
            nixpkgsFor.${system}.systemfd
          ];

          shellHook = ''
            set -o vi

            export RUST_BACKTRACE=1
          '';
        };
      });

      nixosModules = {
        webhook-listener = { pkgs, lib, config, options, ... }: let
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

              secret-file = mkOption {
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
                    "secret_file" = cfg.secret-file;
                    "commands" = cfg.commands;
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
        };

        default = self.nixosModules.webhook-listener;
      };

      checks = forAllSystems (system:
        let
          pkgs = nixpkgsFor.${system};
          lib = pkgs.lib;
          nixos-lib = import "${pkgs.path}/nixos/lib" { };
        in
        {
          # Run the Cargo tests.
          webhook-listener = pkgs.webhook-listener.overrideAttrs (_: {
            doCheck = true;
          });

          vm-test = (nixos-lib.runTest {
            hostPkgs = pkgs;

            # This speeds up the evaluation by skipping evaluating documentation (optional)
            defaults.documentation.enable = lib.mkDefault false;

            # Each module in this list is a test (?).
            imports = [
              {
                name = "handles-valid-event";

                nodes.machine = { pkgs, config, lib, ... }: {
                  imports = [ self.nixosModules.webhook-listener ];

                  services.webhook-listener = {
                    enable = true;

                    commands = [
                      # We will use the file created by this command as a marker of a received event.
                      {
                        event = "push";
                        command = "touch";
                        args = ["/tmp/received-push-event"];
                      }
                    ];

                    # The secret to be used when authenticating event's signature.
                    secret-file = toString (pkgs.writeText "secret.txt" "mysecret");
                  };

                  environment.systemPackages = [
                    (pkgs.writeShellScriptBin "send-push-event.sh" ''
                      ${pkgs.curl}/bin/curl ${lib.escapeShellArgs [
                        # Connection details
                        "--unix-socket" config.services.webhook-listener.socket-path
                        "http://localhost/"

                        # All the data our application needs for a push event.
                        "-X" "POST"
                        "--data" (builtins.readFile ./examples/sample_push_payload.json)
                        "-H" "X-Github-Event: push"
                        "-H" "X-Hub-Signature-256: sha256=6803d2a3e495fc4bd286d428ea4b794476a1ff1b72bbea4dfafd2477d5d89188"
                        "-H" "Content-Length: 7413"
                        "-H" "Content-Type: application/json"

                        # We want detailed output but no smart output tricks
                        # which 100% break under the 2-3 layers of translation
                        # they undergo during interactive testing.
                        "--verbose"
                        "--no-progress-meter"

                        # Fail the command if the request is rejected. This is
                        # important for use with `Machine.succeed`.
                        "--fail"
                      ]}
                    '')
                  ];

                  system.stateVersion = "24.05";
                };

                # Open shell for interactive testing
                interactive.nodes.machine = {
                   services.openssh = {
                     enable = true;
                     settings = {
                       PermitRootLogin = "yes";
                       PermitEmptyPasswords = "yes";
                     };
                   };

                   security.pam.services.sshd.allowNullPassword = true;

                   virtualisation.forwardPorts = [
                     { from = "host"; host.port = 2000; guest.port = 22; }
                   ];
                };

                testScript = ''
                  machine.start()

                  with subtest("Proper (lazy) socket activation"):
                    machine.wait_for_unit("webhook-listener.socket")
                    exit_code, _ = machine.systemctl("is-active webhook-listener.service --quiet")
                    # According to systemctl(1): "Returns an exit code 0 if at least one is active, or non-zero otherwise."
                    # Combined with table 3, we get $? == 3 => inactive.
                    # See: <https://www.commandlinux.com/man-page/man1/systemctl.1.html>
                    assert exit_code == 3, "Event should be inactive"

                  with subtest("Sending valid request"):
                    machine.succeed("send-push-event.sh")
                    machine.wait_for_file("/tmp/received-push-event")

                  with subtest("Service should be activated after request"):
                    exit_code, _ = machine.systemctl("is-active webhook-listener.service --quiet")
                    assert exit_code == 0, "Event should be active"

                  # TODO: Send an invalid request (subtest).
                '';
              }
            ];
          });
        });
    };
}

