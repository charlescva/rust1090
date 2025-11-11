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

/// Dump the 32-bit I2C Master Control register as seen in the DEMOD register map:
/// page = 1, addr = 0x0044..0x0047 (I2CMCR 0044h–0047h in the datasheet).
fn dump_demod_i2cmcr_page1(
    handle: &mut DeviceHandle<GlobalContext>,
    label: &str,
) -> Result<(), Error> {
    let page: u8 = 1;
    let base_addr: u16 = 0x0044;

    let mut bytes = [0u8; 4];
    for i in 0..4 {
        bytes[i] = rtl_read_demod_reg_byte(handle, page, base_addr + i as u16)?;
    }

    let val = u32::from_le_bytes(bytes);
    println!(
        "  DEMOD I2CMCR {} (page=0x{:02x}, addr=0x{:04x}..0x{:04x}) = 0x{:08x}",
        label,
        page,
        base_addr,
        base_addr + 3,
        val
    );
    Ok(())
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

/// Convenience helper: read a single demod register byte using rtl_get_demod_reg.
fn rtl_read_demod_reg_byte(
    handle: &mut DeviceHandle<GlobalContext>,
    page: u8,
    addr: u16,
) -> Result<u8, Error> {
    let mut buf = [0u8; 1];
    let n = rtl_get_demod_reg(handle, page, addr, &mut buf)?;
    if n != 1 {
        eprintln!(
            "rtl_read_demod_reg_byte: expected 1 byte from demod page=0x{:02x}, addr=0x{:04x}, got {}",
            page, addr, n
        );
    }
    Ok(buf[0])
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
    match set_sys_i2c_clock_10mhz(handle) {
        Ok(()) => {
            let fd10 = read_sys_i2c_clock_fd10(handle).unwrap_or(0xff);
            println!(
                "  SYS I2C clock (SYS_I2CCR) FD10 set to 0x{:02x} (datasheet 10MHz recommendation).",
                fd10
            );
        }
        Err(e) => {
            eprintln!("  Failed to program SYS I2C clock (SYS_I2CCR): {:?}", e);
        }
    }

    // 2b) Initialize I2C Master Control for simple single-shot transfers.
    if let Err(e) = dump_demod_i2cmcr_page1(handle, "BEFORE init") {
    eprintln!("  dump_demod_i2cmcr_page1 (before) failed: {:?}", e);
    }

    if let Err(e) = init_sys_i2c_master_simple(handle) {
        eprintln!("  init_sys_i2c_master_simple failed: {:?}", e);
    } else {
        println!("  I2C Master Control (I2CMCR) initialized for simple single-shot mode.");
    }

    if let Err(e) = dump_demod_i2cmcr_page1(handle, "AFTER init") {
        eprintln!("  dump_demod_i2cmcr_page1 (after) failed: {:?}", e);
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
        

        // Quick write/readback test on some register. We'll use 0x05 arbitrarily;
        // if it's read-only, the write is ignored, which is still harmless.
        if let Err(e) = test_tuner_reg_roundtrip(handle, constants::TUNER_I2C_ADDR, 0x05) {
            eprintln!("  test_tuner_reg_roundtrip failed: {:?}", e);
        }
        
        // New: test I2C master-based write to the tuner.
        if let Err(e) = test_tuner_write_via_i2c_master(handle, 0x05) {
            eprintln!("  test_tuner_write_via_i2c_master failed: {:?}", e);
        }

        // NEW: test the IICB-based tuner I2C path
        if let Err(e) = test_tuner_i2c_block_path(handle, constants::TUNER_I2C_ADDR) {
            eprintln!("  test_tuner_i2c_block_path failed: {:?}", e);
        }
        
        // Try to apply 1090 MHz profile, if any.
        if let Err(e) = tuner_set_1090mhz(handle) {
            eprintln!("  tuner_set_1090mhz failed: {:?}", e);
        }

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
/// Test that we can write and then read back a tuner register.
///
/// This does:
///   1) Read current value at (i2c_addr, reg)
///   2) Write the same value back
///   3) Read again and print both
fn test_tuner_reg_roundtrip(
    handle: &mut DeviceHandle<GlobalContext>,
    i2c_addr: u8,
    reg: u8,
) -> rusb::Result<()> {
    println!(
        "Testing tuner register roundtrip at addr=0x{:02x}, reg=0x{:02x}…",
        i2c_addr, reg
    );

    let v1 = rtl_read_tuner_reg_byte(handle, i2c_addr, reg)?;
    println!("  Initial TUNER[0x{:02x}] = 0x{:02x}", reg, v1);

    rtl_write_tuner_reg_byte(handle, i2c_addr, reg, v1)?;
    println!("  Wrote same value back to TUNER[0x{:02x}] (0x{:02x})", reg, v1);

    let v2 = rtl_read_tuner_reg_byte(handle, i2c_addr, reg)?;
    println!("  After writeback, TUNER[0x{:02x}] = 0x{:02x}", reg, v2);

    if v1 == v2 {
        println!("  Roundtrip OK (v1 == v2).");
    } else {
        println!("  Roundtrip MISMATCH (v1=0x{:02x}, v2=0x{:02x})", v1, v2);
    }

    Ok(())
}
/// Program the R82xx tuner to (approximately) 1090 MHz using a known-good profile.
///
/// This function assumes:
///   - TUNER_I2C_ADDR is the base tuner I2C address (0x34).
///   - R82XX_1090MHZ_PROFILE contains a list of (reg, value) pairs that set
///     the PLL, band, and filters appropriately for 1090 MHz.
fn tuner_set_1090mhz(
    handle: &mut DeviceHandle<GlobalContext>,
) -> rusb::Result<()> {
    if constants::R82XX_1090MHZ_PROFILE.is_empty() {
        println!(
            "tuner_set_1090mhz: R82XX_1090MHZ_PROFILE is empty; \
             no tuner registers being programmed yet."
        );
        return Ok(());
    }

    println!(
        "tuner_set_1090mhz: programming {} tuner register(s) at I2C addr 0x{:02x}…",
        constants::R82XX_1090MHZ_PROFILE.len(),
        constants::TUNER_I2C_ADDR
    );

    for &(reg, val) in constants::R82XX_1090MHZ_PROFILE {
        println!(
            "  TUNER[0x{:02x}] := 0x{:02x}",
            reg, val
        );
        rtl_write_tuner_reg_byte(handle, constants::TUNER_I2C_ADDR, reg, val)?;
    }

    println!("tuner_set_1090mhz: done programming tuner registers.");
    Ok(())
}

/// Write a single R82xx tuner register using the generic I2C master.
///
/// This sends:
///   [TUNER_I2C_ADDR (write), reg, value]
fn tuner_write_reg_i2c_master(
    handle: &mut DeviceHandle<GlobalContext>,
    reg: u8,
    value: u8,
) -> Result<(), Error> {
    // data bytes after address: [reg, value]
    let data = [reg, value];
    rtl_i2c_write(handle, constants::TUNER_I2C_ADDR, &data)
}


/// Read the low byte of SYS_I2CCR (I2C Clock Register).
fn read_sys_i2c_clock_fd10(
    handle: &mut DeviceHandle<GlobalContext>,
) -> Result<u8, rusb::Error> {
    let mut buf = [0u8; 1];
    let n = rtl_read_reg(handle, constants::BLOCK_SYSB, constants::SYS_I2CCR, &mut buf)?;
    if n != 1 {
        eprintln!(
            "read_sys_i2c_clock_fd10: expected 1 byte, got {}",
            n
        );
    }
    Ok(buf[0] & constants::SYS_I2CCR_FD10_MASK)
}

/// Set FD10 in SYS_I2CCR to a specific value (low 6 bits).
/// This does a read-modify-write of the low byte, preserving reserved bits.
fn write_sys_i2c_clock_fd10(
    handle: &mut DeviceHandle<GlobalContext>,
    fd10: u8,
) -> Result<(), rusb::Error> {
    let fd10 = fd10 & constants::SYS_I2CCR_FD10_MASK;
    if fd10 == 0 {
        eprintln!("write_sys_i2c_clock_fd10: fd10=0 is forbidden; using 1 instead.");
    }

    // Read current low byte.
    let mut buf = [0u8; 1];
    let n = rtl_read_reg(handle, constants::BLOCK_SYSB, constants::SYS_I2CCR, &mut buf)?;
    if n != 1 {
        eprintln!(
            "write_sys_i2c_clock_fd10: expected 1 byte, got {}",
            n
        );
    }

    // Preserve upper bits of the byte, replace bits [5:0].
    let current = buf[0];
    let new_val = (current & !constants::SYS_I2CCR_FD10_MASK) | fd10;

    // Write back one byte (low byte of the register).
    rtl_write_reg(
        handle,
        constants::BLOCK_SYSB,
        constants::SYS_I2CCR,
        new_val as u16,
        1,
    )?;

    Ok(())
}



/// Convenience helper: set SYS I2C clock to the datasheet's 10 MHz recommendation.
fn set_sys_i2c_clock_10mhz(
    handle: &mut DeviceHandle<GlobalContext>,
) -> Result<(), rusb::Error> {
    write_sys_i2c_clock_fd10(handle, constants::SYS_I2CCR_FD10_10MHZ)
}

/// Configure I2C Master Control (I2CMCR) for a simple single-shot transaction model:
/// - No repeat starts (RSC = 0).
/// - Default time-out enabled (TORE = 1, TOR = 0x3A).
/// - ACK checking enabled for first/second bytes (FBAIFD = SBAIFD = 0).
/// - Interrupts enabled (TEIE/MRCIE/MTCIE = 1) so status bits update; we'll still poll.
/// Note: We do *not* set CS or IMUR here; those are for kick-off/reset.
fn init_sys_i2c_master_simple(
    handle: &mut DeviceHandle<GlobalContext>,
) -> rusb::Result<()> {
    let mut val: u32 = 0;

    // Enable time-out logic.
    val |= constants::I2CMCR_TORE;
    // TOR default (0x3A)
    val |= constants::I2CMCR_TOR_DEFAULT;

    // Enable interrupt flags so I2CMSR bits actually update.
    val |= constants::I2CMCR_TEIE | constants::I2CMCR_MRCIE | constants::I2CMCR_MTCIE;

    // All other bits default to 0:
    // - IMUR=0 (no reset)
    // - CS=0 (not started)
    // - RWL=0 (we'll set per-transaction)
    // - RSC=0, FRSIB/SRSIB=0 (no repeat starts)
    // - FBAIFD/SBAIFD=0 (check ACK)
    // - TEIE/MRCIE/MTCIE now enabled above.

    let bytes = val.to_le_bytes();

    // Low 16 bits
    let low = u16::from(bytes[0]) | (u16::from(bytes[1]) << 8);
    rtl_write_reg(
        handle,
        constants::BLOCK_SYSB,
        constants::SYS_I2CMCR,
        low,
        2,
    )?;

    // High 16 bits
    let high = u16::from(bytes[2]) | (u16::from(bytes[3]) << 8);
    rtl_write_reg(
        handle,
        constants::BLOCK_SYSB,
        constants::SYS_I2CMCR.wrapping_add(2),
        high,
        2,
    )?;

    Ok(())
}



/// Read the low byte of SYS I2C Master Status (I2CMSR).
/// Only bits [2:0] are currently defined (TEIF, MRCIF, MTCIF).
fn read_sys_i2c_status(
    handle: &mut DeviceHandle<GlobalContext>,
) -> Result<u8, Error> {
    let mut buf = [0u8; 1];
    let n = rtl_read_reg(handle, constants::BLOCK_SYSB, constants::SYS_I2CMSR, &mut buf)?;
    if n != 1 {
        eprintln!(
            "read_sys_i2c_status: expected 1 byte, got {}",
            n
        );
    }
    Ok(buf[0])
}

/// Clear selected I2C status flags in I2CMSR by writing '1' to those bits (W1C).
///
/// mask: combination of I2CMSR_TEIF, I2CMSR_MRCIF, I2CMSR_MTCIF.
/// Only bits set to '1' in mask will be cleared (write-one-to-clear semantics).
fn clear_sys_i2c_status_flags(
    handle: &mut DeviceHandle<GlobalContext>,
    mask: u8,
) -> Result<(), Error> {
    // We only ever touch bits [2:0]; higher bits are reserved.
    let to_clear = mask & constants::I2CMSR_ALL_FLAGS;

    if to_clear == 0 {
        // Nothing to do.
        return Ok(());
    }

    // W1C semantics: writing '1' to a bit clears it, '0' leaves it unchanged.
    rtl_write_reg(
        handle,
        constants::BLOCK_SYSB,
        constants::SYS_I2CMSR,
        to_clear as u16,
        1,
    )?;

    Ok(())
}

/// Write one data byte into the SYS I2C Master FIFO/Data register (I2CMFR.TDD).
/// This is used to queue bytes for transmission (slave address, register index, data...).
fn sys_i2c_write_data_byte(
    handle: &mut DeviceHandle<GlobalContext>,
    byte: u8,
) -> Result<(), Error> {
    rtl_write_reg(
        handle,
        constants::BLOCK_SYSB,
        constants::SYS_I2CMFR,
        byte as u16,
        1,
    )?;
    Ok(())
}

/// Read one data byte from the SYS I2C Master FIFO/Data register (I2CMFR.TDD).
/// After a receive transaction, this returns one byte from the target device.
fn sys_i2c_read_data_byte(
    handle: &mut DeviceHandle<GlobalContext>,
) -> Result<u8, Error> {
    let mut buf = [0u8; 1];
    let n = rtl_read_reg(
        handle,
        constants::BLOCK_SYSB,
        constants::SYS_I2CMFR,
        &mut buf,
    )?;
    if n != 1 {
        eprintln!(
            "sys_i2c_read_data_byte: expected 1 byte, got {}",
            n
        );
    }
    Ok(buf[0] & constants::I2CMFR_TDD_MASK)
}

/// Perform a simple I2C write transaction using the RTL2832U's I2C master.
///
/// This sends:
///   [addr_8bit, data[0], data[1], ...]
///
/// - `addr_8bit` is the full 8-bit I2C address (7-bit address << 1 | R/Wbit),
///   e.g. 0x34 for an R820T tuner write.
/// - `data` are the bytes following the address on the bus (typically register
///   index + one or more data bytes).
///
/// Constraints:
///   - data.len() must be in [1, 24] (RWL encoding 0..17 => 1..24 bytes).
///   - IIC_repeat must already be enabled so the tuner sees the traffic.
///   - I2C clock and master control should already be initialized.
fn rtl_i2c_write(
    handle: &mut DeviceHandle<GlobalContext>,
    addr_8bit: u8,
    data: &[u8],
) -> Result<(), Error> {
    if data.is_empty() {
        eprintln!("rtl_i2c_write: data buffer is empty; nothing to send.");
        return Ok(());
    }
    if data.len() > 24 {
        eprintln!(
            "rtl_i2c_write: data length {} too large (max 24 bytes).",
            data.len()
        );
        return Err(Error::Other);
    }

    // 1) Clear any old status flags (MTCIF/MRCIF/TEIF).
    clear_sys_i2c_status_flags(handle, constants::I2CMSR_ALL_FLAGS)?;

    // 2) Write the address byte, then all data bytes, into the FIFO.
    sys_i2c_write_data_byte(handle, addr_8bit)?;
    for &b in data {
        sys_i2c_write_data_byte(handle, b)?;
    }

    // 3) Program RWL = data.len() - 1 (does NOT include address byte).
    //
    //    From your I2CMCR table:
    //      - RWL encodes data length: 0 => 1 byte ... 17 => 24 bytes.
    //      - "Does not include the slave address byte in the FIFO register."
    //
    //    So for N data bytes we set RWL = N-1.
    let rwl_val: u32 = (data.len() as u32 - 1) & 0x1F;

    let mut i2cmcr_bytes = [0u8; 4];
    let n = rtl_read_reg(
        handle,
        constants::BLOCK_SYSB,
        constants::SYS_I2CMCR,
        &mut i2cmcr_bytes,
    )?;
    if n != 4 {
        eprintln!(
            "rtl_i2c_write: expected to read 4 bytes from I2CMCR, got {}",
            n
        );
    }
    let mut i2cmcr_val = u32::from_le_bytes(i2cmcr_bytes);

    println!(
        "rtl_i2c_write: I2CMCR before RWL/CS update = 0x{:08x}",
        i2cmcr_val
    );

    // Clear old RWL and insert new.
    i2cmcr_val &= !constants::I2CMCR_RWL_MASK;
    i2cmcr_val |= (rwl_val << constants::I2CMCR_RWL_SHIFT) & constants::I2CMCR_RWL_MASK;

    // 4) Set CS=1 (Command Start) to kick off the transaction.
    i2cmcr_val |= constants::I2CMCR_CS;

    println!(
        "rtl_i2c_write: I2CMCR after RWL/CS update = 0x{:08x}",
        i2cmcr_val
    );

    let new_bytes = i2cmcr_val.to_le_bytes();

    // Write back full 32-bit I2CMCR as two 16-bit words.
    let low = u16::from(new_bytes[0]) | (u16::from(new_bytes[1]) << 8);
    rtl_write_reg(
        handle,
        constants::BLOCK_SYSB,
        constants::SYS_I2CMCR,
        low,
        2,
    )?;
    let high = u16::from(new_bytes[2]) | (u16::from(new_bytes[3]) << 8);
    rtl_write_reg(
        handle,
        constants::BLOCK_SYSB,
        constants::SYS_I2CMCR.wrapping_add(2),
        high,
        2,
    )?;

    // 5) Poll I2CMSR for completion or error.
    const MAX_POLLS: usize = 1000;
    for i in 0..MAX_POLLS {
        let status = read_sys_i2c_status(handle)?;

        if status != 0 {
            println!(
                "rtl_i2c_write: poll {}: I2CMSR = 0x{:02x}",
                i, status
            );
        }

        let has_error = (status & constants::I2CMSR_TEIF) != 0;
        let tx_done  = (status & constants::I2CMSR_MTCIF) != 0;
        let rx_done  = (status & constants::I2CMSR_MRCIF) != 0;

        if has_error {
            eprintln!(
                "rtl_i2c_write: I2C Transaction Error (I2CMSR=0x{:02x})",
                status
            );
            clear_sys_i2c_status_flags(handle, constants::I2CMSR_ALL_FLAGS)?;
            return Err(Error::Other);
        }

        if tx_done || rx_done {
            // Clear completion flag(s) and return success.
            clear_sys_i2c_status_flags(
                handle,
                constants::I2CMSR_MTCIF | constants::I2CMSR_MRCIF,
            )?;
            println!(
                "rtl_i2c_write: transaction complete (status=0x{:02x}, tx_done={}, rx_done={})",
                status, tx_done, rx_done
            );
            return Ok(());
        }
    }

    eprintln!("rtl_i2c_write: timed out waiting for I2C transaction to complete.");
    // Best-effort clean-up.
    clear_sys_i2c_status_flags(handle, constants::I2CMSR_ALL_FLAGS)?;
    Err(Error::Timeout)
}

/// Test writing a tuner register via the generic I2C master.
/// We:
///   1) Read the current value using the old vendor GetTunReg path.
///   2) Write the same value back using the generic I2C master.
///   3) Read again (vendor path) and print both values.
///
/// This won’t reveal the true tuner register map yet (GetTunReg is limited),
/// but it will confirm that the I2C master transaction completes without TEIF.
fn test_tuner_write_via_i2c_master(
    handle: &mut DeviceHandle<GlobalContext>,
    reg: u8,
) -> Result<(), Error> {
    println!(
        "Testing tuner write via I2C master at addr=0x{:02x}, reg=0x{:02x}…",
        constants::TUNER_I2C_ADDR,
        reg
    );

    // 1) Read current value via old vendor helper.
    let v1 = rtl_read_tuner_reg_byte(handle, constants::TUNER_I2C_ADDR, reg)?;
    println!("  (vendor) initial TUNER[0x{:02x}] = 0x{:02x}", reg, v1);

    // 2) Write same value back via I2C master.
    tuner_write_reg_i2c_master(handle, reg, v1)?;
    println!(
        "  (i2c_master) wrote same value back to TUNER[0x{:02x}] (0x{:02x})",
        reg, v1
    );

    // 3) Read again via vendor helper.
    let v2 = rtl_read_tuner_reg_byte(handle, constants::TUNER_I2C_ADDR, reg)?;
    println!("  (vendor) after I2C master write, TUNER[0x{:02x}] = 0x{:02x}", reg, v2);

    if v1 == v2 {
        println!("  Tuner I2C master write test: values match (v1 == v2).");
    } else {
        println!(
            "  Tuner I2C master write test: MISMATCH (v1=0x{:02x}, v2=0x{:02x}).",
            v1, v2
        );
    }

    Ok(())
}

fn rtl_read_array(
    handle: &mut DeviceHandle<GlobalContext>,
    block: u8,
    addr: u16,
    buf: &mut [u8],
) -> Result<usize, Error> {
    let index: u16 = (block as u16) << 8;

    handle.read_control(
        constants::CTRL_IN,
        0,          // bRequest
        addr,       // wValue = I2C slave addr for IICB
        index,      // wIndex = (block << 8)
        buf,
        Duration::from_millis(constants::CTRL_TIMEOUT_MS),
    )
}

fn rtl_write_array(
    handle: &mut DeviceHandle<GlobalContext>,
    block: u8,
    addr: u16,
    data: &[u8],
) -> Result<usize, Error> {
    let index: u16 = ((block as u16) << 8) | 0x10;

    handle.write_control(
        constants::CTRL_OUT,
        0,          // bRequest
        addr,       // wValue = I2C slave addr for IICB
        index,      // wIndex = (block << 8) | 0x10
        data,
        Duration::from_millis(constants::CTRL_TIMEOUT_MS),
    )
}


/// Low-level tuner I2C write via IICB block (matches rtlsdr_i2c_write_reg).
fn rtl_tuner_i2c_write_reg_iicb(
    handle: &mut DeviceHandle<GlobalContext>,
    i2c_addr: u8,
    reg: u8,
    val: u8,
) -> Result<(), Error> {
    let addr = i2c_addr as u16;
    let data = [reg, val];
    let written = rtl_write_array(handle, constants::BLOCK_IICB, addr, &data)?;
    if written != data.len() {
        eprintln!(
            "rtl_tuner_i2c_write_reg_iicb: wrote {} bytes (expected {})",
            written,
            data.len()
        );
    }
    Ok(())
}

/// Low-level tuner I2C read via IICB block (matches rtlsdr_i2c_read_reg).
fn rtl_tuner_i2c_read_reg_iicb(
    handle: &mut DeviceHandle<GlobalContext>,
    i2c_addr: u8,
    reg: u8,
) -> Result<u8, Error> {
    let addr = i2c_addr as u16;

    // First write the register index
    let reg_buf = [reg];
    let written = rtl_write_array(handle, constants::BLOCK_IICB, addr, &reg_buf)?;
    if written != reg_buf.len() {
        eprintln!(
            "rtl_tuner_i2c_read_reg_iicb: wrote {} bytes of reg index (expected {})",
            written,
            reg_buf.len()
        );
    }

    // Now read one byte back
    let mut data = [0u8; 1];
    let read = rtl_read_array(handle, constants::BLOCK_IICB, addr, &mut data)?;
    if read != 1 {
        eprintln!(
            "rtl_tuner_i2c_read_reg_iicb: read {} bytes (expected 1)",
            read
        );
    }

    Ok(data[0])
}
fn test_tuner_i2c_block_path(
    handle: &mut DeviceHandle<GlobalContext>,
    tuner_addr: u8,
) -> Result<(), Error> {
    println!(
        "Testing tuner I2C via IICB block at addr=0x{:02x}, reg=0x00…",
        tuner_addr
    );

    // Read reg 0x00 via new path
    let v0 = rtl_tuner_i2c_read_reg_iicb(handle, tuner_addr, 0x00)?;
    println!(
        "  (IICB) TUNER[0x00] = 0x{:02x} (this should be 0x69 for R820T/R828D)",
        v0
    );

    // Maybe also compare with your existing vendor-based `rtl_get_tuner_reg`
    let mut buf = [0u8; 1];
    let n = rtl_get_tuner_reg(handle, tuner_addr, 0x00, &mut buf)?;
    if n == 1 {
        println!(
            "  (vendor) TUNER[0x00] = 0x{:02x} (via GetTunReg)",
            buf[0]
        );
    } else {
        println!(
            "  (vendor) rtl_get_tuner_reg read {} bytes for reg 0x00",
            n
        );
    }

    Ok(())
}

