// 115200 8N1 transmitter on the DE2's RS-232 port: sends an incrementing
// byte (shown on LEDG) about ten times a second; SW[7:0] is XORed in.
module top(input CLOCK_50, input [17:0] SW, output UART_TXD, output [8:0] LEDG);
  localparam DIV = 434; // 50 MHz / 115200
  reg [8:0] baud = 0;
  reg [3:0] bitn = 0;
  reg [9:0] shift = 10'h3ff;
  reg [22:0] gap = 0;
  reg [7:0] data = 0;
  wire busy = bitn != 0;
  always @(posedge CLOCK_50) begin
    gap <= gap + 1'b1;
    if (!busy) begin
      baud <= 0;
      if (gap == 0) begin
        shift <= {1'b1, data ^ SW[7:0], 1'b0};
        bitn <= 4'd10;
        data <= data + 1'b1;
      end
    end else if (baud == DIV - 1) begin
      baud <= 0;
      shift <= {1'b1, shift[9:1]};
      bitn <= bitn - 1'b1;
    end else
      baud <= baud + 1'b1;
  end
  assign UART_TXD = shift[0];
  assign LEDG = {busy, data};
endmodule
