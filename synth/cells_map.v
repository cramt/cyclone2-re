// Generic yosys cells -> Cyclone II LEs.

module \$_DFF_P_ (input D, input C, output Q);
  LCELL_FF _TECHMAP_REPLACE_ (.DATAIN(D), .CLK(C), .REGOUT(Q));
endmodule

// LUTs narrower than 4 repeat their table over the unused inputs, so the
// value those inputs read doesn't matter.
module \$lut (A, Y);
  parameter WIDTH = 0;
  parameter LUT = 0;
  input [WIDTH-1:0] A;
  output Y;

  function [15:0] expand(input integer w, input [15:0] lut);
    integer i;
    begin
      for (i = 0; i < 16; i = i + 1)
        expand[i] = lut[i % (1 << w)];
    end
  endfunction

  localparam [15:0] MASK = expand(WIDTH, LUT);
  generate
    if (WIDTH == 1)
      LCELL_COMB #(.LUT_MASK(MASK)) _TECHMAP_REPLACE_ (.DATAA(A[0]), .COMBOUT(Y));
    else if (WIDTH == 2)
      LCELL_COMB #(.LUT_MASK(MASK)) _TECHMAP_REPLACE_ (.DATAA(A[0]), .DATAB(A[1]), .COMBOUT(Y));
    else if (WIDTH == 3)
      LCELL_COMB #(.LUT_MASK(MASK)) _TECHMAP_REPLACE_ (.DATAA(A[0]), .DATAB(A[1]), .DATAC(A[2]), .COMBOUT(Y));
    else if (WIDTH == 4)
      LCELL_COMB #(.LUT_MASK(MASK)) _TECHMAP_REPLACE_ (.DATAA(A[0]), .DATAB(A[1]), .DATAC(A[2]), .DATAD(A[3]), .COMBOUT(Y));
    else
      wire _TECHMAP_FAIL_ = 1;
  endgenerate
endmodule
