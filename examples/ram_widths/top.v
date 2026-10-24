// The M4K widths examples/ram doesn't use, each as a single port and as a
// simple dual port, some with initial contents: x2, x4, x9, x18 single port;
// x1, x2, x4, x9, x16, x18 simple dual port. Written from a counter and the
// switches while SW[17] is up; read data folded onto the LEDs.
module top(input CLOCK_50, input [17:0] SW, input [3:0] KEY, output [17:0] LEDR, output [8:0] LEDG);
  wire clk = CLOCK_50;
  reg [31:0] cnt = 0;
  always @(posedge clk) cnt <= cnt + 1'b1;

  wire we = SW[17] & cnt[2];
  wire [17:0] d = cnt[31:14] ^ SW;
  wire [11:0] wa = cnt[31:20];
  wire [11:0] ra = cnt[30:19] ^ SW[16:5];

  // Single ports (read-during-write: new data, a registered address).
  reg [1:0] s2 [0:2047];
  reg [3:0] s4 [0:1023];
  reg [8:0] s9 [0:511];
  reg [17:0] s18 [0:255];
  reg [11:0] s2_a;
  reg [9:0] s4_a;
  reg [8:0] s9_a;
  reg [7:0] s18_a;
  always @(posedge clk) begin
    if (we) begin
      s2[wa[10:0]] <= d[1:0];
      s4[wa[9:0]] <= d[3:0];
      s9[wa[8:0]] <= d[8:0];
      s18[wa[7:0]] <= d;
    end
    s2_a <= wa[10:0];
    s4_a <= wa[9:0];
    s9_a <= wa[8:0];
    s18_a <= wa[7:0];
  end

  // Simple dual ports (read-during-write: old data).
  reg d1 [0:4095];
  reg [1:0] d2 [0:2047];
  reg [3:0] d4 [0:1023];
  reg [8:0] d9 [0:511];
  reg [15:0] d16 [0:255];
  reg [17:0] d18 [0:255];
  reg q1;
  reg [1:0] q2;
  reg [3:0] q4;
  reg [8:0] q9;
  reg [15:0] q16;
  reg [17:0] q18;
  always @(posedge clk) begin
    if (we) begin
      d1[wa] <= d[0];
      d2[wa[10:0]] <= d[1:0];
      d4[wa[9:0]] <= d[3:0];
      d9[wa[8:0]] <= d[8:0];
      d16[wa[7:0]] <= d[15:0];
      d18[wa[7:0]] <= d;
    end
    q1 <= d1[ra];
    q2 <= d2[ra[10:0]];
    q4 <= d4[ra[9:0]];
    q9 <= d9[ra[8:0]];
    q16 <= d16[ra[7:0]];
    q18 <= d18[ra[7:0]];
  end

  // Initial contents for some: a hash of the address.
  integer i;
  initial begin
    for (i = 0; i < 2048; i = i + 1) s2[i] = (i * 32'h9E3779B1) >> 30;
    for (i = 0; i < 512; i = i + 1) s9[i] = (i * 32'h85EBCA6B) >> 23;
    for (i = 0; i < 1024; i = i + 1) d4[i] = (i * 32'hC2B2AE35) >> 28;
    for (i = 0; i < 256; i = i + 1) d18[i] = (i * 32'h27D4EB2F) >> 14;
  end

  wire [107:0] all = {s2[s2_a], s4[s4_a], s9[s9_a], s18[s18_a], q1, q2, q4, q9, q16, q18};
  genvar k;
  generate
    for (k = 0; k < 27; k = k + 1) begin : fold
      wire [3:0] g = {all[k + 81], all[k + 54], all[k + 27], all[k]};
      if (k < 18) assign LEDR[k] = ^g;
      else assign LEDG[k - 18] = ^g;
    end
  endgenerate
endmodule
