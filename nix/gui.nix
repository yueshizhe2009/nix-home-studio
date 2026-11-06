{
  lib,
  rustPlatform,
  pkg-config,
  wrapGAppsHook3,
  copyDesktopItems,
  makeDesktopItem,
  gtk3,
  webkitgtk_4_1,
  libsoup_3,
  glib,
  gsettings-desktop-schemas,
  adwaita-icon-theme,
  src,
}:

let
  desktop = makeDesktopItem {
    name = "nix-home-studio";
    desktopName = "Nix Home Studio";
    genericName = "Home Manager editor";
    comment = "Browse packages, review a diff, and apply a Home Manager module";
    exec = "nhs-gui";
    icon = "nix-home-studio";
    categories = [ "Utility" ];
    terminal = false;
  };
in
# The GUI has its own workspace and lock. It is packed separately from the CLI
# so that WebKitGTK or GTK problems are isolated to this derivation.
rustPlatform.buildRustPackage (finalAttrs: {
  pname = "nix-home-studio";
  version = "0.1.0";

  inherit src;

  cargoLock.lockFile = ../crates/nhs-gui/Cargo.lock;
  # The GUI is its own workspace, so both the vendor lock check and the build
  # must happen below `crates/nhs-gui`; otherwise Cargo would use the root
  # workspace lock and miss the GUI's dependency graph.
  cargoRoot = "crates/nhs-gui";
  buildAndTestSubdir = "crates/nhs-gui";

  nativeBuildInputs = [
    pkg-config
    wrapGAppsHook3
    copyDesktopItems
  ];

  buildInputs = [
    gtk3
    webkitgtk_4_1
    libsoup_3
    glib
    gsettings-desktop-schemas
    adwaita-icon-theme
  ];

  # Packaging only proves that the binary builds and installs. The full flow is
  # exercised separately by `--check` and `--flow-check`.
  doCheck = false;

  desktopItems = [ desktop ];

  postInstall = ''
    install -Dm644 ${src}/crates/nhs-gui/icons/icon.png \
      $out/share/icons/hicolor/32x32/apps/nix-home-studio.png
  '';

  meta = {
    description = "Nix Home Studio desktop application";
    homepage = "https://github.com/yueshizhe2009/nix-home-studio";
    license = lib.licenses.asl20;
    mainProgram = "nhs-gui";
    platforms = lib.platforms.linux;
  };
})
