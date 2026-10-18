// Cyclone II primitives as the open flow models them (nextpnr/cyclone2).
// Behaviour follows Quartus' cycloneii_lcell_comb/_ff: COMBOUT is a LUT4 over
// (DATAA, DATAB, C, DATAD) with C = CIN when CIN is connected, else DATAC;
// COUT is the same LUT with C = CIN and DATAD = 0. Unconnected LUT inputs
// read 1, as Quartus ties them to vcc.

(* blackbox *)
module LCELL_COMB (input DATAA, input DATAB, input DATAC, input DATAD, input CIN,
                   output COMBOUT, output COUT);
  parameter [15:0] LUT_MASK = 16'hFFFF;
endmodule

(* blackbox *)
module LCELL_FF (input DATAIN, input CLK, output REGOUT);
endmodule

(* blackbox *)
module CY2_IBUF (input PAD, output COMBOUT);
endmodule

(* blackbox *)
module CY2_OBUF (input DATAOUT, output PAD);
endmodule
