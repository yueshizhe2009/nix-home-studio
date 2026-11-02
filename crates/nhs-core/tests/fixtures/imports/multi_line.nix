{ pkgs, ... }:
{
  imports = [
    ./a.nix
    ./b.nix
  ];

  home.stateVersion = "24.05";
}
