{
  lib,
  rustPlatform,
  src,
}:

# The CLI is deliberately its own derivation. It has no GUI toolkit or WebKit
# dependency, so a broken WebKitGTK package cannot block the command-line
# delivery. The root workspace lock is the CLI's lock.
rustPlatform.buildRustPackage (finalAttrs: {
  pname = "nhs-cli";
  version = (lib.importTOML ../Cargo.toml).workspace.package.version;

  inherit src;

  cargoLock.lockFile = ../Cargo.lock;
  cargoBuildFlags = [ "-p" "nhs-cli" ];

  # The package is verified by `nix build` and by running `nhs discover`; the
  # repository's own test suite is run outside the packaging derivation.
  doCheck = false;

  meta = {
    description = "Command-line front end for Nix Home Studio";
    homepage = "https://github.com/yueshizhe2009/nix-home-studio";
    license = lib.licenses.asl20;
    mainProgram = "nhs";
    platforms = [ "x86_64-linux" ];
  };
})
