# rust1090_raw

`rust1090_raw` is a from-scratch Rust experiment that talks directly to an RTL2832U USB SDR dongle (e.g. RTL-SDR) **without** using any existing SDR/rtl-sdr/librtlsdr bindings.
<img width="861" height="864" alt="image" src="https://github.com/user-attachments/assets/5871b93c-7f73-42b0-ab88-fe0defe5d07d" />

Right now it:

* Finds and opens an RTL2832U (VID `0x0bda`, PID `0x2832`) over USB
* Performs low-level vendor control transfers to:

  * Power up and initialize the RTL2832U baseband / demodulator
  * Configure the demod sample rate (~2.4 MS/s)
  * Enable/disable the internal test mode
* Resets the USB FIFO and streams raw I/Q bytes from the bulk IN endpoint (`0x81`)
* Runs a **continuous capture loop**, showing sample snapshots to stdout
* **Optionally writes all captured I/Q data** to a `.iq` file for offline analysis
* Has a Rust **ADS-B @ 1090 MHz configuration hook** ready for tuner control (RF tuning not yet implemented)

This is the “driver”/hardware layer that a future ADS-B decoder (like `dump1090`) can be built on top of.

---

## What the project does now

### 1. USB probe and open

* Enumerates USB devices with `rusb`
* Finds the first device with:

  * VID `0x0bda`
  * PID `0x2832`
* Opens it, sets configuration 1, and claims interface 0
* Detaches any kernel driver on interface 0 if necessary
* Reads and prints:

  * Manufacturer
  * Product
  * Serial number (e.g. `1090`)

### 2. Register access sanity checks

Using vendor-specific control transfers, it:

* Reads a known USB register (`USB_SYSCTL` at `0x2000`)
* Writes the same value back and reads it again to verify:

  * USB control transfers are working
  * The RTL2832U responds as expected

On the demod side, it:

* Reads from a known demod register at page `0x0a`, addr `0x0001`
* Writes the value back
* Confirms the readback matches

### 3. Baseband initialization

A Rust port of `rtlsdr_init_baseband` performs:

* **USB block init**

  * Programs `USB_SYSCTL`, `USB_EPA_MAXPKT`, `USB_EPA_CTL`
* **Demod power-on & reset**

  * Uses `DEMOD_CTL` / `DEMOD_CTL_1` in `SYSB` space
  * Issues RTL2832U “soft reset” sequences
* **Demod configuration**

  * Clears DDC/IF shift registers
  * Programs the FIR filter with known coefficients
  * Disables DVB-specific features (AGC loops, PID filter, etc.)
  * Enables SDR mode, zero-IF, DC cancellation, IQ compensation
  * Disables unwanted clock output

### 4. Sample rate configuration

The code:

* Sets a target sample rate of ~**2.4 MS/s**
* Computes the internal `rsamp_ratio` the same way librtlsdr does
* Writes `rsamp_ratio` high/low words into demod registers:

  * `0x9f` (high)
  * `0xa1` (low)
* Performs another demod soft reset so the new rate takes effect
* Logs the “exact” effective rate calculated from the ratio

### 5. Test mode (internal counter) control

The RTL2832U can generate a **built-in 8-bit counter stream** (no RF needed). The code:

* Can enable test mode via a demod register write
* Can disable test mode to return to real ADC I/Q samples

You can see this directly in the capture output:

* **Test mode ON** → bytes look like a clean counter:
  `… e5 e6 e7 e8 e9 ea eb ec … fe ff 00 01 02 03 04 …`

* **Test mode OFF** → bytes look like baseband noise around 0x80:
  `7f 80 7f 80 80 80 7f 80 7f 80 80 80 7f 80 7f 7f …`

### 6. USB FIFO reset

Before streaming, the code:

* Resets the USB endpoint FIFO using `USB_EPA_CTL` writes
* Can clear a STALL (PIPE) on the bulk endpoint with `clear_halt`
* Treats PIPE/Timeout errors gracefully in the capture loop

### 7. Continuous capture loop

Instead of a single bulk read, the program now:

* Finds the bulk IN endpoint (`0x81`, max packet size 512)
* Allocates a 16 KiB buffer
* Enters a **continuous loop**:

  * Calls `read_bulk(endpoint, &mut buf, timeout)` repeatedly
  * Tracks total bytes captured and iteration count
  * Logs:

    * Every block for the first few iterations
    * Then every Nth iteration (e.g. every 100th) to avoid spam
  * Dumps the first 32 bytes of each logged block in hex

You can stop the loop with **Ctrl-C**.

### 8. Optional `.iq` file output

The program now supports **optional raw I/Q capture to disk**:

* If you run with **no arguments**, it behaves as before:

  * Console-only: just logs the first bytes of some blocks.

* If you provide a **path as the first CLI argument**, e.g.:

  ```bash
  sudo ./target/debug/rust1090_raw capture.iq
  ```

  then:

  * The program opens `capture.iq` for writing
  * Every successful bulk read (`Ok(n)`) is appended to the file
  * It uses a `BufWriter<File>` for efficiency
  * If any write fails, it logs an error once and disables file output (capture loop continues without crashing)

The file format is:

* Raw, interleaved **8-bit unsigned I/Q**
* Nominal sample rate ~**2.4 MS/s**

You can open `capture.iq` in tools like:

* `inspectrum`
* `GNU Radio`
* Your own analysis scripts

Configure them for:

* Sample rate: 2.4e6
* Format: 8-bit unsigned IQ interleaved

---

## ADS-B 1090 MHz configuration hook

Right now, **RF tuning is *not* implemented** yet. The RTL2832U baseband and streaming are under Rust’s control, but the external tuner (e.g. R820T/R820T2) still uses whatever configuration it powers up with (or what another tool like `rtl_test` last programmed).

The code now includes:

```rust
const ADSB_CENTER_FREQ_HZ: u32 = 1_090_000_000;
```

and a function:

```rust
fn configure_for_adsb_1090mhz(
    handle: &mut DeviceHandle<GlobalContext>,
) -> rusb::Result<()> {
    println!(
        "configure_for_adsb_1090mhz: tuner programming not yet implemented.\n\
         The device is still using whatever RF center frequency the tuner\n\
         powers up with (or what a previous tool like rtl_test configured)."
    );

    // Future home of:
    //   - I2C-based tuner init
    //   - R82xx PLL programming to ADSB_CENTER_FREQ_HZ
    //   - Tuner bandwidth config, etc.
    Ok(())
}
```

This is a **structural placeholder**: it runs during startup, after sample rate is set, so later when the tuner logic is ported, there is a clear single spot where “tune to 1090 MHz” lives.

---

## Building

From the project root:

```bash
cargo build
```

This produces a debug binary, e.g.:

```text
target/debug/rust1090_raw
```

You’ll need:

* Rust (stable)
* libusb (e.g. `libusb-1.0-0-dev` on Debian/Ubuntu)
* An RTL2832U-based USB dongle

---

## Running

### Console-only mode

```bash
sudo ./target/debug/rust1090_raw
```

You should see:

* Probe logs
* Manufacturer/Product/Serial
* USB/demod register tests
* Baseband init / sample-rate logs
* Optional testmode logs
* Capture loop logs like:

```text
Using bulk IN endpoint: 0x81
Starting capture loop (Ctrl+C to stop)…
Bulk read succeeded: 16384 bytes (total …).
First up-to-32 bytes: 7f 80 7f 80 80 80 …
```

### Capture to `.iq` file

```bash
sudo ./target/debug/rust1090_raw capture.iq
```

This does everything above **and** writes all captured I/Q samples to `capture.iq`.

To visualize:

* Open `capture.iq` in `inspectrum` or GNU Radio.
* Use sample rate ~2.4 MHz, unsigned 8-bit IQ.

---

## Current limitations

* **No tuner programming yet**
  The RF front-end (R820T/R820T2 etc.) is not configured from Rust; it uses its default / previous configuration.

* **No ADS-B demodulation**
  The project currently captures baseband samples only. There is no Mode S / ADS-B pulse detection or message decoding yet.

* **Single device, fixed parameters**

  * Fixed RTL2832U VID/PID (`0x0bda:0x2832`)
  * Hard-coded sample rate (~2.4 MS/s)
  * No gain control or tuner bandwidth settings yet

* **No cross-platform polishing**
  It should work where `rusb` and libusb work, but most testing is on Linux.

---

## Next steps

The next planned milestones focus on **RF tuning** and preparing for ADS-B decoding:

1. **I²C helper functions (SYS I²C registers)**
   Implement Rust wrappers for the RTL2832U’s I²C master:

   * Use the SYS I²C registers (e.g. `SYS_I2CCR`, `SYS_I2CMCR`, `SYS_I2CMSR`, `SYS_I2CMFR`) to:

     * Send I²C writes/reads to the external tuner chip
   * Expose helpers like:

     * `rtl_i2c_write(handle, i2c_addr, data)`
     * `rtl_i2c_read(handle, i2c_addr, buf)`

2. **Tiny “probe tuner ID over I²C” function**
   As a self-contained milestone:

   * Talk to the tuner’s I²C address (e.g. 0x34 for R820T)
   * Read back an ID or status register
   * Confirm we can **see the tuner** over I²C from Rust

   This will prove the I²C path works before tackling full tuning.

3. **Tuner tuning to 1090 MHz**
   After the I²C probe works:

   * Port a minimal subset of the tuner driver logic (`tuner_r82xx.c`) to Rust
   * Implement a function like `r82xx_set_freq(handle, ADSB_CENTER_FREQ_HZ)`
   * Call it from `configure_for_adsb_1090mhz` so capture is actually centered on 1090 MHz

4. **ADS-B / Mode S decoder**
   With samples at 1090 MHz:

   * Implement amplitude-based demodulation (like `dump1090`)
   * Detect Mode S preambles and decode frames
   * Validate messages, decode ADS-B payloads, and display aircraft info

---

## Status summary

`rust1090_raw` is currently a **low-level RTL2832U driver and capture tool in Rust**, with:

* Full USB + demod register control
* Baseband initialization and sample rate setup
* Test mode on/off verification
* A continuous capture loop
* Optional `.iq` file output
* A clear hook for “ADS-B @ 1090 MHz” tuning logic

The next concrete milestone is to **bring up I²C to the tuner** and implement a tiny “probe tuner ID” function as the first step toward real 1090 MHz tuning and, eventually, a full Rust ADS-B receiver.

