//! The byte pipe an enclosure talks over.
//!
//! Kept deliberately narrow — write a report, read a report, drop stale ones —
//! so the protocol codec in [`super::geldoc_ez`] is pure logic over bytes and
//! can be tested against the simulated enclosure with no hardware in the loop.

use super::Result;

/// A HID-report-shaped byte pipe.
///
/// Reports are passed *without* a leading report-ID byte: byte 0 of what you
/// write is the opcode, byte 0 of what you read is the status. This matches
/// Linux `hidraw` for a device that declares no report IDs, which is what the
/// Gel Doc EZ does.
pub trait Transport: Send {
    /// Size of one report, in bytes. Read from the device's HID report
    /// descriptor rather than assumed: static analysis of the vendor software
    /// narrows it to 63 or 64 but cannot settle which, and the descriptor says
    /// so directly.
    fn report_size(&self) -> usize;

    /// Send one report. `report` is `report_size()` bytes.
    fn write_report(&mut self, report: &[u8]) -> Result<()>;

    /// Receive one report into `buf`, waiting up to the transport's timeout.
    fn read_report(&mut self, buf: &mut [u8]) -> Result<usize>;

    /// Discard any input reports already queued.
    ///
    /// Called before every write. Without it, a response left over from an
    /// aborted operation is read as the answer to the *next* command and every
    /// subsequent reply is off by one — the vendor software drains the same way,
    /// and it is the difference between a driver that works and one that
    /// mysteriously reports the wrong tray after a cancelled run.
    fn drain(&mut self);
}

// ---- HID report descriptors -------------------------------------------------
//
// Shared by every HID transport rather than owned by one: how big a report is
// is a property of the *device*, stated in its own descriptor, and the answer
// must not differ by which host stack happened to read it.

/// Compute the input report size, in bytes, from a HID report descriptor.
///
/// Walks the short-item stream tracking Report Size (bits) and Report Count, and
/// sums them across the Input main items of the first report. If the descriptor
/// declares a Report ID, one byte is added — a host that prefixes the ID (Linux
/// `hidraw` does) needs the buffer to hold it.
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
        let pos = desc
            .windows(2)
            .position(|w| w == [0x95, 0x3f])
            .expect("count");
        desc[pos + 1] = 0x40;
        assert_eq!(report_size_from_descriptor(&desc), Some(64));
    }

    #[test]
    fn a_descriptor_with_no_input_item_yields_none() {
        // Better to fall back than to believe a zero-length report.
        let desc = vec![0x06, 0x00, 0xff, 0x09, 0x01, 0xa1, 0x01, 0xc0];
        assert_eq!(report_size_from_descriptor(&desc), None);
    }
}
