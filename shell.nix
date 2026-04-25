# Tool versions should match .tool-versions — run `just check-tool-versions` to verify
let
  pinned_nixpkgs = builtins.fetchTarball {
    url = "https://github.com/NixOS/nixpkgs/archive/566acc07c54dc807f91625bb286cb9b321b5f42a.tar.gz";
    sha256 = "19mppaiq05h4xrpch4i0jkkca4nnfdksc2fkhssplawggsj57id6";
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
    cargo-llvm-cov
    typos
    taplo
    nodejs
  ];
}
