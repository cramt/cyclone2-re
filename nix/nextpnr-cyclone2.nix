# nextpnr-himbaechel with the out-of-tree Cyclone II uarch (nextpnr/cyclone2).
#
# Source pin: whatever nextpnr nixpkgs ships (0.11.1 at the flake.lock rev),
# because the uarch only uses the himbaechel API and building against the
# packaged release keeps it reproducible. Remove this derivation if the uarch
# is ever upstreamed into nextpnr (https://github.com/YosysHQ/nextpnr).
#
# Only himbaechel/cyclone2 is built, with its chip database generated from the
# repo's committed db/ the way an upstream build would (HIMBAECHEL_CYCLONE2_DB;
# ~10 min, 2.6 GB RAM).
{
  stdenv,
  nextpnr,
  cmake,
  python3,
  boost,
  eigen,
}:

let
  boostPython = boost.override {
    python = python3;
    enablePython = true;
  };
in
stdenv.mkDerivation {
  pname = "nextpnr-cyclone2";
  inherit (nextpnr) version src;

  nativeBuildInputs = [
    cmake
    python3
  ];
  buildInputs = [
    boostPython
    eigen
  ];

  postPatch = ''
    cp -r ${../nextpnr/cyclone2} himbaechel/uarch/cyclone2
    chmod -R u+w himbaechel/uarch/cyclone2
    substituteInPlace himbaechel/CMakeLists.txt \
      --replace-fail "set(HIMBAECHEL_UARCHES example gowin xilinx ng-ultra gatemate)" \
                     "set(HIMBAECHEL_UARCHES example gowin xilinx ng-ultra gatemate cyclone2)"
  '';

  cmakeFlags = [
    "-DCURRENT_GIT_VERSION=${nextpnr.version}-cyclone2"
    "-DARCH=himbaechel"
    "-DHIMBAECHEL_UARCH=cyclone2"
    "-DBUILD_TESTS=OFF"
    "-DBUILD_PYTHON=OFF"
    "-DUSE_OPENMP=ON"
    "-DHIMBAECHEL_CYCLONE2_DB=${../db}"
  ];

  meta.mainProgram = "nextpnr-himbaechel";
}
