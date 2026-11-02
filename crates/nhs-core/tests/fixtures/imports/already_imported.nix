{ pkgs, ... }:
{
  imports = [
    ./a.nix
    ./modules/nhs-packages.nix
  ];
}
