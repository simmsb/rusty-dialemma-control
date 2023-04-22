{
  description = "things";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    parts.url = "github:hercules-ci/flake-parts";
    parts.inputs.nixpkgs-lib.follows = "nixpkgs";
  };

  outputs =
    inputs@{ self
    , nixpkgs
    , crane
    , fenix
    , parts
    , ...
    }:
    parts.lib.mkFlake { inherit inputs; } {
      systems = nixpkgs.lib.systems.flakeExposed;
      imports = [
      ];
      perSystem = { config, pkgs, system, lib, ... }:
        let
          toolchain = fenix.packages.${system}.complete.withComponents [
            "cargo"
            "clippy"
            "rust-src"
            "rustc"
            "rustfmt"
          ];
          craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
          package = { path, args ? "", profile ? "release" }: craneLib.buildPackage {
            cargoExtraArgs = args;
            CARGO_PROFILE = profile;
            src = craneLib.cleanCargoSource (craneLib.path path);
            doCheck = false;
            buildInputs = [
              # Add additional build inputs here
            ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
              # Additional darwin specific inputs can be set here
              pkgs.libiconv
              pkgs.darwin.apple_sdk.frameworks.IOKit
            ];
          };
          control = args: package (args // { path = ./.; });
        in
        {
          devShells.default = pkgs.mkShell {
            inputsFrom = [ (control { profile = "dev"; }) ];
            nativeBuildInputs = with pkgs; [ fenix.packages.${system}.rust-analyzer cargo-binutils picotool ];
          };
          packages.default = control { profile = "release"; };
        };
    };
}
