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
        webhook-listener = final.callPackage ./nix/package.nix { inherit version; };
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
        webhook-listener = import ./nix/module.nix self;

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

            # This makes `self` available in the NixOS configuration of our virtual machines.
            node.specialArgs = { inherit self; };

            # Each module in this list is a test (?).
            imports = [
              ./nix/test-socket-usage.nix
            ];
          });
        });
    };
}

