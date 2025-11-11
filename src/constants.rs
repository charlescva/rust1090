// src/pub constants.rs
// Note the pub keyword, which makes the constants public and accessible from other modules.



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
