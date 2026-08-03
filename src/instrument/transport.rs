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
