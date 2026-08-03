//! Imaging *enclosures* — the darkroom around the camera.
//!
//! An enclosure is not a camera. It holds the sample tray, senses which tray is
//! in and whether the door is closed, drives the illumination, reports faults
//! and lights the front-panel LEDs. The camera inside it is a separate USB
//! device driven through [`crate::camera`], so the two are modelled separately
//! and paired by the application.
//!
//! * [`geldoc_ez`] — the Bio-Rad Gel Doc EZ (and Criterion Stain Free, which
//!   shares its command set): 14 opcodes over HID reports.
//! * [`transport`] — the byte pipe an enclosure talks over; [`hidraw`] is the
//!   Linux one, [`sim`] is a simulated enclosure that answers the same wire
//!   protocol so the whole stack is exercisable with no hardware.
//! * [`acquisition`] — what to shoot: one image per selected channel, where a
//!   channel is a light source (i.e. a tray).
//!
//! Why the driver lives here rather than in nu-manager (which supplies our
//! cameras): the enclosure is not one of nu-manager's device kinds, and its
//! control surface — trays, door interlock, lamp banks, latched faults — has no
//! camera analogue. The camera in a Gel Doc EZ still arrives through nu-manager
//! like any other.

pub mod acquisition;
pub mod geldoc_ez;
pub mod sim;
pub mod transport;

#[cfg(target_os = "linux")]
pub mod hidraw;

pub use acquisition::{AcquisitionPlan, CaptureMode, ChannelPlan};
pub use geldoc_ez::GelDocEz;

#[derive(Debug, thiserror::Error)]
pub enum InstrumentError {
    #[error("no instrument found")]
    NotFound,
    #[error("transport error: {0}")]
    Transport(String),
    #[error("timed out waiting for the instrument to respond")]
    Timeout,
    /// The instrument answered, but with a non-zero status byte.
    #[error("instrument rejected the command: {0}")]
    Device(DeviceStatus),
    /// A short or malformed response.
    #[error("malformed response: {0}")]
    Protocol(String),
    /// Refused locally, before touching the hardware — a door-open exposure, a
    /// missing tray. These are the interlocks, and they are ours to enforce.
    #[error("{0}")]
    Refused(String),
    #[error("not supported by this instrument")]
    Unsupported,
}

pub type Result<T> = std::result::Result<T, InstrumentError>;

/// The status byte an enclosure returns in every response. A bitmask, so more
/// than one reason can come back at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceStatus(pub u8);

impl DeviceStatus {
    pub const OK: DeviceStatus = DeviceStatus(0);
    pub fn is_ok(self) -> bool {
        self.0 == 0
    }
    pub fn unknown_command(self) -> bool {
        self.0 & 0x01 != 0
    }
    pub fn rejected(self) -> bool {
        self.0 & 0x02 != 0
    }
    pub fn invalid_parameter(self) -> bool {
        self.0 & 0x04 != 0
    }
    pub fn failed(self) -> bool {
        self.0 & 0x08 != 0
    }
}

impl std::fmt::Display for DeviceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut reasons = Vec::new();
        if self.unknown_command() {
            reasons.push("unknown command");
        }
        if self.rejected() {
            reasons.push("rejected in this state");
        }
        if self.invalid_parameter() {
            reasons.push("invalid parameter");
        }
        if self.failed() {
            reasons.push("execution failed");
        }
        if reasons.is_empty() {
            write!(f, "status 0x{:02x}", self.0)
        } else {
            write!(f, "{}", reasons.join(", "))
        }
    }
}

/// The interchangeable sample trays. Which one is inserted selects the light
/// source, so it is an input to the acquisition, not a cosmetic detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum TrayType {
    /// UV transilluminator (302 nm).
    Uv,
    /// White-light transilluminator.
    White,
    /// Blue-light transilluminator.
    Blue,
    /// Stain-free activation tray.
    StainFree,
}

impl TrayType {
    pub const ALL: [TrayType; 4] = [
        TrayType::Uv,
        TrayType::White,
        TrayType::Blue,
        TrayType::StainFree,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TrayType::Uv => "UV",
            TrayType::White => "White light",
            TrayType::Blue => "Blue light",
            TrayType::StainFree => "Stain-free",
        }
    }

    /// Whether inserting this tray means the lamps emit UV-B. Drives how loudly
    /// the door interlock is explained.
    pub fn is_uv(self) -> bool {
        matches!(self, TrayType::Uv | TrayType::StainFree)
    }
}

/// Live instrument state, decoded from one sense reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sense {
    /// The tray that is physically inserted, if it decodes to a known one.
    pub tray: Option<TrayType>,
    pub door_closed: bool,
    /// The instrument is busy — lamps warming up, or an operation in flight.
    pub busy: bool,
    /// The raw word, kept so the UI can surface bits nobody has decoded yet
    /// (the front Run button is believed to live in one of them).
    pub raw: u16,
}

/// The latched fault word.
///
/// Faults latch: they stay set until explicitly cleared, which is the point —
/// "the door was opened during imaging" is only meaningful after the fact, and
/// is what invalidates an exposure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Faults(pub u16);

/// One fault, in the user's language rather than the wire's.
pub struct FaultMessage {
    pub headline: &'static str,
    pub remedy: &'static str,
}

impl Faults {
    pub const NONE: Faults = Faults(0);

    pub fn is_clear(self) -> bool {
        self.0 == 0
    }
    pub fn no_sample_tray(self) -> bool {
        self.0 & 0x01 != 0
    }
    pub fn no_light_tray(self) -> bool {
        self.0 & 0x02 != 0
    }
    /// Latched: the door was opened while an exposure was running. An image
    /// taken across this is not valid data.
    pub fn door_opened_during_imaging(self) -> bool {
        self.0 & 0x04 != 0
    }
    pub fn lamp_bank_1(self) -> bool {
        self.0 & 0x08 != 0
    }
    pub fn lamp_bank_2(self) -> bool {
        self.0 & 0x10 != 0
    }
    pub fn communication_timeout(self) -> bool {
        self.0 & 0x20 != 0
    }

    /// Every active fault, worst first, each with what to do about it.
    ///
    /// Both lamp banks failing is reported as one message rather than two: all
    /// six lamps are replaced together, so it is one job, not two.
    pub fn messages(self) -> Vec<FaultMessage> {
        let mut out = Vec::new();
        if self.communication_timeout() {
            out.push(FaultMessage {
                headline: "Communication timeout — the instrument stopped responding",
                remedy: "Disconnect and reconnect the USB cable, then power-cycle the imager. \
                         Any image in progress should be discarded.",
            });
        }
        if self.lamp_bank_1() && self.lamp_bank_2() {
            out.push(FaultMessage {
                headline: "Both lamp banks have failed",
                remedy: "The illumination is dead, so exposures will be blank. Replace all six \
                         302 nm lamps together (lamp kit 1708097); see the instrument manual for \
                         the procedure.",
            });
        } else if self.lamp_bank_1() {
            out.push(FaultMessage {
                headline: "Lamp failure in bank 1",
                remedy: "Illumination is uneven, so quantitation from this image is unreliable. \
                         Replace all six lamps together (lamp kit 1708097).",
            });
        } else if self.lamp_bank_2() {
            out.push(FaultMessage {
                headline: "Lamp failure in bank 2",
                remedy: "Illumination is uneven, so quantitation from this image is unreliable. \
                         Replace all six lamps together (lamp kit 1708097).",
            });
        }
        if self.door_opened_during_imaging() {
            out.push(FaultMessage {
                headline: "The door was opened during imaging",
                remedy: "The exposure was interrupted and has been discarded. Close the door and \
                         run again.",
            });
        }
        if self.no_light_tray() {
            out.push(FaultMessage {
                headline: "Lamp assembly not detected",
                remedy: "The light tray is not seated. Power the instrument off, reseat the lamp \
                         assembly, and power it back on.",
            });
        }
        if self.no_sample_tray() {
            out.push(FaultMessage {
                headline: "No sample tray detected",
                remedy: "Slide a sample tray in until the magnet holds it.",
            });
        }
        out
    }
}

/// Front-panel indicator LEDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedId {
    Amber,
    Red,
    Green,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedState {
    Off,
    On,
    Blink,
}

/// What an instrument reports about itself at open.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstrumentInfo {
    pub model: String,
    pub firmware: (u32, u32),
    pub hardware: (u32, u32),
    pub serial: String,
    /// The camera serial the *enclosure* has stored. The pairing between box and
    /// camera is recorded in the enclosure's flash, so this is how we can tell
    /// the user they have the wrong camera selected.
    pub camera_serial: String,
}

impl InstrumentInfo {
    pub fn firmware_string(&self) -> String {
        format!("{}.{}", self.firmware.0, self.firmware.1)
    }
    pub fn hardware_string(&self) -> String {
        format!("{}.{}", self.hardware.0, self.hardware.1)
    }
}

/// A darkroom enclosure.
///
/// Every method may talk to the hardware, so callers belong on a worker thread
/// (see `gui::instrument_worker`), never the UI thread.
pub trait Instrument: Send {
    fn info(&self) -> &InstrumentInfo;

    /// One raw sense reading. Cheap, but bounces while a tray slides in — use
    /// [`Instrument::tray_debounced`] when the answer matters.
    fn sense(&mut self) -> Result<Sense>;

    /// The inserted tray, read until two consecutive readings agree.
    ///
    /// The tray sensors bounce during insertion; a single read intermittently
    /// reports the wrong tray, which would silently image with the wrong light
    /// source. `settle` is the pause between readings.
    fn tray_debounced(&mut self, settle: std::time::Duration) -> Result<Option<TrayType>> {
        let mut a = self.sense()?.raw;
        loop {
            std::thread::sleep(settle);
            let b = self.sense()?.raw;
            if a == b {
                return Ok(decode_tray(a));
            }
            a = b;
        }
    }

    fn faults(&mut self) -> Result<Faults>;

    /// Clear the latched faults (and the red fault LED with them).
    fn clear_faults(&mut self) -> Result<()>;

    /// Turn the illumination on. There is no separate lamp command: starting an
    /// acquisition *is* switching the light on, and which source fires is
    /// decided by the tray the user inserted.
    ///
    /// With `wait_ready`, this blocks until the lamps have stabilised.
    fn start_acquire(&mut self, wait_ready: bool) -> Result<()>;

    /// Turn the illumination off.
    fn stop_acquire(&mut self) -> Result<()>;

    fn set_led(&mut self, led: LedId, state: LedState) -> Result<()>;
}

/// Decode the tray from a raw sense word. Exposed for the codec and its tests.
pub(crate) fn decode_tray(raw: u16) -> Option<TrayType> {
    geldoc_ez::decode_tray(raw)
}

/// Linux udev rules granting access to the enclosures OpenGel drives.
///
/// These are `hidraw` rules, not USB ones: the enclosure is a HID device we open
/// through `/dev/hidraw*`, so a rule on the `usb` subsystem alone would not make
/// the node readable. Without this, connecting fails with a permission error
/// that looks like a missing instrument.
pub fn udev_rules() -> String {
    format!(
        "# Generated by `gel udev-rules` — do not edit.\n\
         #\n\
         # Imaging enclosures OpenGel drives over hidraw. TAG+=\"uaccess\" lets\n\
         # systemd-logind grant access to the active local desktop user;\n\
         # GROUP=\"plugdev\" is a fallback for lab workstations and headless\n\
         # setups where users are added to that group explicitly.\n\
         \n\
         # Bio-Rad Gel Doc EZ ({:04x}:{:04x}) and Criterion Stain Free ({:04x}:{:04x})\n\
         SUBSYSTEM==\"hidraw\", ATTRS{{idVendor}}==\"{:04x}\", MODE=\"0660\", \
         GROUP=\"plugdev\", TAG+=\"uaccess\"\n",
        geldoc_ez::VENDOR_ID,
        geldoc_ez::PRODUCT_ID_GEL_DOC_EZ,
        geldoc_ez::VENDOR_ID,
        geldoc_ez::PRODUCT_ID_CRITERION_STAIN_FREE,
        geldoc_ez::VENDOR_ID,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn udev_rules_cover_the_enclosure_on_the_hidraw_subsystem() {
        let rules = udev_rules();
        // A usb-subsystem rule would not make /dev/hidraw* accessible, which is
        // the node we actually open.
        assert!(rules.contains("SUBSYSTEM==\"hidraw\""), "{rules}");
        assert!(rules.contains("idVendor}==\"0614\""), "{rules}");
    }

    #[test]
    fn a_clear_fault_word_reports_nothing() {
        assert!(Faults::NONE.is_clear());
        assert!(Faults::NONE.messages().is_empty());
    }

    #[test]
    fn every_fault_bit_produces_an_actionable_message() {
        // A fault the user cannot act on is just an error message; each bit must
        // carry a remedy.
        for bit in 0..6 {
            let faults = Faults(1 << bit);
            let messages = faults.messages();
            assert_eq!(messages.len(), 1, "bit {bit} produced {} messages", messages.len());
            assert!(!messages[0].remedy.is_empty(), "bit {bit} has no remedy");
        }
    }
}
