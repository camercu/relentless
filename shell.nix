let
  pinned_nixpkgs = builtins.fetchTarball {
    url = "https://github.com/NixOS/nixpkgs/archive/ed142ab1b3a092c4d149245d0c4126a5d7ea00b0.tar.gz";
  };
  pkgs = import pinned_nixpkgs {};
in
pkgs.mkShell {
  packages = with pkgs; [
    rustup
    just
    cargo-deny
    typos
    taplo
  ];
}
