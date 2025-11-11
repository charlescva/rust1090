That output is *exactly* what we were hoping to see 🙌

Let’s decode it:

```text
I2C repeater (IIC_repeat) ENABLED
  SYS I2C clock (SYS_I2CCR) set to FD10=0x13.
  Probing tuner I2C addresses for reg 0x00…
    Tuner probe: I2C addr 0x34, reg 0x00 -> 0x69
    Tuner probe: I2C addr 0x35, reg 0x00 -> 0x69
    Tuner probe: I2C addr 0x36, reg 0x00 read failed: Pipe
    Tuner probe: I2C addr 0x38, reg 0x00 read failed: Pipe
  Tuner probe complete (see values above).
```

### What this tells us

1. **I2C repeater is working**
   We successfully enabled `IIC_repeat` and then got valid responses back from the tuner. So the demod → tuner I²C path is alive.

2. **The tuner answered at 0x34 and 0x35 with `0x69`**
   That’s exactly what you’d expect for an R820T/R828D-style tuner:

   * The “address” 0x34 is the **8-bit I²C address** where the **LSB is the R/W bit**.
   * So:

     * `0x34` = (7-bit addr `0x1a` << 1) | 0 (write)
     * `0x35` = (7-bit addr `0x1a` << 1) | 1 (read)

   We’re calling both of them with the same vendor command, so both end up mapping to the same physical device, hence both return `0x69`.

   `0x69` is the classic “chip ID” byte for R820T-type tuners. So:

   > ✅ You have an R820T-family tuner, reachable at base I²C address 0x34/0x35.

3. **0x36 and 0x38 PIPE**
   Those giving `Pipe` is good: it means “no device there / not responding,” which confirms 0x34/0x35 is the one that matters.

So the milestone we wanted:

> “Bring up I2C to the tuner and implement a tiny ‘probe tuner ID’ function”

is achieved. 🎯

We now *know*:

* The I²C repeater bit is under our control.
* The tuner is present and answering.
* The tuner type is almost certainly R820T/R828D.
* The tuner’s base address is 0x34 (with 0x35 as the read variant).

---

### Tiny cleanup you *can* do next (no big changes)

If you want to lock this into the code for future steps, you can now safely add:

```rust
const TUNER_I2C_ADDR: u8 = 0x34; // R820T/R828D base (write) address
```

and, in `configure_for_adsb_1090mhz`, once the probe succeeds, maybe log:

```rust
println!(
    "  Detected tuner at I2C addr 0x{:02x}, reg 0x00 = 0x{:02x} (likely R820T/R828D).",
    0x34, 0x69
);
```

But the important part is: **I²C access works, and we’ve positively identified the tuner.**

Whenever you’re ready, the next step after this is to start porting a *minimal* `r82xx_set_freq`-style sequence for 1090 MHz from the C side into Rust, using `TUNER_I2C_ADDR` and our `rtl_get_tuner_reg` / `rtl_set_tuner_reg` logic.

