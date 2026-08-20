//! HID transport for hosts without `hidraw` — macOS and Windows.
//!
//! The enclosure is a plain HID device, so the only platform-specific part is
//! how the host lets a program exchange reports with it. On Linux that is
//! `/dev/hidraw*` ([`super::hidraw`]); elsewhere it is the OS HID stack —
//! IOHIDManager on macOS, `hid.dll` on Windows — reached here through
//! nu-manager's HID layer rather than a driver of our own, because nu-manager
//! *is* the device layer this project consumes hardware through.
//!
//! **On report size:** the same 63-or-64 ambiguity described in
//! [`super::hidraw`] applies, and it is settled the same way — by reading the
//! device's own report descriptor and computing the size from it with the
//! shared [`super::hidraw::report_size_from_descriptor`] parser. The descriptor
//! is the authority on every host.
//!
//! **On report IDs:** the enclosure declares none, so reports go on the wire
//! with no leading report-ID byte. nu-manager's report device is opened with
//! report id 0, which is exactly the "no report IDs" convention its `hidapi`
//! backing expects, and it strips the byte again on the way back — leaving this
//! transport with the same bytes `hidraw` sees.

use numanager_core::hid::{
    enumerate_hid_devices, HidReportIo, OsHidFeatureConfig, OsHidReportDevice,
};

use super::transport::Transport;
use super::{InstrumentError, Result};

/// How long to wait for a response before giving up, matching the vendor
/// software's own read timeout (and [`super::hidraw`]).
const READ_TIMEOUT_MS: i32 = 1000;
/// Cap on how many stale reports to discard, so a device stuck producing input
/// reports cannot spin the drain forever.
const MAX_DRAIN: usize = 32;
/// Reports the enclosure exchanges are at most this big; used only to size the
/// scratch buffer when the descriptor cannot be read.
const FALLBACK_REPORT_SIZE: usize = 64;

/// An enclosure found on the host HID stack.
#[derive(Debug, Clone)]
pub struct HidDevice {
    pub vendor_id: u16,
    pub product_id: u16,
    /// What the OS calls it, for the connection list.
    pub product_string: Option<String>,
    pub serial_number: Option<String>,
}

/// Every HID device matching `vendor_id`, optionally filtered by product id.
///
/// Mirrors [`super::hidraw::find_devices`] so the worker's enumeration reads
/// the same on every platform.
pub fn find_devices(vendor_id: u16, product_ids: &[u16]) -> Vec<HidDevice> {
    let Ok(devices) = enumerate_hid_devices() else {
        return Vec::new();
    };
    devices
        .into_iter()
        .filter(|device| device.vendor_id == vendor_id)
        .filter(|device| product_ids.is_empty() || product_ids.contains(&device.product_id))
        .map(|device| HidDevice {
            vendor_id: device.vendor_id,
            product_id: device.product_id,
            product_string: device.product_string,
            serial_number: device.serial_number,
        })
        .collect()
}

/// A connection to the enclosure over the host HID stack.
pub struct OsHidTransport {
    device: OsHidReportDevice,
    report_size: usize,
    identity: HidDevice,
}

impl OsHidTransport {
    pub fn open(identity: HidDevice) -> Result<Self> {
        let config = OsHidFeatureConfig {
            vendor_id: identity.vendor_id,
            product_id: identity.product_id,
            // Pinned to the exact unit that was listed: two enclosures on one
            // bench must not silently swap places between listing and opening.
            serial_number: identity.serial_number.clone(),
            read_timeout_ms: READ_TIMEOUT_MS,
        };
        // Report id 0: the enclosure declares no report IDs (see the module
        // note).
        let device = OsHidReportDevice::open_config(config, 0).map_err(|e| {
            InstrumentError::Transport(format!(
                "opening the HID device for {:04x}:{:04x} failed: {e}",
                identity.vendor_id, identity.product_id
            ))
        })?;
        // The descriptor is the authority; a device that will not give one up
        // still works at the conventional size rather than not at all.
        let report_size = device
            .report_descriptor()
            .ok()
            .and_then(|descriptor| super::transport::report_size_from_descriptor(&descriptor))
            .unwrap_or(FALLBACK_REPORT_SIZE);
        Ok(Self {
            device,
            report_size,
            identity,
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
        &self.identity
    }
}

impl Transport for OsHidTransport {
    fn report_size(&self) -> usize {
        self.report_size
    }

    fn write_report(&mut self, report: &[u8]) -> Result<()> {
        self.device
            .write_report(report)
            .map_err(|e| InstrumentError::Transport(format!("write: {e}")))
    }

    fn read_report(&mut self, buf: &mut [u8]) -> Result<usize> {
        let wanted = buf.len().min(self.report_size).max(1);
        let report = self.device.read_report(wanted).map_err(|e| {
            // Nothing arrived in time: the wedged-device case the caller retries
            // or reports, which is a timeout rather than a transport fault.
            if e.code == numanager_core::ErrorCode::Timeout {
                InstrumentError::Timeout
            } else {
                InstrumentError::Transport(format!("read: {e}"))
            }
        })?;
        let n = report.len().min(buf.len());
        buf[..n].copy_from_slice(&report[..n]);
        Ok(n)
    }

    fn drain(&mut self) {
        // Zero timeout: draining an empty queue must cost nothing, or every
        // command would pay the full read timeout before it could be sent.
        self.device.set_read_timeout_ms(0);
        for _ in 0..MAX_DRAIN {
            // The timeout error *is* the exit condition: nothing was queued, so
            // there is nothing left to discard.
            if self.device.read_report(self.report_size.max(1)).is_err() {
                break;
            }
        }
        self.device.set_read_timeout_ms(READ_TIMEOUT_MS);
    }
}
