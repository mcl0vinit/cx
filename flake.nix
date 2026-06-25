{
  description = "Codex account/session router";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      lib = nixpkgs.lib;
      manifest = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      rustSource = lib.fileset.toSource {
        root = ./.;
        fileset = lib.fileset.unions [
          ./Cargo.toml
          ./Cargo.lock
          ./src
        ];
      };
      systems = [
        "aarch64-darwin"
        "x86_64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ];
      forAllSystems =
        f:
        lib.genAttrs systems (
          system:
          f {
            inherit system;
            pkgs = import nixpkgs { inherit system; };
          }
        );
    in
    {
      packages = forAllSystems (
        { pkgs, ... }:
        {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = manifest.package.name;
            version = manifest.package.version;
            src = rustSource;
            cargoLock.lockFile = ./Cargo.lock;

            buildInputs = lib.optionals pkgs.stdenv.isDarwin [
              pkgs.libiconv
            ];

            meta = {
              description = manifest.package.description;
              license = lib.licenses.mit;
              mainProgram = manifest.package.name;
            };
          };
        }
      );

      apps = forAllSystems (
        { system, ... }:
        {
          default = {
            type = "app";
            program = "${self.packages.${system}.default}/bin/${manifest.package.name}";
            meta.description = manifest.package.description;
          };
        }
      );

      checks = forAllSystems (
        { pkgs, system }:
        {
          default = self.packages.${system}.default;
          fmt =
            pkgs.runCommand "${manifest.package.name}-rustfmt-check" { nativeBuildInputs = [ pkgs.rustfmt ]; }
              ''
                cp -R ${rustSource} source
                chmod -R u+w source
                cd source
                rustfmt --check src/*.rs
                touch $out
              '';
        }
      );

      devShells = forAllSystems (
        { pkgs, ... }:
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              clippy
              rustc
              rustfmt
            ];

            RUST_BACKTRACE = "1";
          };
        }
      );

      formatter = forAllSystems (
        { pkgs, ... }:
        pkgs.writeShellApplication {
          name = "cx-fmt";
          runtimeInputs = [
            pkgs.nixfmt
            pkgs.rustfmt
          ];
          text = ''
            if [ "$#" -eq 0 ]; then
              nixfmt flake.nix
              rustfmt src/*.rs
              exit 0
            fi

            for file in "$@"; do
              case "$file" in
                *.nix) nixfmt "$file" ;;
                *.rs) rustfmt "$file" ;;
              esac
            done
          '';
        }
      );
    };
}
