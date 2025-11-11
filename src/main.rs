// Find the RTL-SDR USB dongle
// Open it
// claim the interface
// find the bulk IN endpoint
// try to read raw bytes continuously.
// optionally write raw bytes to fs.
// Bus 001 Device 003: ID 0bda:2832 Realtek Semiconductor Corp. RTL2832U DVB-T

use rusb::{
    self, devices, Device, DeviceDescriptor, DeviceHandle, Direction, GlobalContext, TransferType, Error, 
};

use std::time::Duration;
use std::fs::File;
use std::io::BufWriter;
use std::io::Write;


mod constants; // Declares the constants module


fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> rusb::Result<()> {
    println!("rust1090_raw: probing for RTL2832 USB device...");
    
    // Optional I/Q output file: if the first CLI argument is present,
    // treat it as a path to write raw IQ samples.
    let iq_out_path = std::env::args().nth(1);
    let mut iq_out: Option<BufWriter<File>> = match iq_out_path {
        Some(path) => {
            println!("I/Q capture enabled. Writing raw samples to: {}", path);
            match File::create(&path) {
                Ok(f) => Some(BufWriter::new(f)),
                Err(e) => {
                    eprintln!("Failed to create I/Q output file '{}': {:?}. Continuing without file output.", path, e);
                    None
                }
            }
        }
        None => {
            println!("No I/Q output file specified. Running in console-only mode.");
            None
        }
    };

    
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
    
    // Configure RF front-end for ADS-B @ 1090 MHz (placeholder for now).
    if let Err(e) = configure_for_adsb_1090mhz(&mut handle) {
        eprintln!(
            "configure_for_adsb_1090mhz failed: {:?}. Continuing with existing tuner settings.",
            e
        );
    }

    // enable internal test mode (8-bit counter stream)
    if let Err(e) = rtl_set_testmode(&mut handle, constants::TEST_MODE) {
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
                
                // If we have an output file, append this buffer.
                if let Some(writer) = iq_out.as_mut() {
                    if let Err(e) = writer.write_all(&buf[..n]) {
                        eprintln!(
                            "Failed to write {} bytes of IQ data to file: {:?}. Disabling file output.",
                            n, e
                        );
                        // Drop the writer so we don't keep spamming errors.
                        iq_out = None;
                    }
                }

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
        
        if vid == constants::RTL_VID && constants::RTL_PIDS.contains(&pid) {
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
    let request_type = constants::CTRL_IN;
    let request = 0u8; // bRequest is "x" in the datasheet; rtl-sdr uses 0.
    let value = addr;
    let index = 0x0100u16; // USB block, GetUSBReg (see vendor command table). :contentReference[oaicite:2]{index=2}

    handle.read_control(
        request_type,
        request,
        value,
        index,
        buf,
        Duration::from_millis(constants::CTRL_TIMEOUT_MS),
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
    let request_type = constants::CTRL_OUT;
    let request = 0u8;
    let value = addr;
    let index = 0x0110u16; // USB block, SetUSBReg. :contentReference[oaicite:3]{index=3}

    handle.write_control(
        request_type,
        request,
        value,
        index,
        data,
        Duration::from_millis(constants::CTRL_TIMEOUT_MS),
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
        constants::CTRL_IN,  // 0xC0: device-to-host, vendor, device
        0,        // bRequest
        value,    // wValue
        index,    // wIndex
        buf,
        Duration::from_millis(constants::CTRL_TIMEOUT_MS),
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
        constants::CTRL_OUT, // 0x40: host-to-device, vendor, device
        0,        // bRequest
        value,    // wValue
        index,    // wIndex
        data,
        Duration::from_millis(constants::CTRL_TIMEOUT_MS),
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
    let n = rtl_get_usb_reg(handle, constants::USB_SYSCTL_0, &mut buf)?;
    if n != 1 {
        println!(
            "Unexpected read length from USB_SYSCTL_0: {} bytes (expected 1)",
            n
        );
    }
    let original = buf[0];
    println!("  Original USB_SYSCTL_0 value: 0x{:02x}", original);

    // 2) Write the same value back (safe "no-op" write)
    let written = rtl_set_usb_reg(handle, constants::USB_SYSCTL_0, &[original])?;
    if written != 1 {
        println!(
            "  Warning: wrote {} bytes back to USB_SYSCTL_0 (expected 1)",
            written
        );
    } else {
        println!("  Wrote USB_SYSCTL_0 back unchanged (0x{:02x})", original);
    }

    // 3) Read again to confirm the transfer path works
    let n2 = rtl_get_usb_reg(handle, constants::USB_SYSCTL_0, &mut buf)?;
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
        constants::DEMOD_PAGE_SYS,
        constants::DEMOD_REG_SYS_TEST,
    );

    let mut buf = [0u8; 1];

    // 1) Read original value
    let n = rtl_get_demod_reg(handle, constants::DEMOD_PAGE_SYS, constants::DEMOD_REG_SYS_TEST, &mut buf)?;
    if n != 1 {
        println!(
            "  Unexpected read length from demod reg page=0x{:02x}, addr=0x{:04x}: {} bytes (expected 1)",
            constants::DEMOD_PAGE_SYS,
            constants::DEMOD_REG_SYS_TEST,
            n
        );
    }
    let original = buf[0];
    println!(
        "  Original DEMOD[page=0x{:02x}, addr=0x{:04x}] value: 0x{:02x}",
        constants::DEMOD_PAGE_SYS,
        constants::DEMOD_REG_SYS_TEST,
        original
    );

    // 2) Write the same value back (no-op)
    let written = rtl_set_demod_reg(
        handle,
        constants::DEMOD_PAGE_SYS,
        constants::DEMOD_REG_SYS_TEST,
        &[original],
    )?;
    if written != 1 {
        println!(
            "  Warning: wrote {} bytes to demod reg page=0x{:02x}, addr=0x{:04x} (expected 1)",
            written,
            constants::DEMOD_PAGE_SYS,
            constants::DEMOD_REG_SYS_TEST,
        );
    } else {
        println!(
            "  Wrote DEMOD[page=0x{:02x}, addr=0x{:04x}] back unchanged (0x{:02x})",
            constants::DEMOD_PAGE_SYS,
            constants::DEMOD_REG_SYS_TEST,
            original
        );
    }

    // 3) Read again
    let n2 = rtl_get_demod_reg(handle, constants::DEMOD_PAGE_SYS, constants::DEMOD_REG_SYS_TEST, &mut buf)?;
    let readback = buf[0];
    println!(
        "  Readback DEMOD[page=0x{:02x}, addr=0x{:04x}] value: 0x{:02x} ({} bytes)",
        constants::DEMOD_PAGE_SYS,
        constants::DEMOD_REG_SYS_TEST,
        readback,
        n2
    );

    Ok(())
}

/// Generic helper to read from USB/SYS/TUN blocks via vendor command table.
/// This complements `rtl_write_reg` and uses:
///   wIndex = (block << 8) | 0x00   (GetUSBReg/GetSysReg/GetTunReg style)
fn rtl_read_reg(
    handle: &mut DeviceHandle<GlobalContext>,
    block: u8,
    addr: u16,
    buf: &mut [u8],
) -> Result<usize, Error> {
    let index: u16 = ((block as u16) << 8) | 0x00;

    handle.read_control(
        constants::CTRL_IN,
        0,          // bRequest
        addr,       // wValue
        index,      // wIndex = block << 8 | 0x00
        buf,
        Duration::from_millis(constants::CTRL_TIMEOUT_MS),
    )
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
        constants::CTRL_OUT,
        0,              // bRequest
        addr,           // wValue = register address (e.g. 0x3000)
        index,          // wIndex = block << 8 | 0x10
        &data[..len as usize],
        Duration::from_millis(constants::CTRL_TIMEOUT_MS),
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
    rtl_write_reg(handle, constants::BLOCK_USBB, constants::USB_SYSCTL_0, 0x0009, 1)?;

    // rtlsdr_write_reg(dev, USBB, USB_EPA_MAXPKT, 0x0002, 2);
    rtl_write_reg(handle, constants::BLOCK_USBB, constants::USB_EPA_MAXPKT, 0x0002, 2)?;

    // rtlsdr_write_reg(dev, USBB, constants::USB_EPA_CTL, 0x1002, 2);
    rtl_write_reg(handle, constants::BLOCK_USBB, constants::USB_EPA_CTL, 0x1002, 2)?;

    // --- power on demod ---
    // rtlsdr_write_reg(dev, SYSB, DEMOD_CTL_1, 0x22, 1);
    rtl_write_reg(handle, constants::BLOCK_SYSB, constants::DEMOD_CTL_1, 0x0022, 1)?;

    // rtlsdr_write_reg(dev, SYSB, DEMOD_CTL, 0xe8, 1);
    rtl_write_reg(handle, constants::BLOCK_SYSB, constants::DEMOD_CTL, 0x00e8, 1)?;

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
    //   rtlsdr_write_reg(dev, USBB, constants::USB_EPA_CTL, 0x1002, 2);
    //   rtlsdr_write_reg(dev, USBB, constants::USB_EPA_CTL, 0x0000, 2);
    rtl_write_reg(handle, constants::BLOCK_USBB, constants::USB_EPA_CTL, 0x1002, 2)?;
    rtl_write_reg(handle, constants::BLOCK_USBB, constants::USB_EPA_CTL, 0x0000, 2)?;

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
        let val = constants::FIR_DEFAULT[i];
        // Sanity check like librtlsdr does
        if val < -128 || val > 127 {
            eprintln!("constants::FIR_DEFAULT[{}] out of int8_t range: {}", i, val);
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
        let val0 = constants::FIR_DEFAULT[8 + i];
        let val1 = constants::FIR_DEFAULT[8 + i + 1];

        if val0 < -2048 || val0 > 2047 || val1 < -2048 || val1 > 2047 {
            eprintln!(
                "constants::FIR_DEFAULT 12-bit taps out of range: i={}, val0={}, val1={}",
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

/// Configure the RF front-end for ADS-B @ 1090 MHz (early steps).
///
/// For now, this:
///   - Enables the I2C repeater (IIC_repeat).
///   - Programs a safe default I2C clock value into SYS_I2CCR.
///   - Probes a few likely tuner I2C addresses and reads reg 0x00,
///     printing out whatever we see (chip-ID-ish).
fn configure_for_adsb_1090mhz(
    handle: &mut DeviceHandle<GlobalContext>,
) -> rusb::Result<()> {
    println!("configure_for_adsb_1090mhz: enabling I2C repeater and probing tuner ID…");

    // 1) Enable I2C repeater so the tuner actually sees I2C traffic.
    if let Err(e) = set_iic_repeat(handle, true) {
        eprintln!("  Failed to enable I2C repeater (IIC_repeat): {:?}", e);
        // We continue anyway; reads will likely fail if this didn't work.
    }

    // 2) Program I2C clock to datasheet default (FD10 = 0x13 => internal ~10 MHz).
    if let Err(e) = rtl_write_reg(
        handle,
        constants::BLOCK_SYSB,
        constants::SYS_I2CCR,
        0x0013,
        1,
    ) {
        eprintln!("  Failed to program SYS I2C clock (SYS_I2CCR): {:?}", e);
    } else {
        println!("  SYS I2C clock (SYS_I2CCR) set to FD10=0x13.");
    }

    // 3) Try a few common tuner I2C addresses and read reg 0x00 as a "chip ID" probe.
    const CANDIDATE_ADDRS: [u8; 4] = [0x34, 0x35, 0x36, 0x38];

    println!("  Probing tuner I2C addresses for reg 0x00…");
    let mut any_success = false;

    for &addr in &CANDIDATE_ADDRS {
        match rtl_read_tuner_reg_byte(handle, addr, 0x00) {
            Ok(val) => {
                any_success = true;
                println!(
                    "    Tuner probe: I2C addr 0x{:02x}, reg 0x00 -> 0x{:02x}",
                    addr, val
                );
            }
            Err(e) => {
                println!(
                    "    Tuner probe: I2C addr 0x{:02x}, reg 0x00 read failed: {:?}",
                    addr, e
                );
            }
        }
    }

    if !any_success {
        eprintln!("  No tuner responded to reg 0x00 probe (yet).");
    } else {
        println!(
            "  Detected tuner at I2C addr 0x{:02x}, reg 0x00 = 0x{:02x} (likely R820T/R828D).",
            0x34, 0x69
        );

        println!(
            "Dumping first 0x20 tuner registers at TUNER_I2C_ADDR=0x{:02x}…",
            constants::TUNER_I2C_ADDR
        );
        if let Err(e) = dump_tuner_registers(handle, constants::TUNER_I2C_ADDR, 0x00, 0x1f) {
            eprintln!(" dump_tuner_registers failed: {:?}", e);
        }
    }
    
    println!("  Tuner probe complete (see values above).");
    Ok(())
}


/// IIC Repeat
/// Why it's used: It ensures that no other device can interrupt the sequence, making combined read/write operations more reliable.
/// Enable or disable the RTL2832U's I2C repeater towards the tuner.
///
/// Datasheet (Table 3, "I2C Repeater Register Table"):
///   - Register Name: IIC_repeat
///   - Page: 1
///   - Offset: 0x01
///   - Bits used: [3]
///   - 1 = Enable repeater (tuner hears I2C traffic)
///   - 0 = Disable repeater (tuner isolated)
fn set_iic_repeat(
    handle: &mut DeviceHandle<GlobalContext>,
    enable: bool,
) -> Result<(), rusb::Error> {
    let page: u8 = 1;
    let addr: u16 = 0x0001; // first byte of page 1

    let mut buf = [0u8; 1];
    let n = rtl_get_demod_reg(handle, page, addr, &mut buf)?;
    if n != 1 {
        eprintln!(
            "set_iic_repeat: unexpected read length {}, expected 1",
            n
        );
    }

    let mut val = buf[0];
    if enable {
        val |= 1 << 3; // set bit 3
    } else {
        val &= !(1 << 3); // clear bit 3
    }

    let written = rtl_set_demod_reg(handle, page, addr, &[val])?;
    if written != 1 {
        eprintln!(
            "set_iic_repeat: wrote {} bytes (expected 1) when updating IIC_repeat",
            written
        );
    }

    println!(
        "I2C repeater (IIC_repeat) {}",
        if enable { "ENABLED" } else { "DISABLED" }
    );

    Ok(())
}


/// Low-level tuner register read via the RTL2832U "GetTunReg" command.
///
/// From the datasheet vendor command table:
///   bmRequestType = 0xC0
///   bRequest      = 0
///   wValue        = (OffsetAdd << 8) + IICAdd
///   wIndex        = 0x0300  (tuner block)
///   wLength       = buf.len()
fn rtl_get_tuner_reg(
    handle: &mut DeviceHandle<GlobalContext>,
    i2c_addr: u8,
    reg: u8,
    buf: &mut [u8],
) -> Result<usize, Error> {
    let request_type = constants::CTRL_IN;
    let request = 0u8;

    // wValue = (OffsetAdd << 8) + IICAdd
    let value: u16 = ((reg as u16) << 8) | (i2c_addr as u16);

    // 0x0300 = GetTunReg (tuner block)
    let index: u16 = 0x0300u16;

    handle.read_control(
        request_type,
        request,
        value,
        index,
        buf,
        Duration::from_millis(constants::CTRL_TIMEOUT_MS),
    )
}


/// Convenience wrapper: read exactly one tuner register byte.
fn rtl_read_tuner_reg_byte(
    handle: &mut DeviceHandle<GlobalContext>,
    i2c_addr: u8,
    reg: u8,
) -> Result<u8, rusb::Error> {
    let mut buf = [0u8; 1];
    let n = rtl_get_tuner_reg(handle, i2c_addr, reg, &mut buf)?;
    if n != 1 {
        eprintln!(
            "rtl_read_tuner_reg_byte: expected 1 byte, got {} (addr=0x{:02x}, reg=0x{:02x})",
            n, i2c_addr, reg
        );
    }
    Ok(buf[0])
}

/// Low-level tuner register *write* via the RTL2832U "SetTunReg" command.
///
/// From the datasheet vendor command table:
/// - bmRequestType = 0x40 (host-to-device, vendor, device)
/// - bRequest      = 0
/// - wValue        = (OffsetAdd << 8) + IICAdd
/// - wIndex        = 0x0310 (tuner block, write)
/// - wLength       = data.len()
fn rtl_set_tuner_reg(
    handle: &mut DeviceHandle<GlobalContext>,
    i2c_addr: u8,
    reg: u8,
    data: &[u8],
) -> rusb::Result<usize> {
    let request_type = constants::CTRL_OUT;
    let request = 0u8;

    // wValue = (OffsetAdd << 8) + IICAdd
    let value: u16 = ((reg as u16) << 8) | (i2c_addr as u16);

    // 0x0310 = SetTunReg (tuner block write)
    let index: u16 = 0x0310u16;

    handle.write_control(
        request_type,
        request,
        value,
        index,
        data,
        Duration::from_millis(constants::CTRL_TIMEOUT_MS),
    )
}

/// Convenience wrapper: write exactly one tuner register byte.
fn rtl_write_tuner_reg_byte(
    handle: &mut DeviceHandle<GlobalContext>,
    i2c_addr: u8,
    reg: u8,
    value: u8,
) -> rusb::Result<()> {
    let written = rtl_set_tuner_reg(handle, i2c_addr, reg, &[value])?;
    if written != 1 {
        eprintln!(
            "rtl_write_tuner_reg_byte: wrote {} bytes (expected 1) \
             to tuner addr=0x{:02x}, reg=0x{:02x}",
            written, i2c_addr, reg
        );
    }
    Ok(())
}

/// Debug helper: dump a range of tuner registers via GetTunReg.
fn dump_tuner_registers(
    handle: &mut DeviceHandle<GlobalContext>,
    i2c_addr: u8,
    first: u8,
    last: u8,
) -> rusb::Result<()> {
    println!(
        "Dumping tuner registers 0x{:02x}..0x{:02x} at I2C addr 0x{:02x}…",
        first, last, i2c_addr
    );

    let mut reg = first;
    loop {
        match rtl_read_tuner_reg_byte(handle, i2c_addr, reg) {
            Ok(val) => {
                println!("  TUNER[0x{:02x}] = 0x{:02x}", reg, val);
            }
            Err(e) => {
                println!(
                    "  TUNER[0x{:02x}] read failed at addr 0x{:02x}: {:?}",
                    reg, i2c_addr, e
                );
            }
        }

        if reg == last {
            break;
        }
        reg = reg.wrapping_add(1);
    }

    Ok(())
}

