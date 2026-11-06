{
  description = "httplib development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
      {
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            rustc
            cargo
            rustfmt
            clippy

            cmake
            clang
            gcc
            perl
            pkg-config

            git
            openssl
          ];

          env = {
            LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
          };

          shellHook = ''
            echo "Rust development environment"
            rustc --version
            cargo --version
            cmake --version
          '';
        };
      });
}
