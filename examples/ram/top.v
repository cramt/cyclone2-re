// M4K RAM blocks in the open flow: one memory of each supported shape, all
// on CLOCK_50, written from a free-running counter and the switches while
// SW[17] is up, their read data folded onto the LEDs. Each fills its block
// (synth/m4k.txt), so the address uses every pin of its width.
//
//   ram1   4096 x 1   single port (read-during-write: new data)
//   ram8   512 x 8    simple dual port
//   ram16  256 x 16   single port
//   ram32  128 x 32   simple dual port
//   ram36  128 x 36   simple dual port
//   rom8   512 x 8    ROM, initial contents
//   rom36  128 x 36   ROM, initial contents (Quartus builds it on port B)
module top(input CLOCK_50, input [17:0] SW, input [3:0] KEY, output [17:0] LEDR, output [8:0] LEDG);
  wire clk = CLOCK_50;
  reg [31:0] cnt = 0;
  always @(posedge clk) cnt <= cnt + 1'b1;

  wire we = SW[17] & cnt[3];
  // Write data: counter bits mixed with the switches.
  wire [35:0] d = {cnt[31:14], cnt[17:0]} ^ {SW, SW};
  // Read addresses that differ from the write address.
  wire [11:0] ra = cnt[31:20] ^ SW[11:0];

  reg ram1 [0:4095];
  reg [11:0] ram1_a;
  always @(posedge clk) begin
    if (we) ram1[cnt[31:20]] <= d[0];
    ram1_a <= cnt[31:20];
  end
  wire q1 = ram1[ram1_a];

  reg [7:0] ram8 [0:511];
  reg [7:0] q8;
  always @(posedge clk) begin
    if (we) ram8[cnt[28:20]] <= d[7:0];
    q8 <= ram8[ra[8:0]];
  end

  reg [15:0] ram16 [0:255];
  reg [7:0] ram16_a;
  always @(posedge clk) begin
    if (we) ram16[cnt[27:20]] <= d[15:0];
    ram16_a <= cnt[27:20];
  end
  wire [15:0] q16 = ram16[ram16_a];

  reg [31:0] ram32 [0:127];
  reg [31:0] q32;
  always @(posedge clk) begin
    if (we) ram32[cnt[26:20]] <= d[31:0];
    q32 <= ram32[ra[6:0]];
  end

  reg [35:0] ram36 [0:127];
  reg [35:0] q36;
  always @(posedge clk) begin
    if (we) ram36[cnt[26:20]] <= d;
    q36 <= ram36[ra[11:5]];
  end

  // ROM contents: a multiplicative hash of the address, distinct per word.
  reg [7:0] rom8 [0:511];
  reg [35:0] rom36 [0:127];
  integer i;
  initial begin
    for (i = 0; i < 512; i = i + 1) rom8[i] = (i * 32'h9E3779B1) >> 7;
    for (i = 0; i < 128; i = i + 1) rom36[i] = {i[6:0], 29'h0} ^ ((i * 64'h9E3779B97F4A7C15) >> 11);
  end
  reg [7:0] r8;
  reg [35:0] r36;
  always @(posedge clk) begin
    r8 <= rom8[cnt[31:23] ^ SW[8:0]];
    r36 <= rom36[cnt[31:25] ^ SW[16:10]];
  end

  // Every read bit reaches a pin.
  wire [148:0] all = {q1, q8, q16, q32, q36, r8, r36};
  genvar k;
  generate
    for (k = 0; k < 27; k = k + 1) begin : fold
      wire [5:0] g;
      assign g = {all[(k + 135) % 149], all[(k + 108) % 149], all[k + 81], all[k + 54], all[k + 27], all[k]};
      if (k < 18) assign LEDR[k] = ^g;
      else assign LEDG[k - 18] = ^g;
    end
  endgenerate
endmodule
