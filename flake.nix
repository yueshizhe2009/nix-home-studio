{
  description = "Nix Home Studio — a beginner-friendly Home Manager editor";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      # Keep the read-only upstream clones and build output out of the store
      # path. The application itself is only a few megabytes.
      src = nixpkgs.lib.cleanSourceWith {
        src = ./.;
        filter =
          path: _type:
          let
            base = baseNameOf (toString path);
          in
          !(builtins.elem base [
            ".git"
            ".direnv"
            "upstream"
            "target"
            "result"
            "result-"
            "node_modules"
            "docs"
            "README.md"
            "AGENTS.md"
            "nhs-gui-spike-tauri"
            "nhs-gui-spike-iced"
          ])
          && !(nixpkgs.lib.hasSuffix ".log" base);
      };
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          nhs-cli = pkgs.callPackage ./nix/cli.nix { inherit src; };
          nhs-gui = pkgs.callPackage ./nix/gui.nix { inherit src; };
          default = self.packages.${system}.nhs-gui;
        }
      );

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.nhs-gui}/bin/nhs-gui";
        };
        nhs-cli = {
          type = "app";
          program = "${self.packages.${system}.nhs-cli}/bin/nhs";
        };
      });

      checks = forAllSystems (system: {
        # Release metadata must not claim platforms that have not been built.
        metadata = nixpkgs.legacyPackages.${system}.runCommand "nhs-metadata-check" {
          cliPlatforms = builtins.toJSON self.packages.${system}.nhs-cli.meta.platforms;
          guiPlatforms = builtins.toJSON self.packages.${system}.nhs-gui.meta.platforms;
          cliVersion = self.packages.${system}.nhs-cli.version;
          guiVersion = self.packages.${system}.nhs-gui.version;
        } ''
          test "$cliPlatforms" = '["x86_64-linux"]'
          test "$guiPlatforms" = '["x86_64-linux"]'
          test "$cliVersion" = "$guiVersion"
          touch $out
        '';
      });

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixfmt-rfc-style);
    };
}
