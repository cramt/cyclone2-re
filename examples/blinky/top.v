// DE2 blinky: a 26-bit counter on CLOCK_50; the top four bits drive
// LEDR[3:0] (about one toggle per second on LEDR[0]). Same design as
// chipsmith's example-de2.
module top(input CLOCK_50, output [3:0] LEDR);
  reg [25:0] counter = 0;
  always @(posedge CLOCK_50) counter <= counter + 1'b1;
  assign LEDR = {counter[22], counter[23], counter[24], counter[25]};
endmodule
