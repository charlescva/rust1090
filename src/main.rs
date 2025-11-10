// Find the RTL-SDR USB dongle
// Open it
// claim the interface
// find the bulk IN endpoint
// try to read raw bytes.
// Bus 001 Device 003: ID 0bda:2832 Realtek Semiconductor Corp. RTL2832U DVB-T

use rusb::{
    self, devices, Device, DeviceDescriptor, DeviceHandle, Direction, GlobalContext, TransferType, Error, 
};

use std::time::Duration;    

const RTL_VID: u16 = 0x0bda;
// Common Realtek DVB-T dongle PIDs used by RTL-SDR sticks.
// Adjust/add if your lsusb shows something else.
const RTL_PIDS: &[u16] = &[
    0x2832, // RTL2832U (common "RTL2832U" DVB-T)
];

// Vendor-specific control transfer flags for RTL2832U
// bmRequestType values from RTL2832U datasheet vendor command table.
// 0xC0 = IN, Vendor, Device  |  0x40 = OUT, Vendor, Device
const CTRL_IN: u8 = 0xC0;
const CTRL_OUT: u8 = 0x40;

// Control transfer timeout (milliseconds)
const CTRL_TIMEOUT_MS: u64 = 1000;

// USB register address we’ll test: USB_SYSCTL byte 0 at 0x2000
// (see RTL2832U datasheet, USB SIE control registers). :contentReference[oaicite:1]{index=1}
const USB_SYSCTL_0: u16 = 0x2000;

// Demod "pages" and a known-safe test register.
// These values are taken from how librtlsdr probes the RTL2832U demod: it uses
// rtlsdr_demod_read_reg(dev, 0x0a, 0x01, 1). :contentReference[oaicite:1]{index=1}
const DEMOD_PAGE_SYS: u8 = 0x0a;
const DEMOD_REG_SYS_TEST: u16 = 0x0001;

// RTL2832 "blocks" (same as enum blocks in librtlsdr)
const BLOCK_DEMODB: u8 = 0;
const BLOCK_USBB:  u8 = 1;
const BLOCK_SYSB:  u8 = 2;
// (others exist but we don't need them yet)

// USB and SYS register addresses (same values you found)
const USB_SYSCTL:      u16 = 0x2000;
const USB_EPA_CTL:     u16 = 0x2148;
const USB_EPA_MAXPKT:  u16 = 0x2158;

const DEMOD_CTL:       u16 = 0x3000;
const DEMOD_CTL_1:     u16 = 0x300b;

// FIR_LEN = 16
const FIR_DEFAULT: [i16; 16] = [
    -54, -36, -41, -40, -32, -14, 14, 53,     // 8-bit signed
    101, 156, 215, 273, 327, 372, 404, 421,   // 12-bit signed
];


fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> rusb::Result<()> {
    println!("rust1090_raw: probing for RTL2832 USB device...");
    
    let (device, desc) = find_rtl_device()?;
    println!(
        "Found device: VID={:#06x}, PID={:#06x}",
            desc.vendor_id(),
            desc.product_id()
    );

    let mut handle = open_and_claim(&device, &desc)?;
    
    // NEW: sanity-check control transfers (EP0)
    print_device_strings(&handle, &desc);
    
    // test vendor command register access
    if let Err(e) = test_usb_sysctl_register(&mut handle) {
        eprintln!("Vendor USB register test failed: {:?}", e);
    }
    
    //init baseband
    if let Err(e) = init_baseband(&mut handle) {
        eprintln!("Baseband init failed: {:?}", e);
    }
    
    // test demod register access
    if let Err(e) = test_demod_register(&mut handle) {
        eprintln!("Demod register test failed: {:?}", e);
    }
    
    // set a reasonable sample rate (e.g. 2.4 MS/s)
    if let Ok(real_rate) = rtl_set_sample_rate(&mut handle, 2_400_000) {
        println!("rtl_set_sample_rate: configured real_rate ≈ {} Hz", real_rate);
    } else {
        eprintln!("rtl_set_sample_rate failed; continuing anyway.");
    }

    // enable internal test mode (8-bit counter stream)
    if let Err(e) = rtl_set_testmode(&mut handle, false) {
        eprintln!("rtl_set_testmode failed: {:?}", e);
    }
    
    // Reset FIFO after changing streaming mode
    if let Err(e) = rtl_reset_buffer(&mut handle) {
        eprintln!("rtl_reset_buffer failed: {:?}", e);
    }
    
    let endpoint = find_bulk_in_endpoint(&device)?;
    println!("Using bulk IN endpoint: 0x{:02x}", endpoint);

    // Continuous capture loop.
    let mut buf = vec![0u8; 16 * 1024]; // small-ish buffer for now
    let timeout = Duration::from_secs(1); // shorter timeout so the loop is responsive
    let mut total_bytes: u64 = 0;
    let mut iterations: u64 = 0;

    println!("Starting capture loop (Ctrl+C to stop)…");

    loop {
        iterations += 1;

        match handle.read_bulk(endpoint, &mut buf, timeout) {
            Ok(n) => {
                total_bytes += n as u64;

                // Print details for the first few blocks, then periodically.
                if iterations <= 5 || iterations % 100 == 0 {
                    println!(
                        "Bulk read succeeded: {} bytes (total {} bytes, iter {}).",
                        n, total_bytes, iterations
                    );
                    let to_show = n.min(32);
                    print!("First up-to-32 bytes: ");
                    for b in &buf[..to_show] {
                        print!("{:02x} ", b);
                    }
                    println!();
                }
            }

            Err(Error::Timeout) => {
                // Occasional timeout log so you know it's still alive.
                if iterations % 100 == 0 {
                    println!(
                        "Timeout on iteration {} (total {} bytes so far)…",
                        iterations, total_bytes
                    );
                }
                // Just keep looping.
            }

            Err(Error::Pipe) => {
                eprintln!(
                    "Bulk read got PIPE (endpoint stalled) on iter {}. Trying to clear halt…",
                    iterations
                );
                if let Err(e) = clear_bulk_endpoint_halt(&mut handle, endpoint) {
                    eprintln!("  Failed to clear halt on endpoint: {:?}. Stopping.", e);
                    break;
                } else {
                    // After clearing halt, just continue the loop; next read should work.
                    continue;
                }
            }

            Err(e) => {
                eprintln!("Bulk read failed with unexpected error on iter {}: {:?}", iterations, e);
                break;
            }
        }
    }

    Ok(())

}

/// Find the first USB device matching our RTL VID/PID set.

fn find_rtl_device() -> rusb::Result<(Device<GlobalContext>, DeviceDescriptor)> {
    let list = devices()?;
    println!("Total USB devices found: {}", list.len());
    
    for device in list.iter() {
        let desc = match device.device_descriptor() {
            Ok(d) => d,
            Err(_) => continue,
        };
        
        let vid = desc.vendor_id();
        let pid = desc.product_id();
        
        if vid == RTL_VID && RTL_PIDS.contains(&pid) {
            println!(
                "Matched Realtek RTL device at bus {} address {} (VID={:#06x}, PID={:#06x})",
                device.bus_number(),
                device.address(),
                vid,
                pid
            );
            return Ok((device, desc));
        }
    }
    
    Err(rusb::Error::NoDevice)
}

/// Open the device, detach kernel driver (if necessary), and claim interface 0.
fn open_and_claim(
    device: &Device<GlobalContext>,
    desc: &DeviceDescriptor,
) -> rusb::Result<DeviceHandle<GlobalContext>> {
    let mut handle = device.open()?;
    println!("Device opened.");

    // We'll assume configuration 0 / interface 0 for now, which is what
    // these DVB-T sticks normally use for the streaming interface.
    let iface = 0u8;

    // On Linux we may need to detach the DVB kernel driver first.
    #[cfg(unix)]
    {
        if handle.kernel_driver_active(iface)? {
            println!("Kernel driver is active on interface {iface}, detaching...");
            handle.detach_kernel_driver(iface)?;
        }
    }

    // Make sure the device is in a known configuration. Usually 0 or 1.
    // We query the first configuration value to be explicit.
    let config_desc = device.config_descriptor(0)?;
    let config_value = config_desc.number();
    println!("Setting active configuration to {}...", config_value);
    handle.set_active_configuration(config_value)?;

    println!("Claiming interface {iface}...");
    handle.claim_interface(iface)?;

    println!("Interface claimed successfully.");
    Ok(handle)
}

/// Inspect descriptors and find the first bulk IN endpoint on interface 0.
fn find_bulk_in_endpoint(device: &Device<GlobalContext>) -> rusb::Result<u8> {
    let config = device.active_config_descriptor()?;

    for interface in config.interfaces() {
        for interface_desc in interface.descriptors() {
            let iface_num = interface_desc.interface_number();
            if iface_num != 0 {
                continue; // we only care about interface 0 for now
            }

            println!(
                "Inspecting interface {}, alt-setting {}",
                iface_num,
                interface_desc.setting_number()
            );

            for ep in interface_desc.endpoint_descriptors() {
                let addr = ep.address();
                let dir = ep.direction();
                let tt = ep.transfer_type();

                println!(
                    "  Endpoint 0x{:02x}: dir={:?}, type={:?}, max_packet_size={}",
                    addr,
                    dir,
                    tt,
                    ep.max_packet_size()
                );

                if dir == Direction::In && tt == TransferType::Bulk {
                    return Ok(addr);
                }
            }
        }
    }

    Err(rusb::Error::Pipe) // crude, but “no suitable endpoint” situation
}

fn print_device_strings(handle: &DeviceHandle<GlobalContext>, desc: &DeviceDescriptor) {
    // These calls use standard USB control transfers on endpoint 0 (EP0).
    match handle.read_manufacturer_string_ascii(desc) {
        Ok(s) => println!("Manufacturer: {}", s),
        Err(e) => println!("Manufacturer: <unavailable> ({:?})", e),
    }

    match handle.read_product_string_ascii(desc) {
        Ok(s) => println!("Product: {}", s),
        Err(e) => println!("Product: <unavailable> ({:?})", e),
    }

    match handle.read_serial_number_string_ascii(desc) {
        Ok(s) => println!("Serial: {}", s),
        Err(e) => println!("Serial: <unavailable> ({:?})", e),
    }
}
/// Low-level helper to perform a vendor-specific control *read*.
///
/// This is wired to the RTL2832U "GetUSBReg" command:
///   bmRequestType = 0xC0 (device-to-host, vendor, device)
///   bRequest      = 0
///   wValue        = register address (e.g. 0x2000)
///   wIndex        = 0x0100 (USB block, "GetUSBReg")
///   wLength       = buf.len()
fn rtl_get_usb_reg(
    handle: &mut DeviceHandle<GlobalContext>,
    addr: u16,
    buf: &mut [u8],
) -> Result<usize, rusb::Error> {
    let request_type = CTRL_IN;
    let request = 0u8; // bRequest is "x" in the datasheet; rtl-sdr uses 0.
    let value = addr;
    let index = 0x0100u16; // USB block, GetUSBReg (see vendor command table). :contentReference[oaicite:2]{index=2}

    handle.read_control(
        request_type,
        request,
        value,
        index,
        buf,
        Duration::from_millis(CTRL_TIMEOUT_MS),
    )
}

/// Low-level helper to perform a vendor-specific control *write*.
///
/// This is wired to the RTL2832U "SetUSBReg" command:
///   bmRequestType = 0x40 (host-to-device, vendor, device)
///   bRequest      = 0
///   wValue        = register address (e.g. 0x2000)
///   wIndex        = 0x0110 (USB block, "SetUSBReg")
///   wLength       = data.len()
fn rtl_set_usb_reg(
    handle: &mut DeviceHandle<GlobalContext>,
    addr: u16,
    data: &[u8],
) -> Result<usize, rusb::Error> {
    let request_type = CTRL_OUT;
    let request = 0u8;
    let value = addr;
    let index = 0x0110u16; // USB block, SetUSBReg. :contentReference[oaicite:3]{index=3}

    handle.write_control(
        request_type,
        request,
        value,
        index,
        data,
        Duration::from_millis(CTRL_TIMEOUT_MS),
    )
}

/// Read from a demodulator register using the same control transfer layout
/// as librtlsdr's rtlsdr_demod_read_reg:
///   wValue = (addr << 8) | 0x20
///   wIndex = page
fn rtl_get_demod_reg(
    handle: &mut DeviceHandle<GlobalContext>,
    page: u8,
    addr: u16,
    buf: &mut [u8],
) -> Result<usize, rusb::Error> {
    // In rtl-sdr, 'len' is 1 or 2; we assume buf.len() is in that range.
    let index: u16 = page as u16;
    let value: u16 = (addr << 8) | 0x20;

    handle.read_control(
        CTRL_IN,  // 0xC0: device-to-host, vendor, device
        0,        // bRequest
        value,    // wValue
        index,    // wIndex
        buf,
        Duration::from_millis(CTRL_TIMEOUT_MS),
    )
}

/// Write to a demodulator register using the same layout as
/// rtlsdr_demod_write_reg:
///   wValue = (addr << 8) | 0x20
///   wIndex = 0x10 | page
fn rtl_set_demod_reg(
    handle: &mut DeviceHandle<GlobalContext>,
    page: u8,
    addr: u16,
    data: &[u8],
) -> Result<usize, rusb::Error> {
    let index: u16 = 0x10 | (page as u16);
    let value: u16 = (addr << 8) | 0x20;

    handle.write_control(
        CTRL_OUT, // 0x40: host-to-device, vendor, device
        0,        // bRequest
        value,    // wValue
        index,    // wIndex
        data,
        Duration::from_millis(CTRL_TIMEOUT_MS),
    )
}



/// Test that we can read and write a known USB register (USB_SYSCTL[7:0]).
///
/// USB_SYSCTL is at base address 0x2000 in the chip’s USB register map. :contentReference[oaicite:5]{index=5}
fn test_usb_sysctl_register(
    handle: &mut DeviceHandle<GlobalContext>,
) -> Result<(), rusb::Error> {
    println!("Testing vendor-specific control transfers (USB_SYSCTL_0 at 0x2000)...");

    let mut buf = [0u8; 1];

    // 1) Read original value
    let n = rtl_get_usb_reg(handle, USB_SYSCTL_0, &mut buf)?;
    if n != 1 {
        println!(
            "Unexpected read length from USB_SYSCTL_0: {} bytes (expected 1)",
            n
        );
    }
    let original = buf[0];
    println!("  Original USB_SYSCTL_0 value: 0x{:02x}", original);

    // 2) Write the same value back (safe "no-op" write)
    let written = rtl_set_usb_reg(handle, USB_SYSCTL_0, &[original])?;
    if written != 1 {
        println!(
            "  Warning: wrote {} bytes back to USB_SYSCTL_0 (expected 1)",
            written
        );
    } else {
        println!("  Wrote USB_SYSCTL_0 back unchanged (0x{:02x})", original);
    }

    // 3) Read again to confirm the transfer path works
    let n2 = rtl_get_usb_reg(handle, USB_SYSCTL_0, &mut buf)?;
    let readback = buf[0];
    println!(
        "  Readback USB_SYSCTL_0 value: 0x{:02x} ({} bytes)",
        readback, n2
    );

    Ok(())
}

/// Test that we can actually read and write a demod register:
/// This mirrors: rtlsdr_demod_read_reg(dev, 0x0a, 0x01, 1);
fn test_demod_register(
    handle: &mut DeviceHandle<GlobalContext>,
) -> Result<(), rusb::Error> {
    println!(
        "Testing demod vendor-specific control transfers (page=0x{:02x}, addr=0x{:04x})...",
        DEMOD_PAGE_SYS,
        DEMOD_REG_SYS_TEST,
    );

    let mut buf = [0u8; 1];

    // 1) Read original value
    let n = rtl_get_demod_reg(handle, DEMOD_PAGE_SYS, DEMOD_REG_SYS_TEST, &mut buf)?;
    if n != 1 {
        println!(
            "  Unexpected read length from demod reg page=0x{:02x}, addr=0x{:04x}: {} bytes (expected 1)",
            DEMOD_PAGE_SYS,
            DEMOD_REG_SYS_TEST,
            n
        );
    }
    let original = buf[0];
    println!(
        "  Original DEMOD[page=0x{:02x}, addr=0x{:04x}] value: 0x{:02x}",
        DEMOD_PAGE_SYS,
        DEMOD_REG_SYS_TEST,
        original
    );

    // 2) Write the same value back (no-op)
    let written = rtl_set_demod_reg(
        handle,
        DEMOD_PAGE_SYS,
        DEMOD_REG_SYS_TEST,
        &[original],
    )?;
    if written != 1 {
        println!(
            "  Warning: wrote {} bytes to demod reg page=0x{:02x}, addr=0x{:04x} (expected 1)",
            written,
            DEMOD_PAGE_SYS,
            DEMOD_REG_SYS_TEST,
        );
    } else {
        println!(
            "  Wrote DEMOD[page=0x{:02x}, addr=0x{:04x}] back unchanged (0x{:02x})",
            DEMOD_PAGE_SYS,
            DEMOD_REG_SYS_TEST,
            original
        );
    }

    // 3) Read again
    let n2 = rtl_get_demod_reg(handle, DEMOD_PAGE_SYS, DEMOD_REG_SYS_TEST, &mut buf)?;
    let readback = buf[0];
    println!(
        "  Readback DEMOD[page=0x{:02x}, addr=0x{:04x}] value: 0x{:02x} ({} bytes)",
        DEMOD_PAGE_SYS,
        DEMOD_REG_SYS_TEST,
        readback,
        n2
    );

    Ok(())
}

fn rtl_write_reg(
    handle: &mut DeviceHandle<GlobalContext>,
    block: u8,
    addr: u16,
    val: u16,
    len: u8,
) -> Result<(), rusb::Error> {
    let mut data = [0u8; 2];

    // Encode value into data[0..len]
    if len == 1 {
        data[0] = (val & 0xff) as u8;
    } else {
        data[0] = (val >> 8) as u8;
    }
    data[1] = (val & 0xff) as u8;

    // wIndex = (block << 8) | 0x10  (no IRB special-case needed yet)
    let index: u16 = ((block as u16) << 8) | 0x10;

    let written = handle.write_control(
        CTRL_OUT,
        0,              // bRequest
        addr,           // wValue = register address (e.g. 0x3000)
        index,          // wIndex = block << 8 | 0x10
        &data[..len as usize],
        Duration::from_millis(CTRL_TIMEOUT_MS),
    )?;

    if written != len as usize {
        eprintln!(
            "rtl_write_reg: wrote {} bytes (expected {}) to block {} addr 0x{:04x}",
            written, len, block, addr
        );
    }

    Ok(())
}

/// Write a demod register with a value/length API, mirroring librtlsdr.
/// Internally delegates to `rtl_set_demod_reg` after packing `val` to 1 or 2 bytes.
fn rtl_demod_write_reg(
    handle: &mut DeviceHandle<GlobalContext>,
    page: u8,
    addr: u16,
    val: u16,
    len: u8,
) -> Result<usize, rusb::Error> {
    let mut data = [0u8; 2];
    match len {
        1 => {
            data[0] = (val & 0x00ff) as u8;
        }
        2 => {
            data[0] = (val >> 8) as u8;
            data[1] = (val & 0x00ff) as u8;
        }
        _ => {
            // librtlsdr only uses len 1 or 2 for these calls
            return Err(rusb::Error::InvalidParam);
        }
    }
    rtl_set_demod_reg(handle, page, addr, &data[..len as usize])
}

fn init_baseband(handle: &mut DeviceHandle<GlobalContext>) -> Result<(), rusb::Error> {
    println!("Initializing baseband (USB + demod power-up)...");

    // --- initialize USB ---
    // rtlsdr_write_reg(dev, USBB, USB_SYSCTL, 0x09, 1);
    rtl_write_reg(handle, BLOCK_USBB, USB_SYSCTL_0, 0x0009, 1)?;

    // rtlsdr_write_reg(dev, USBB, USB_EPA_MAXPKT, 0x0002, 2);
    rtl_write_reg(handle, BLOCK_USBB, USB_EPA_MAXPKT, 0x0002, 2)?;

    // rtlsdr_write_reg(dev, USBB, USB_EPA_CTL, 0x1002, 2);
    rtl_write_reg(handle, BLOCK_USBB, USB_EPA_CTL, 0x1002, 2)?;

    // --- power on demod ---
    // rtlsdr_write_reg(dev, SYSB, DEMOD_CTL_1, 0x22, 1);
    rtl_write_reg(handle, BLOCK_SYSB, DEMOD_CTL_1, 0x0022, 1)?;

    // rtlsdr_write_reg(dev, SYSB, DEMOD_CTL, 0xe8, 1);
    rtl_write_reg(handle, BLOCK_SYSB, DEMOD_CTL, 0x00e8, 1)?;

    // --- reset demod (bit 3, soft_rst) ---
    // rtlsdr_demod_write_reg(dev, 1, 0x01, 0x14, 1);
    // rtlsdr_demod_write_reg(dev, 1, 0x01, 0x10, 1);
    rtl_demod_write_reg(handle, 1, 0x0001, 0x0014, 1)?;
    rtl_demod_write_reg(handle, 1, 0x0001, 0x0010, 1)?;

    // --- disable spectrum inversion and adjacent channel rejection ---
    // rtlsdr_demod_write_reg(dev, 1, 0x15, 0x00, 1);
    // rtlsdr_demod_write_reg(dev, 1, 0x16, 0x0000, 2);
    rtl_demod_write_reg(handle, 1, 0x0015, 0x0000, 1)?;
    rtl_demod_write_reg(handle, 1, 0x0016, 0x0000, 2)?;

    // --- clear both DDC shift and IF frequency registers ---
    // for (i = 0; i < 6; i++)
    //     rtlsdr_demod_write_reg(dev, 1, 0x16 + i, 0x00, 1);
    for offset in 0u16..6 {
        rtl_demod_write_reg(handle, 1, 0x0016 + offset, 0x0000, 1)?;
    }

    // --- program FIR ---
    rtl_set_fir(handle)?;

    // --- enable SDR mode, disable DAGC (bit 5) ---
    // rtlsdr_demod_write_reg(dev, 0, 0x19, 0x05, 1);
    rtl_demod_write_reg(handle, 0, 0x0019, 0x0005, 1)?;

    // --- init FSM state-holding register ---
    // rtlsdr_demod_write_reg(dev, 1, 0x93, 0xf0, 1);
    // rtlsdr_demod_write_reg(dev, 1, 0x94, 0x0f, 1);
    rtl_demod_write_reg(handle, 1, 0x0093, 0x00f0, 1)?;
    rtl_demod_write_reg(handle, 1, 0x0094, 0x000f, 1)?;

    // --- disable AGC (en_dagc, bit 0) ---
    // rtlsdr_demod_write_reg(dev, 1, 0x11, 0x00, 1);
    rtl_demod_write_reg(handle, 1, 0x0011, 0x0000, 1)?;

    // --- disable RF and IF AGC loop ---
    // rtlsdr_demod_write_reg(dev, 1, 0x04, 0x00, 1);
    rtl_demod_write_reg(handle, 1, 0x0004, 0x0000, 1)?;

    // --- disable PID filter (enable_PID = 0) ---
    // rtlsdr_demod_write_reg(dev, 0, 0x61, 0x60, 1);
    rtl_demod_write_reg(handle, 0, 0x0061, 0x0060, 1)?;

    // --- opt_adc_iq = 0, default ADC_I/ADC_Q datapath ---
    // rtlsdr_demod_write_reg(dev, 0, 0x06, 0x80, 1);
    rtl_demod_write_reg(handle, 0, 0x0006, 0x0080, 1)?;

    // --- Enable Zero-IF, DC cancellation, IQ estimation/compensation ---
    // rtlsdr_demod_write_reg(dev, 1, 0xb1, 0x1b, 1);
    rtl_demod_write_reg(handle, 1, 0x00b1, 0x001b, 1)?;

    // --- disable 4.096 MHz clock output on pin TP_CK0 ---
    // rtlsdr_demod_write_reg(dev, 0, 0x0d, 0x83, 1);
    rtl_demod_write_reg(handle, 0, 0x000d, 0x0083, 1)?;

    Ok(())
}


fn clear_bulk_endpoint_halt(
    handle: &mut DeviceHandle<GlobalContext>,
    endpoint: u8,
) -> Result<(), rusb::Error> {
    println!(
        "Clearing halt (STALL) condition on endpoint 0x{:02x}...",
        endpoint
    );
    handle.clear_halt(endpoint)?;
    Ok(())
}

fn rtl_reset_buffer(handle: &mut DeviceHandle<GlobalContext>) -> Result<(), rusb::Error> {
    println!("Resetting RTL2832U USB FIFO/buffer...");

    // Match rtlsdr_reset_buffer(dev) from librtlsdr.c:
    //   rtlsdr_write_reg(dev, USBB, USB_EPA_CTL, 0x1002, 2);
    //   rtlsdr_write_reg(dev, USBB, USB_EPA_CTL, 0x0000, 2);
    rtl_write_reg(handle, BLOCK_USBB, USB_EPA_CTL, 0x1002, 2)?;
    rtl_write_reg(handle, BLOCK_USBB, USB_EPA_CTL, 0x0000, 2)?;

    Ok(())
}

/// Enable or disable the RTL2832U internal test mode.
///
/// Exact match to librtlsdr's rtlsdr_set_testmode(dev, on):
///   rtlsdr_demod_write_reg(dev, 0, 0x19, on ? 0x03 : 0x05, 1);
fn rtl_set_testmode(
    handle: &mut DeviceHandle<GlobalContext>,
    on: bool,
) -> Result<(), rusb::Error> {
    let value: u16 = if on { 0x03 } else { 0x05 };

    println!(
        "Setting RTL2832U test mode {} (page=0, addr=0x0019, val=0x{:02x})...",
        if on { "ON" } else { "OFF" },
        value
    );

    rtl_demod_write_reg(handle, 0, 0x0019, value, 1)?;
    Ok(())
}

/// Configure the RTL2832U demod sample rate.
///
/// This mirrors the core of `rtlsdr_set_sample_rate(dev, samp_rate)` from
/// librtlsdr.c (ratio math + demod writes + demod soft reset), but omits
/// tuner bandwidth and frequency-correction bits for now.
///
/// Returns the *actual* sample rate the chip will use.
fn rtl_set_sample_rate(
    handle: &mut DeviceHandle<GlobalContext>,
    samp_rate: u32,
) -> Result<u32, rusb::Error> {
    // These are the same validity limits as librtlsdr:
    // - samp_rate in (225 kHz, 3.2 MHz]
    // - forbid the "gap" (300–900 kHz) that the resampler can't handle.
    if (samp_rate <= 225_000)
        || (samp_rate > 3_200_000)
        || ((samp_rate > 300_000) && (samp_rate <= 900_000))
    {
        eprintln!("Invalid sample rate for RTL2832U: {} Hz", samp_rate);
        // librtlsdr returns -EINVAL; we'll just bail without touching the chip.
        return Ok(0);
    }

    // In librtlsdr, dev->rtl_xtal is usually ~28.8 MHz (and may be corrected).
    // We'll assume 28.8 MHz here; later you could make this configurable.
    let rtl_xtal_hz: u32 = 28_800_000;

    // rsamp_ratio = (rtl_xtal * 2^22) / samp_rate;
    // Use u64 to avoid overflow, then mask to 28 bits like the C code does.
    let two_pow_22: u64 = 1u64 << 22;
    let mut rsamp_ratio: u64 =
        (rtl_xtal_hz as u64 * two_pow_22) / (samp_rate as u64);

    // rsamp_ratio &= 0x0ffffffc;
    rsamp_ratio &= 0x0ffffffc;

    // real_rsamp_ratio = rsamp_ratio | ((rsamp_ratio & 0x08000000) << 1);
    let mut real_rsamp_ratio = rsamp_ratio;
    if (rsamp_ratio & 0x0800_0000) != 0 {
        real_rsamp_ratio |= 0x1000_0000;
    }

    // real_rate = (rtl_xtal * 2^22) / real_rsamp_ratio;
    let real_rate_f64 =
        (rtl_xtal_hz as f64 * two_pow_22 as f64) / (real_rsamp_ratio as f64);
    let real_rate = real_rate_f64 as u32;

    if (samp_rate as f64) != real_rate_f64 {
        println!("Exact sample rate is: {:.3} Hz", real_rate_f64);
    } else {
        println!("Sample rate set exactly: {} Hz", real_rate);
    }

    let rsamp_ratio_u32 = rsamp_ratio as u32;
    let tmp_hi: u16 = (rsamp_ratio_u32 >> 16) as u16;
    let tmp_lo: u16 = (rsamp_ratio_u32 & 0xffff) as u16;

    println!(
        "Programming rsamp_ratio=0x{:08x} (hi=0x{:04x}, lo=0x{:04x})",
        rsamp_ratio_u32, tmp_hi, tmp_lo
    );

    // These writes are exactly:
    //   tmp = (rsamp_ratio >> 16);
    //   rtlsdr_demod_write_reg(dev, 1, 0x9f, tmp, 2);
    //   tmp = rsamp_ratio & 0xffff;
    //   rtlsdr_demod_write_reg(dev, 1, 0xa1, tmp, 2);
    rtl_demod_write_reg(handle, 1, 0x009f, tmp_hi, 2)?;
    rtl_demod_write_reg(handle, 1, 0x00a1, tmp_lo, 2)?;

    // We skip rtlsdr_set_sample_freq_correction(dev, dev->corr) for now.

    // Soft-reset demod (bit 3, soft_rst), same as in librtlsdr:
    //   rtlsdr_demod_write_reg(dev, 1, 0x01, 0x14, 1);
    //   rtlsdr_demod_write_reg(dev, 1, 0x01, 0x10, 1);
    rtl_demod_write_reg(handle, 1, 0x0001, 0x0014, 1)?;
    rtl_demod_write_reg(handle, 1, 0x0001, 0x0010, 1)?;

    Ok(real_rate)
}

/// Program the RTL2832U FIR filter with the default coefficients.
///
/// This mirrors rtlsdr_set_fir(dev) from librtlsdr.c:
/// - First 8 taps: int8_t
/// - Next 8 taps: int12_t, packed into 12 bytes
/// - Total 20 bytes written to demod page=1, addr 0x1c..0x2f
fn rtl_set_fir(handle: &mut DeviceHandle<GlobalContext>) -> Result<(), rusb::Error> {
    println!("Programming RTL2832U FIR coefficients...");

    let mut fir_bytes = [0u8; 20];

    // format: int8_t[8]
    for i in 0..8 {
        let val = FIR_DEFAULT[i];
        // Sanity check like librtlsdr does
        if val < -128 || val > 127 {
            eprintln!("FIR_DEFAULT[{}] out of int8_t range: {}", i, val);
            return Ok(()); // don't crash; just skip if somehow broken
        }
        fir_bytes[i] = val as i8 as u8;
    }

    // format: int12_t[8], packed into 12 bytes
    //
    // fir[8 + i*3/2]     = val0 >> 4;
    // fir[8 + i*3/2 + 1] = (val0 << 4) | ((val1 >> 8) & 0x0f);
    // fir[8 + i*3/2 + 2] = val1;
    //
    for i in (0..8).step_by(2) {
        let val0 = FIR_DEFAULT[8 + i];
        let val1 = FIR_DEFAULT[8 + i + 1];

        if val0 < -2048 || val0 > 2047 || val1 < -2048 || val1 > 2047 {
            eprintln!(
                "FIR_DEFAULT 12-bit taps out of range: i={}, val0={}, val1={}",
                i, val0, val1
            );
            return Ok(());
        }

        let base = 8 + (i * 3 / 2);

        // Do all bit ops in i32 to avoid type mismatches & sign issues,
        // then mask and cast down to u8.
        let v0 = val0 as i32;
        let v1 = val1 as i32;

        let b0 = ((v0 >> 4) & 0xff) as u8;
        let b1 = (((v0 << 4) & 0xf0) | ((v1 >> 8) & 0x0f)) as u8;
        let b2 = (v1 & 0xff) as u8;

        fir_bytes[base] = b0;
        fir_bytes[base + 1] = b1;
        fir_bytes[base + 2] = b2;
    }


    // Write 20 bytes to demod page=1, registers 0x1c .. 0x1c+19
    for (i, &b) in fir_bytes.iter().enumerate() {
        rtl_demod_write_reg(handle, 1, 0x001c + i as u16, b as u16, 1)?;
    }

    Ok(())
}

