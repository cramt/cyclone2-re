// Tri-state and bidirectional pins on the DE2: five expansion-header pins
// (GPIO_0[4:0], row IOEs on the right edge) and four SRAM data pins
// (SRAM_DQ[3:0], column IOEs on the bottom edge). Each is driven with its
// output enable from a switch, an inverted switch, logic or a register, or
// open drain, and read back onto the red LEDs; LEDG[0] is an output-only
// tri-state pin.
// Nothing here is meant to run on the board with the SRAM fitted (it would
// fight the SRAM's own drivers when SW[4] is up).
module top(input CLOCK_50, input [17:0] SW, input [3:0] KEY,
           inout [4:0] GPIO_0, inout [3:0] SRAM_DQ,
           output [8:0] LEDR, output [1:0] LEDG);
  reg [25:0] c = 0;
  always @(posedge CLOCK_50) c <= c + 1'b1;
  reg oe_q = 0;
  always @(posedge CLOCK_50) oe_q <= SW[3];

  assign GPIO_0[0] = SW[0] ? c[25] : 1'bz;
  assign GPIO_0[1] = ~SW[1] ? c[24] : 1'bz;
  assign GPIO_0[2] = (SW[2] & KEY[0]) ? ~c[23] : 1'bz;
  assign GPIO_0[3] = oe_q ? c[22] : 1'bz;
  assign SRAM_DQ = SW[4] ? SW[11:8] : 4'bz;
  // Open drain: pulls low or lets go.
  assign GPIO_0[4] = c[20] ? 1'bz : 1'b0;

  assign LEDR = {SRAM_DQ, GPIO_0};
  assign LEDG[0] = SW[5] ? c[21] : 1'bz;
  assign LEDG[1] = GPIO_0[0] ^ SRAM_DQ[0] ^ GPIO_0[4];
endmodule
