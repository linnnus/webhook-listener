let
  # Normally the max-idle time would probably be a little longer than this, but
  # I don't want to drag out this test for 10 minutes.
  max-idle-secs = 20;
in
{
  name = "socket-usage";

  nodes.machine = { pkgs, config, lib, self, ... }: {
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

      max-idle-time = "${toString max-idle-secs}s";

      # The secret to be used when authenticating event's signature.
      secret-path = toString (pkgs.writeText "secret.txt" "mysecret");
    };

    environment.systemPackages = [
      (pkgs.writeShellScriptBin "send-push-event.sh" ''
        ${pkgs.curl}/bin/curl ${lib.escapeShellArgs [
          # Connection details
          "--unix-socket" config.services.webhook-listener.socket-path
          "http://localhost/"

          # All the data our application needs for a push event.
          "-X" "POST"
          "--data" (builtins.readFile ../examples/sample_push_payload.json)
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
    import time

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

    with subtest("Service should exit after idle time"):
      # Give it a little buffer to avoid false negatives.
      time.sleep(${toString max-idle-secs} + 5)

      exit_code, _ = machine.systemctl("is-active webhook-listener.service --quiet")
      assert exit_code == 3, "Event should be inactive"

    # TODO: Send an invalid request (subtest).
  '';
}
