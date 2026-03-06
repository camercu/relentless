{ pkgs ? import <nixpkgs> {} }:
pkgs.mkShell {
  packages = with pkgs; [
    rustup
    just
    uv
    cargo-deny
    cargo-nextest
    cargo-hack
    typos
    taplo
  ];
}
