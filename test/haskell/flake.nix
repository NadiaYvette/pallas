{
  description = "Pallas Trace-Forwarding Haskell Test Infrastructure";

  nixConfig = {
    extra-substituters = ["https://cache.iog.io"];
    extra-trusted-public-keys = ["hydra.iohk.io:f/Ea+s+dFdN+3Y/G+FDgSq+a5NEWhJGzdjvKNGv0/EQ="];
  };

  inputs = {
    CHaP = {
      url = "github:intersectmbo/cardano-haskell-packages?ref=repo";
      flake = false;
    };

    hackageNix = {
      url = "github:input-output-hk/hackage.nix";
      flake = false;
    };

    haskellNix = {
      url = "github:input-output-hk/haskell.nix";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.hackage.follows = "hackageNix";
    };

    iohkNix = {
      url = "github:input-output-hk/iohk-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    nixpkgs.follows = "haskellNix/nixpkgs-unstable";

    utils.url = "github:numtide/flake-utils";
  };

  outputs = {
    CHaP,
    haskellNix,
    iohkNix,
    nixpkgs,
    self,
    utils,
    ...
  }: let
    supportedSystems = [ "x86_64-linux" "x86_64-darwin" "aarch64-darwin" ];
  in
    utils.lib.eachSystem supportedSystems (system: let
      pkgs = import nixpkgs {
        inherit system;
        overlays = [
          iohkNix.overlays.crypto
          haskellNix.overlay
          iohkNix.overlays.haskell-nix-extra
          iohkNix.overlays.haskell-nix-crypto
        ];
      };

      project = pkgs.haskell-nix.cabalProject' {
        src = ./.;
        name = "cardano-tracer-tests";
        compiler-nix-name = "ghc966";
        cabalProjectLocal = ''
          repository cardano-haskell-packages-local
            url: file:${CHaP}
            secure: True
          active-repositories: hackage.haskell.org, cardano-haskell-packages-local
          allow-newer: terminfo:base
        '';
        inputMap = {
          "https://chap.intersectmbo.org/" = CHaP;
        };
        shell = {
          name = "pallas-trace-test-shell";
          nativeBuildInputs = with pkgs.pkgsBuildBuild; [
            pkg-config
          ];
          withHoogle = false;
        };
        modules = [
          ({ lib, pkgs, ... }: {
            # Register crypto packages so we can configure their pkgconfig
            package-keys = ["cardano-crypto-praos" "cardano-crypto-class"];
            # Use the VRF fork of libsodium
            packages.cardano-crypto-praos.components.library.pkgconfig = lib.mkForce [ [ pkgs.libsodium-vrf ] ];
            packages.cardano-crypto-class.components.library.pkgconfig = lib.mkForce [ [ pkgs.libsodium-vrf pkgs.secp256k1 pkgs.libblst ] ];
          })
        ];
      };

    in {
      devShells.default = project.shell;

      packages = project.flake'.packages or {};

      checks = project.flake'.checks or {};
    });
}
