// 32-bit Fibonacci LFSR (taps 32, 22, 2, 1) in XNOR form, so all-zeros is
// a valid state and the registers can power up at 0 (an init of 1 would
// cost an inverter in front of the next stage of the shift). KEY[0] (active
// low) loads SW[17:0] as the seed, SW[17] also gates the shift; LEDR/LEDG
// show the state.
module top(input CLOCK_50, input [17:0] SW, input [3:0] KEY, output [17:0] LEDR, output [8:0] LEDG);
  reg [31:0] lfsr = 0;
  reg [21:0] slow = 0;
  wire tick = slow == 0;
  always @(posedge CLOCK_50) begin
    slow <= slow + 1'b1;
    if (!KEY[0])
      lfsr <= {SW, 14'h0};
    else if (tick && SW[17])
      lfsr <= {lfsr[30:0], ~(lfsr[31] ^ lfsr[21] ^ lfsr[1] ^ lfsr[0])};
  end
  assign LEDR = lfsr[31:14];
  assign LEDG = lfsr[8:0];
endmodule
