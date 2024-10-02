{ rustPlatform
, version
}:

rustPlatform.buildRustPackage {
  pname = "webhook-listener";
  inherit version;
  src = ../.;
  cargoLock.lockFile = ../Cargo.lock;

  # Tests in systemd_socket are extremely finicky, so they cannot be run in parallel with other unit tests.
  checkPhase = ''
    cargo test -- --test-threads=1
  '';
}
