# The yosys techmap for Cyclone II LEs. synth/cyclone2.ys names its maps
# relative to the directory holding synth/, so yosys runs with $out as its
# working directory.
{ runCommand }:

runCommand "cyclone2-synth" { } ''
  mkdir -p $out
  cp -r ${../synth} $out/synth
''
