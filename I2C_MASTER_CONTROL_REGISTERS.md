I2CCR
Bits | Access | Reset | Description |
31:6 | - |  - | Reserved |
5:0 | RW | 13h | FD10. Frequency 10M Divisor and 0 are forbidden. 10M=Bus clock/(FD10+1). When powered on, software must write FD10 to let the I2C controller generate a 10MHz clock. | 

I2CMCR
Bits | Access | Reset | Description |
31 | RW | 0 | IMUR. I 2C Master Unit Reset
0: Normal
1: Reset the I2C Unit (only resets hardware FSM). This bit will self-clear to Zero after reset complete |
30 | RW | 0 | CS. Command Start
0: Stop. After completing a whole transaction, it returns to Zero
1: Start |
29:25 | RW | 0 | RWL. Read/Write Data Length for Related Commands
Does not include the slave address byte in the FIFO register.
When accessed, the controller will parse the byte following the last start (or Sr) byte to find the command type.
0: 1 byte ……… 17: 24 bytes |
24 | RW | 1 | TORE. Time-Out Register Enable
If TOR is required, the I2C rate must be constrained within 25kbps~400kbps.
This constraint is due to the time-out register bit. |
23:16 | RW | 3Ah | TOR. Time-Out Register
Time-out = TOR x 2 x ((FD10+1)/Bus clock) (For receive /transmit one bit).
If time-out occurs, it will trigger the Transaction Error Interrupt Flag.
Note: Time-out must > (1 SCL low period + repeat start setup time). |
15:11 | - | - | Reserved |
10 | RW | 0 | SBAIFD. Second Byte ACK in FRSIB Data
SBAIFD indicates whether the master checks for ACK from slave after emitting second data in FRSIB data.
0: Check 1: Do not check |
9 | RW | 0 | FBAIFD. First Byte ACK in FRSIB Data
FBAIFD indicates whether the master checks for ACK from slave after emitting first data in FRSIB data.
0: Check 1: Do not check |
8:7 | RW | 0 | SRSIB. Second Repeat Start Interval Byte
After transmitting SRSIB bytes following the first repeat start command, the master will produce a second repeat start command. The slave address or device address byte is included in this interval. Default interval is one byte. 0=1 byte; 1=2 bytes, etc.
Note: The eighth bit of slave address or device address byte followed by second repeat start command must be 1(means a Read CMD). |
6:5 | RW | 0 | FRSIB. First Repeat Start Interval Byte
After transmitting FRSIB bytes following the original start command, the master will produce the first repeat start command. The original slave address or device address byte is included in this interval. Default interval is one byte. 0=1 byte; 1=2 bytes, etc. |
4:3 | RW | 0 | RSC. Repeat Start Count 
00: No repeat start
01: One repeat start
10: Two repeat starts
11: Reserved |
2 | RW | 0 | TEIE. Transaction Error Interrupt Enable
1 | RW | 0 | MRCIE. Master Receive Complete Interrupt Enable
0 | RW | 0 | MTCIE. Master Transmit Complete Interrupt Enable


I2CMSR
Bits | Access | Reset | Description |
31:3 | - | - | Reserved |
2 | RW | 0 | TEIF. Transaction Error Interrupt Flag 
When a master transmit/receive fault or time-out occurs, the I 2C controller will lift the flag up and return the bus to idle. Write ‘1’ to clear. |
1 | RW | 0 | MRCIF. Master Receive Complete Interrupt Flag. Write ‘1’ to clear. |
0 | RW | 0 | MTCIF. Master Transmit Complete Interrupt Flag. Write ‘1’ to clear. |

I2CMFR
Bits | Access | Reset | Description |
31:8 | - | - | Reserved |
7:0 | RW | 0 | TDD. Target Device Data. Read for receive. |
