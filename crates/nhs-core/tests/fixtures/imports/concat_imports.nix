{ pkgs, ... }:
{
  imports = [ ./a.nix ] ++ [ ./b.nix ];
}
