{
  description = "Rust flake";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      flake-utils,
      fenix,
      ...
    }:
    let
      overlay =
        final: prev:
        let
          fenixPkgs = fenix.packages.${final.stdenv.hostPlatform.system};
        in
        {
          rustToolchain =
            with fenixPkgs;
            combine (
              with stable;
              [
                clippy
                rustc
                cargo
                rustfmt
                rust-src
              ]
            );
        };
    in
    {
      overlays.default = overlay;
    }
    // flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ overlay ];
        };
      in
      {
        packages = rec {
          kbdd = pkgs.rustPlatform.buildRustPackage {
            pname = "kbdd";
            version = "0.1.0";

            src = ./.;

            cargoLock.lockFile = ./Cargo.lock;

            enableParallelBuild = true;

            meta = {
              description = "Keyboard backlight daemon";
              homepage = "https://github.com/raung0/kbdd";
              license = pkgs.lib.licenses.gpl3;
              mainProgram = "kbdd";
            };
          };
          default = kbdd;
        };

        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            rustToolchain
            openssl
            pkg-config
            cargo-deny
            cargo-edit
            cargo-watch
            rust-analyzer
          ];

          shellHook = ''
            export RUST_SRC_PATH="${pkgs.rustToolchain}/lib/rustlib/src/rust/library"
          '';
        };
      }
    );
}
