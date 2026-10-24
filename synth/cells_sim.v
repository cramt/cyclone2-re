// Cyclone II primitives as the open flow models them (nextpnr/cyclone2).
// Behaviour follows Quartus' cycloneii_lcell_comb/_ff: COMBOUT is a LUT4 over
// (DATAA, DATAB, C, DATAD) with C = CIN when CIN is connected, else DATAC;
// COUT is the same LUT with C = CIN and DATAD = 0. Unconnected LUT inputs
// read 1, as Quartus ties them to vcc; the maps tie unused inputs to 1.

(* blackbox *)
module LCELL_COMB (input DATAA, input DATAB, input DATAC, input DATAD, input CIN,
                   output COMBOUT, output COUT);
  parameter [15:0] LUT_MASK = 16'hFFFF;
endmodule

// ENA gates SCLR (only an enabled clock edge clears); ACLR is asynchronous.
// Unconnected (the maps tie them so): ENA 1, ACLR and SCLR 0.
(* blackbox *)
module LCELL_FF (input DATAIN, input CLK, input ENA, input ACLR, input SCLR, output REGOUT);
endmodule

(* blackbox *)
module CY2_IBUF (input PAD, output COMBOUT);
endmodule

(* blackbox *)
module CY2_OBUF (input DATAOUT, output PAD);
endmodule

// Tri-state output or bidir: PAD = OE ? DATAOUT : 1'bz, COMBOUT = PAD.
(* blackbox *)
module CY2_IOBUF (input DATAOUT, input OE, output COMBOUT, inout PAD);
endmodule

// M4K RAM block (synth/m4k_map.v builds it from memory_libmap's cells).
// MODE "SINGLE": port A reads and writes at A_ADDR (A_WE never driven: a
// ROM). MODE "SIMPLE_DUAL": port A writes at A_ADDR, port B reads at B_ADDR.
// Both on the rising edge of CLK; DATAOUT is the word read, captured by the
// clock (a synchronous read, no output register). WIDTH: 1, 2, 4, 9, 18 or
// 36 (36 only in simple dual port); an n-bit address uses pins
// 12 - n ..= 11. INIT: the 4,608 content bits in physical order.
(* blackbox *)
module CY2_M4K (input CLK, input [11:0] A_ADDR, input [35:0] A_DATAIN, input A_WE, input [11:0] B_ADDR,
                output [35:0] DATAOUT);
  parameter MODE = "SINGLE";
  parameter WIDTH = 1;
  parameter [4607:0] INIT = 4608'b0;
endmodule
