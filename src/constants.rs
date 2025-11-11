// src/pub constants.rs
// Note the pub keyword, which makes the constants public and accessible from other modules.

/// Hard-coded R82xx tuner register values for 1090 MHz (to be filled in by you).
///
/// This is intentionally left with placeholder values; you can populate it by:
///   1) Tuning the dongle to 1090 MHz with a known-good tool (rtl_sdr/rtl_test).
///   2) Running this Rust program (with only GetTunReg) to dump tuner regs.
///   3) Copying the relevant (reg, value) pairs here.
///
/// For now, we leave it empty or with dummy entries so the code compiles.
pub const R82XX_1090MHZ_PROFILE: &[(u8, u8)] = &[
    // (reg, value) pairs go here, e.g.:
    // (0x05, 0x??),
    // (0x06, 0x??),
    // ...
];


pub const RTL_VID: u16 = 0x0bda;
// Common Realtek DVB-T dongle PIDs used by RTL-SDR sticks.
// Adjust/add if your lsusb shows something else.
pub const RTL_PIDS: &[u16] = &[
    0x2832, // RTL2832U (common "RTL2832U" DVB-T)
];

// ADS-B center frequency (Hz)
pub const ADSB_CENTER_FREQ_HZ: u32 = 1_090_000_000; // 1090 MHz

pub const TUNER_I2C_ADDR: u8 = 0x34; // R820T/R828D base (write) address

// enable demodulate internal test mode (8-bit counter stream)
pub const TEST_MODE: bool = false;

// Vendor-specific control transfer flags for RTL2832U
// bmRequestType values from RTL2832U datasheet vendor command table.
// 0xC0 = IN, Vendor, Device  |  0x40 = OUT, Vendor, Device
pub const CTRL_IN: u8 = 0xC0;
pub const CTRL_OUT: u8 = 0x40;

// Control transfer timeout (milliseconds)
pub const CTRL_TIMEOUT_MS: u64 = 1000;

// USB register address we’ll test: USB_SYSCTL byte 0 at 0x2000
// see RTL2832U datasheet
pub const USB_SYSCTL_0: u16 = 0x2000;
// USB and SYS register addresses (same values you found)
pub const USB_SYSCTL:      u16 = 0x2000;
pub const USB_EPA_CTL:     u16 = 0x2148;
pub const USB_EPA_MAXPKT:  u16 = 0x2158;

// ---------- System (SYSB) I2C Master registers ----------
pub const SYS_I2CCR:  u16 = 0x3040; // I2C Clock Register (I2CCR)
// ---- SYS I2C Clock Register (I2CCR, Table 22) ----
// Address: 0x3040 (SYS_I2CCR)
// Bits [5:0] = FD10 (Frequency-10M divisor)
// Reset: 0x13 (per datasheet), FD10 = 0 is forbidden.
pub const SYS_I2CCR_FD10_MASK: u8 = 0x3F;
pub const SYS_I2CCR_FD10_10MHZ: u8 = 0x13; // recommended value

pub const SYS_I2CMCR: u16 = 0x3044; // I2C Master Control (I2CMCR) – not used yet
pub const SYS_I2CMSTR: u16 = 0x3048; // I2C Master SCL Timing (I2CMSTR) – not used yet
pub const SYS_I2CMSR: u16 = 0x304C; // I2C Master Status (I2CMSR)
pub const SYS_I2CMFR: u16 = 0x3050; // I2C Master FIFO (I2CMFR)



// Block number for tuner access via vendor command table (“TUNB” in the datasheet)
pub const BLOCK_TUNB: u8 = 3;

// Demod "pages" and a known-safe test register.
// These values are taken from the RTL2832U datasheet
/// Demodulator page index for general demod registers (per datasheet)
pub const DEMOD_PAGE_0: u8 = 0;
pub const DEMOD_PAGE_1: u8 = 1;
pub const DEMOD_PAGE_SYS: u8 = 0x0a;
pub const DEMOD_REG_SYS_TEST: u16 = 0x0001;
pub const DEMOD_CTL:       u16 = 0x3000;
pub const DEMOD_CTL_1:     u16 = 0x300b;

// RTL2832 "blocks" (same as enum blocks in librtlsdr)
pub const BLOCK_DEMODB: u8 = 0;
pub const BLOCK_USBB:  u8 = 1;
pub const BLOCK_SYSB:  u8 = 2;
// (others exist but we don't need them yet)

// FIR_LEN = 16
pub const FIR_DEFAULT: [i16; 16] = [
    -54, -36, -41, -40, -32, -14, 14, 53,     // 8-bit signed
    101, 156, 215, 273, 327, 372, 404, 421,   // 12-bit signed
];

/// I2C repeater bit for tuner access:
/// Page 1, offset 0x01, bit 3 (IIC_repeat).
pub const DEMOD_REG_IIC_REPEAT: u16 = 0x0001;
pub const IIC_REPEAT_BIT: u8 = 1 << 3;

// ---- SYS I2C Master Control Register (I2CMCR, 0x3044) ----
// Bits from your Table (pipe-delimited version):

// Bit 31: IMUR – I2C Master Unit Reset
//   0: Normal
//   1: Reset the I2C Unit (FSM). Self-clears when reset completes.
pub const I2CMCR_IMUR: u32 = 1 << 31;

// Bit 30: CS – Command Start
//   0: Stop (idle). Cleared after completing a whole transaction.
//   1: Start (kick off a transaction).
pub const I2CMCR_CS: u32 = 1 << 30;

// Bits 29:25: RWL – Read/Write Data Length
//   Does not include the slave address byte in the FIFO.
//   Encodes "number_of_bytes - 1", so 0 => 1 byte ... 17 => 24 bytes.
pub const I2CMCR_RWL_SHIFT: u32 = 25;
pub const I2CMCR_RWL_MASK: u32 = 0b1_1111 << I2CMCR_RWL_SHIFT;

// Bit 24: TORE – Time-Out Register Enable
pub const I2CMCR_TORE: u32 = 1 << 24;

// Bits 23:16: TOR – Time-Out Register
//   Time-out = TOR x 2 x ((FD10+1)/Bus clock) per bit.
pub const I2CMCR_TOR_SHIFT: u32 = 16;
pub const I2CMCR_TOR_MASK: u32 = 0xFF << I2CMCR_TOR_SHIFT;
pub const I2CMCR_TOR_DEFAULT: u32 = 0x3A << I2CMCR_TOR_SHIFT; // from your table (reset 0x3A)

// Bit 10: SBAIFD – Second Byte ACK in FRSIB Data (0=check ACK, 1=don't check)
pub const I2CMCR_SBAIFD: u32 = 1 << 10;

// Bit 9: FBAIFD – First Byte ACK in FRSIB Data (0=check, 1=don't check)
pub const I2CMCR_FBAIFD: u32 = 1 << 9;

// Bits 8:7: SRSIB – Second Repeat Start Interval Byte
pub const I2CMCR_SRSIB_SHIFT: u32 = 7;
pub const I2CMCR_SRSIB_MASK: u32 = 0b11 << I2CMCR_SRSIB_SHIFT;

// Bits 6:5: FRSIB – First Repeat Start Interval Byte
pub const I2CMCR_FRSIB_SHIFT: u32 = 5;
pub const I2CMCR_FRSIB_MASK: u32 = 0b11 << I2CMCR_FRSIB_SHIFT;

// Bits 4:3: RSC – Repeat Start Count
//   00: No repeat start
//   01: One repeat start
//   10: Two repeat starts
//   11: Reserved
pub const I2CMCR_RSC_SHIFT: u32 = 3;
pub const I2CMCR_RSC_MASK: u32 = 0b11 << I2CMCR_RSC_SHIFT;

// Bit 2: TEIE – Transaction Error Interrupt Enable
pub const I2CMCR_TEIE: u32 = 1 << 2;

// Bit 1: MRCIE – Master Receive Complete Interrupt Enable
pub const I2CMCR_MRCIE: u32 = 1 << 1;

// Bit 0: MTCIE – Master Transmit Complete Interrupt Enable
pub const I2CMCR_MTCIE: u32 = 1 << 0;

// ---- SYS I2C Master Status Register (I2CMSR, 0x304C) ----
// Bits 31:3 reserved.
// Bit 2: TEIF – Transaction Error Interrupt Flag (W1C).
// Bit 1: MRCIF – Master Receive Complete Interrupt Flag (W1C).
// Bit 0: MTCIF – Master Transmit Complete Interrupt Flag (W1C).
pub const I2CMSR_TEIF: u8 = 1 << 2;
pub const I2CMSR_MRCIF: u8 = 1 << 1;
pub const I2CMSR_MTCIF: u8 = 1 << 0;

// Convenience mask for all known flags.
pub const I2CMSR_ALL_FLAGS: u8 = I2CMSR_TEIF | I2CMSR_MRCIF | I2CMSR_MTCIF;

// ---- SYS I2C Master FIFO/Data Register (I2CMFR, 0x3050) ----
// Bits 7:0 = TDD (Target Device Data). Read for receive.
// Bits 31:8 reserved.
pub const I2CMFR_TDD_MASK: u8 = 0xFF;

