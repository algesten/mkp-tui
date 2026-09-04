{
  description = "Make Play — terminal client for the Make Play music server";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        # No version is committed to this repo — the release tag in the
        # private mkp repo is the single source of truth, and each
        # packaging recipe passes it in through MKP_VERSION.
        #
        # Nix flakes do not expose the tag a `github:owner/repo/1.0.0`
        # reference was resolved from, only the commit it points at, so
        # a build here identifies itself by revision. That is exact and
        # traceable, if less pretty than "1.0.0".
        version = self.shortRev or self.dirtyShortRev or "dev";
      in
      {
        packages = rec {
          default = mkp;

          mkp = pkgs.rustPlatform.buildRustPackage {
            pname = "mkp";
            inherit version;

            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            cargoBuildFlags = [ "-p" "mkpclient-tui" ];
            cargoTestFlags = [ "-p" "mkpclient-tui" ];

            # rustls pulls aws-lc-rs, which compiles C.
            nativeBuildInputs = [ pkgs.cmake ];

            # Read by client/tui/build.rs. The sandbox has no .git, so
            # its `git describe` fallback cannot fire here.
            MKP_VERSION = version;

            meta = with pkgs.lib; {
              description = "Terminal client for the Make Play music server";
              homepage = "https://github.com/algesten/mkp-tui";
              license = with licenses; [ mit asl20 ];
              mainProgram = "mkp";
              platforms = platforms.unix;
            };
          };
        };

        devShells.default = pkgs.mkShell {
          nativeBuildInputs = [
            pkgs.cargo
            pkgs.rustc
            pkgs.rust-analyzer
            pkgs.clippy
            pkgs.rustfmt
            pkgs.cmake
          ];
        };
      }
    );
}
