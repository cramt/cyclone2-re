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
      in
      {
        packages = {
          inherit nextpnr-cyclone2;
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
            # throwaway exploration scripts in tools/, and nextpnr's chipdb
            # generator
            python3
          ];
          # `just chipdb` runs the generator and bbasm this nextpnr ships.
          NEXTPNR_CYCLONE2_SHARE = "${nextpnr-cyclone2}/share/nextpnr/cyclone2";
        };
      }
    );
}
