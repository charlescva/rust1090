# rust1090 - ADS-B in Pure Rust!

`rust1090` is a from-scratch Rust experiment that talks directly to an RTL2832U USB SDR dongle (e.g. RTL-SDR) **without** using any existing SDR / rtl-sdr / librtlsdr bindings.

Right now it:

* Finds and opens an RTL2832U (VID `0x0bda`, PID `0x2832`) over USB
* Performs low-level vendor control transfers to:

  * Power up and initialize the baseband / demodulator
  * Configure the sample rate
  * Enable/disable the internal test mode
* Streams raw I/Q bytes from the device over the bulk endpoint
* Prints a small slice of the received bytes so you can see what the device is doing

This is the “driver”/hardware layer that a future ADS-B decoder (like `dump1090`) can be built on top of.

---

## What this project *actually* does right now

At its current stage, the program:

1. **Probes USB**

   * Enumerates USB devices with libusb (via `rusb`)
   * Finds the first device with VID `0x0bda` and PID `0x2832`
   * Opens it, sets configuration 1, and claims interface 0
   * Detaches any kernel driver on interface 0 if needed

2. **Verifies control transfers work**

   * Reads and prints the Manufacturer, Product, and Serial strings
   * Reads a known USB register (`USB_SYSCTL` at `0x2000`)
   * Writes the same value back and reads again to verify the path

3. **Initializes the RTL2832U baseband**
   Using vendor-specific control transfers, it:

   * Powers on the demodulator via `DEMOD_CTL` / `DEMOD_CTL_1` in `SYSB`
   * Performs a demod “soft reset”
   * Programs the baseband FIR filter with known coefficients
   * Disables DVB-specific features (AGC loops, PID filter, etc.)
   * Enables SDR mode, zero-IF, DC cancellation, and IQ compensation

4. **Configures the sample rate**

   * Sets a target sample rate (currently ~**2.4 MS/s**)
   * Computes and programs the internal `rsamp_ratio` into the demod registers
   * Performs another demod soft reset so the new rate takes effect

5. **Controls test mode**

   * Can enable the demod’s internal **test counter stream**
   * Can disable test mode and return to real ADC I/Q samples

6. **Resets the USB FIFO / buffer**

   * Uses USB register writes to reset the EPA (bulk endpoint) FIFO
   * Clears halts (STALL) on the bulk endpoint when needed

7. **Streams data over bulk endpoint 0x81**

   * Locates the bulk IN endpoint with max packet size 512
   * Allocates a 16 KiB buffer and issues a synchronous bulk read
   * On success, prints the first 32 bytes in hex

You should see a continuous loop printing the first 32 bytes of the 16KiB from the bulk endpoint:

* With **test mode ON**: a clean monotonically increasing byte pattern
  e.g. `… e5 e6 e7 e8 … fe ff 00 01 02 03 04 …`

* With **test mode OFF**: “noisy” I/Q centered near 0x80
  e.g. `7f 80 7f 80 80 80 7f 80 …` (typical idle RF baseband)

---

## Project goals

The long-term goal is to replicate the core of rtl-sdr in Rust:

* **Driver layer**: talk to RTL2832U directly via USB (this is what exists now)
* **Tuning**: configure the tuner (e.g. R820T) over I²C to lock to **1090 MHz**
* **Baseband capture**: continuous streaming of I/Q at 2.4 MS/s
* **ADS-B / Mode S decode**:

  * Detect pulses and demodulate Mode S frames
  * Check CRC and extract ADS-B messages
  * Display aircraft information (position, altitude, callsign, etc.)

This repo currently focuses only on the **driver + streaming** part.

---

## Requirements

* **Rust** (stable toolchain)
* **libusb** on your system (e.g. `libusb-1.0-0-dev` on Debian/Ubuntu)
* An **RTL2832U-based dongle** (e.g. standard RTL-SDR USB stick)
* Sufficient permissions to access USB devices:

  * Easiest: run the binary with `sudo`
  * Longer-term: add a udev rule so root isn’t required

---

## Building

From the project root:

```bash
cargo build
```

This will produce a debug binary at:

```text
target/debug/rust1090
```

(Or whatever binary name your `Cargo.toml` specifies.)

---

## Running

Most simply (on Linux):

```bash
sudo ./target/debug/rust1090
```

Expected high-level output:

* It finds the RTL2832U device (bus, address, VID/PID)
* Shows manufacturer/product/serial strings (e.g. Realtek / RTL2832U / 1090)
* Prints a successful USB SYSCTL register read/write
* Runs baseband init and sample-rate configuration
* Optionally toggles test mode
* Resets the USB FIFO
* Locates endpoint 0x81 and does a bulk read
* Prints:

```text
Bulk read succeeded, 16384 bytes.
First up-to-32 bytes: ...
```

If test mode is **enabled**, those bytes form a simple counter.
If test mode is **disabled**, they look like 0x7f/0x80-ish “noise”.

If you see `Timeout` or `Pipe` messages, that usually means:

* The device isn’t fully initialized yet, or
* The FIFO was stalled and needs a reset/clear (the code attempts to handle this).

---

## Current limitations

* No tuner configuration yet (no real 1090 MHz tuning)
* No ADS-B / Mode S demod or decoding
* USB VID/PID is hard-coded for classic RTL2832U sticks
* No cross-platform support beyond “whatever libusb + rusb can handle” (tested on Linux)

---

## Next steps

Things that could be added on top of the current code:

1. **Tuner support**

   * Implement the same I²C register programming as librtlsdr for R8xx tuners
   * Add a function like `set_center_frequency(1_090_000_000)` (1090 MHz)

2. **ADS-B decoder**

   * Implement amplitude-based demod (like dump1090) over 2.4 MS/s I/Q
   * Detect Mode S preambles and decode frames
   * Display or output decoded ADS-B messages

3. **Command-line options**

   * Select sample rate, test mode, center frequency, buffer size, etc.

---

## Status summary

This project is currently a **low-level RTL2832U driver in Rust** that:

* Shows how to talk to the dongle directly via USB control transfers
* Reimplements a minimal subset of librtlsdr’s init logic
* Proves end-to-end streaming by:

  * Enabling internal test mode (counter pattern)
  * Disabling it and reading live I/Q samples

It’s a solid foundation for building a pure-Rust ADS-B receiver on top.
