# Tool versions should match .tool-versions — run `just check-tool-versions` to verify
let
  pinned_nixpkgs = builtins.fetchTarball {
    url = "https://github.com/NixOS/nixpkgs/archive/ed142ab1b3a092c4d149245d0c4126a5d7ea00b0.tar.gz";
    sha256 = "1h7v295lpjfxpxkag2csam7whx918sdypixdi8i85vlb707gg0vm";
  };
  pkgs = import pinned_nixpkgs {};
in
pkgs.mkShell {
  packages = with pkgs; [
    rustup
    just
    pre-commit
    cargo-deny
    cargo-nextest
    cargo-semver-checks
    cargo-mutants
    typos
    taplo
    nodejs
  ];
}
