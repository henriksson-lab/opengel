//! Linux `hidraw` transport.
//!
//! The enclosure is a plain HID device, so no kernel driver and no libusb are
//! needed — open `/dev/hidraw*` and exchange fixed-size reports. Devices are
//! found by walking `/sys/class/hidraw`, which also gives us the report
//! descriptor, and with it the report size.
//!
//! **On report size:** static analysis of the vendor software narrows the
//! on-wire report to 63 or 64 bytes but cannot decide which, because Windows
//! reports HID report lengths as *max size + 1* whether or not the device uses
//! report IDs. Rather than hard-code a guess that would silently truncate or
//! over-read, this reads the device's own report descriptor and computes the
//! size from it. The descriptor is the authority, and it is right there in
//! sysfs.
//!
//! Access needs a udev rule for vendor `0614` (see `packaging/`); without one
//! the device nodes are root-only and opening fails with a permission error,
//! which this reports as such rather than as "not found".

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::transport::Transport;
use super::{InstrumentError, Result};

/// `O_NONBLOCK` on Linux. Opening non-blocking is what lets a read time out (by
/// polling) instead of hanging the worker thread forever on a wedged device.
const O_NONBLOCK: i32 = 0o4000;

/// How long to wait for a response before giving up, matching the vendor
/// software's own read timeout.
const READ_TIMEOUT: Duration = Duration::from_millis(1000);
/// Gap between read attempts while waiting.
const READ_POLL: Duration = Duration::from_millis(1);
/// Cap on how many stale reports to discard, so a device stuck producing input
/// reports cannot spin the drain forever.
const MAX_DRAIN: usize = 32;

/// A HID device node found in sysfs.
#[derive(Debug, Clone)]
pub struct HidDevice {
    pub path: PathBuf,
    pub vendor_id: u16,
    pub product_id: u16,
    /// Report size in bytes, from the report descriptor.
    pub report_size: usize,
}

/// Every hidraw node matching `vendor_id`, optionally filtered by product id.
pub fn find_devices(vendor_id: u16, product_ids: &[u16]) -> Vec<HidDevice> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/class/hidraw") else {
        return found;
    };
    for entry in entries.flatten() {
        let sysfs = entry.path();
        let Some(name) = sysfs.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some((vid, pid)) = read_hid_id(&sysfs) else {
            continue;
        };
        if vid != vendor_id || (!product_ids.is_empty() && !product_ids.contains(&pid)) {
            continue;
        }
        let report_size = std::fs::read(sysfs.join("device/report_descriptor"))
            .ok()
            .and_then(|desc| report_size_from_descriptor(&desc))
            .unwrap_or(64);
        found.push(HidDevice {
            path: PathBuf::from("/dev").join(name),
            vendor_id: vid,
            product_id: pid,
            report_size,
        });
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

/// Parse `HID_ID=0003:00000614:00000467` out of a hidraw device's uevent.
fn read_hid_id(sysfs: &Path) -> Option<(u16, u16)> {
    let uevent = std::fs::read_to_string(sysfs.join("device/uevent")).ok()?;
    let line = uevent.lines().find_map(|l| l.strip_prefix("HID_ID="))?;
    let mut parts = line.split(':');
    let _bus = parts.next()?;
    let vid = u32::from_str_radix(parts.next()?.trim(), 16).ok()?;
    let pid = u32::from_str_radix(parts.next()?.trim(), 16).ok()?;
    Some((vid as u16, pid as u16))
}

/// Compute the input report size, in bytes, from a HID report descriptor.
///
/// Walks the short-item stream tracking Report Size (bits) and Report Count, and
/// sums them across the Input main items of the first report. If the descriptor
/// declares a Report ID, one byte is added — `hidraw` prefixes the ID in that
/// case, and the buffer has to hold it.
///
/// Returns `None` for a descriptor with no Input item, so the caller can fall
/// back rather than believe a zero.
pub fn report_size_from_descriptor(desc: &[u8]) -> Option<usize> {
    let mut report_size_bits = 0usize;
    let mut report_count = 0usize;
    let mut has_report_id = false;
    let mut input_bits = 0usize;
    let mut saw_input = false;

    let mut i = 0;
    while i < desc.len() {
        let prefix = desc[i];
        // Long items (0xfe) carry their own length byte; none of the devices
        // here use them, but skipping them correctly keeps the walk in step.
        if prefix == 0xfe {
            let len = *desc.get(i + 1)? as usize;
            i += 3 + len;
            continue;
        }
        let size = match prefix & 0x03 {
            3 => 4,
            n => n as usize,
        };
        let tag = prefix & 0xfc;
        let data = desc.get(i + 1..i + 1 + size)?;
        let value = data
            .iter()
            .enumerate()
            .fold(0u32, |acc, (n, b)| acc | ((*b as u32) << (8 * n)));

        match tag {
            // Global: Report Size (bits per field).
            0x74 => report_size_bits = value as usize,
            // Global: Report Count (number of fields).
            0x94 => report_count = value as usize,
            // Global: Report ID.
            0x84 => has_report_id = true,
            // Main: Input.
            0x80 => {
                input_bits += report_size_bits * report_count;
                saw_input = true;
            }
            _ => {}
        }
        i += 1 + size;
    }

    if !saw_input {
        return None;
    }
    let bytes = input_bits.div_ceil(8);
    Some(bytes + usize::from(has_report_id))
}

/// A hidraw connection.
pub struct HidRawTransport {
    file: File,
    report_size: usize,
    device: HidDevice,
}

impl HidRawTransport {
    /// Open a specific device node.
    pub fn open(device: HidDevice) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(O_NONBLOCK)
            .open(&device.path)
            .map_err(|e| match e.kind() {
                ErrorKind::PermissionDenied => InstrumentError::Transport(format!(
                    "{} is not accessible ({e}). Install the udev rule for USB vendor \
                     {:04x} so your user may open it.",
                    device.path.display(),
                    device.vendor_id
                )),
                _ => InstrumentError::Transport(format!("opening {}: {e}", device.path.display())),
            })?;
        Ok(Self {
            report_size: device.report_size,
            file,
            device,
        })
    }

    /// Open the first device matching `vendor_id` and one of `product_ids`.
    pub fn open_first(vendor_id: u16, product_ids: &[u16]) -> Result<Self> {
        let device = find_devices(vendor_id, product_ids)
            .into_iter()
            .next()
            .ok_or(InstrumentError::NotFound)?;
        Self::open(device)
    }

    pub fn device(&self) -> &HidDevice {
        &self.device
    }
}

impl Transport for HidRawTransport {
    fn report_size(&self) -> usize {
        self.report_size
    }

    fn write_report(&mut self, report: &[u8]) -> Result<()> {
        self.file
            .write_all(report)
            .map_err(|e| InstrumentError::Transport(format!("write: {e}")))
    }

    fn read_report(&mut self, buf: &mut [u8]) -> Result<usize> {
        let deadline = Instant::now() + READ_TIMEOUT;
        loop {
            match self.file.read(buf) {
                Ok(n) => return Ok(n),
                // Non-blocking with nothing queued yet: wait and retry until the
                // deadline, so a slow answer still arrives but a dead device
                // does not hang the worker.
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(InstrumentError::Timeout);
                    }
                    std::thread::sleep(READ_POLL);
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(InstrumentError::Transport(format!("read: {e}"))),
            }
        }
    }

    fn drain(&mut self) {
        let mut scratch = vec![0u8; self.report_size.max(1)];
        for _ in 0..MAX_DRAIN {
            match self.file.read(&mut scratch) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                // WouldBlock is the normal exit: nothing left queued.
                Err(_) => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A descriptor for a vendor-defined device with 63 one-byte input and
    /// output fields and no report ID — the "63 bytes on the wire" reading.
    fn descriptor_63_no_report_id() -> Vec<u8> {
        vec![
            0x06, 0x00, 0xff, // Usage Page (vendor-defined)
            0x09, 0x01, // Usage
            0xa1, 0x01, // Collection (Application)
            0x09, 0x02, //   Usage
            0x15, 0x00, //   Logical Minimum (0)
            0x26, 0xff, 0x00, //   Logical Maximum (255)
            0x75, 0x08, //   Report Size (8 bits)
            0x95, 0x3f, //   Report Count (63)
            0x81, 0x02, //   Input (Data, Var, Abs)
            0x09, 0x03, //   Usage
            0x95, 0x3f, //   Report Count (63)
            0x91, 0x02, //   Output
            0xc0, // End Collection
        ]
    }

    #[test]
    fn report_size_comes_from_the_descriptor() {
        assert_eq!(
            report_size_from_descriptor(&descriptor_63_no_report_id()),
            Some(63)
        );
    }

    #[test]
    fn a_declared_report_id_adds_its_byte() {
        // hidraw prefixes the report ID when the device declares one, so the
        // buffer must be one byte longer. Getting this wrong truncates every
        // reply by a byte.
        let mut desc = descriptor_63_no_report_id();
        // Insert Report ID (1) just inside the collection.
        desc.splice(7..7, [0x85, 0x01]);
        assert_eq!(report_size_from_descriptor(&desc), Some(64));
    }

    #[test]
    fn a_64_byte_descriptor_reads_as_64() {
        let mut desc = descriptor_63_no_report_id();
        // Report Count 63 -> 64 for the input item.
        let pos = desc.windows(2).position(|w| w == [0x95, 0x3f]).expect("count");
        desc[pos + 1] = 0x40;
        assert_eq!(report_size_from_descriptor(&desc), Some(64));
    }

    #[test]
    fn a_descriptor_with_no_input_item_yields_none() {
        // Better to fall back than to believe a zero-length report.
        let desc = vec![0x06, 0x00, 0xff, 0x09, 0x01, 0xa1, 0x01, 0xc0];
        assert_eq!(report_size_from_descriptor(&desc), None);
    }

    #[test]
    fn hid_id_parsing_reads_vendor_and_product() {
        let dir = std::env::temp_dir().join("opengel-hidraw-test/device");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("uevent"), "DRIVER=hid-generic\nHID_ID=0003:00000614:00000467\n")
            .expect("write");
        let parsed = read_hid_id(dir.parent().expect("parent"));
        std::fs::remove_dir_all(dir.parent().expect("parent")).ok();
        assert_eq!(parsed, Some((0x0614, 0x0467)));
    }
}
