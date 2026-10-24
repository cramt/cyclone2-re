// memory_libmap's M4K cells (synth/m4k.txt) -> CY2_M4K.
//
// Address buses are the block's 12 physical address pins: a port of width w
// uses the top 12 - log2-step(w) of them, libmap ties the rest to 0, which
// is how the hardware wants it (an address of n bits on pins 12 - n ..= 11).
// Data bit k is column k of the word. INIT is the block's contents in
// physical order, 4,608 bits: word a of a x9/x18/x36 port at INIT[a*w +: w],
// of a x1/x2/x4 port skipping every ninth bit (libmap's layout and the
// block's, README "Memory contents").

module \$__CY2_M4K_SP_ (PORT_A_CLK, PORT_A_ADDR, PORT_A_WR_EN, PORT_A_WR_DATA, PORT_A_RD_DATA);
  parameter INIT = 0;
  parameter WIDTH = 1;

  input PORT_A_CLK;
  input [11:0] PORT_A_ADDR;
  input PORT_A_WR_EN;
  input [WIDTH-1:0] PORT_A_WR_DATA;
  output [WIDTH-1:0] PORT_A_RD_DATA;

  wire [35:0] dout;
  wire [35:0] din = PORT_A_WR_DATA;
  assign PORT_A_RD_DATA = dout[WIDTH-1:0];

  CY2_M4K #(.MODE("SINGLE"), .WIDTH(WIDTH), .INIT(INIT)) _TECHMAP_REPLACE_ (
    .CLK(PORT_A_CLK), .A_ADDR(PORT_A_ADDR), .A_WE(PORT_A_WR_EN),
    .A_DATAIN(din), .B_ADDR(12'bx), .DATAOUT(dout));
endmodule

module \$__CY2_M4K_SDP_ (CLK_C, PORT_W_CLK, PORT_W_ADDR, PORT_W_WR_EN, PORT_W_WR_DATA, PORT_R_CLK, PORT_R_ADDR, PORT_R_RD_DATA);
  parameter INIT = 0;
  parameter WIDTH = 1;

  // The clock both ports share ("C" in m4k.txt); PORT_W_CLK and PORT_R_CLK
  // are the same net, or PORT_W_CLK a constant in a ROM.
  input CLK_C;
  input PORT_W_CLK;
  input [11:0] PORT_W_ADDR;
  input PORT_W_WR_EN;
  input [WIDTH-1:0] PORT_W_WR_DATA;
  input PORT_R_CLK;
  input [11:0] PORT_R_ADDR;
  output [WIDTH-1:0] PORT_R_RD_DATA;

  wire [35:0] dout;
  wire [35:0] din = PORT_W_WR_DATA;
  assign PORT_R_RD_DATA = dout[WIDTH-1:0];

  CY2_M4K #(.MODE("SIMPLE_DUAL"), .WIDTH(WIDTH), .INIT(INIT)) _TECHMAP_REPLACE_ (
    .CLK(CLK_C), .A_ADDR(PORT_W_ADDR), .A_WE(PORT_W_WR_EN),
    .A_DATAIN(din), .B_ADDR(PORT_R_ADDR), .DATAOUT(dout));
endmodule
