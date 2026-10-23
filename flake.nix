{
  description = "cyclone2-re — open bitstream database and toolchain for Altera Cyclone II";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    { nixpkgs, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        nextpnr-cyclone2 = pkgs.callPackage ./nix/nextpnr-cyclone2.nix { };
        cram = pkgs.callPackage ./nix/cram.nix { };
        cyclone2-synth = pkgs.callPackage ./nix/synth.nix { };
        # v1.1.1 only knows the EP2C5, so it can't find the DE2's EP2C35 on
        # the JTAG chain. Cyclone II IDCODEs landed upstream in
        # https://github.com/trabucayre/openFPGALoader/pull/697. Drop this
        # once nixpkgs ships a release newer than 1.1.1.
        openfpgaloader = pkgs.openfpgaloader.overrideAttrs (old: {
          patches = (old.patches or [ ]) ++ [
            (pkgs.fetchpatch {
              name = "cyclone-ii-idcodes.patch";
              url = "https://github.com/trabucayre/openFPGALoader/commit/5d77b4acb657.patch";
              hash = "sha256-L58MTDF72kL1wyvTesQxebof+mJKKC8jldtIkeDRVZA=";
            })
          ];
        });
      in
      {
        packages = {
          inherit
            nextpnr-cyclone2
            openfpgaloader
            cram
            cyclone2-synth
            ;
        };

        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            just

            # the open flow this project feeds
            yosys
            nextpnr-cyclone2
            openfpgaloader

            # the oracle is a 2013 binary, so tools/quartus.sh runs it in an
            # empty network namespace
            util-linux
            # throwaway exploration scripts in tools/
            python3
          ];
        };
      }
    );
}
