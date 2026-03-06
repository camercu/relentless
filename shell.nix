{ pkgs ? import <nixpkgs> {} }:
pkgs.mkShell {
  packages = with pkgs; [
    rustup
    just
    cargo-deny
    cargo-nextest
    cargo-hack
    typos
    taplo
  ];
}
