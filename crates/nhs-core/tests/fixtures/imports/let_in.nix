{ pkgs, ... }:
let
  helper = 1;
in
{
  imports = [
    ./a.nix
  ];
}
