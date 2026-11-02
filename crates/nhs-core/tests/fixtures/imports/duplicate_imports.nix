{ pkgs, ... }:
{
  imports = [ ./a.nix ];
  imports = [ ./b.nix ];
}
