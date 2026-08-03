//! Bio-Rad Gel Doc EZ enclosure — protocol codec and device.
//!
//! Also covers the Criterion Stain Free imager, which answers the same command
//! set on a different USB product id.
//!
//! The protocol is 14 opcodes over fixed-size HID reports: no checksum, no
//! sequence number, no handshake beyond reading the version at open. Every
//! transaction is one report out, one report in.
//!
//! Two facts shape everything above this layer:
//!
//! * **There is no "lamp on" command.** Light comes on with `StartAcquire` and
//!   off with `StopAcquire`, and *which* source fires is decided by the tray the
//!   user physically inserted — the host does not choose it. So an acquisition
//!   is a bracket around the camera exposure, and the tray is an input to the
//!   experiment rather than a setting.
//! * **The door interlock is real.** The lamps are gated on the door sensor in
//!   hardware. We refuse to start an exposure with the door open anyway, and
//!   treat the latched "door opened during imaging" fault as invalidating the
//!   image rather than as a warning.

use std::time::{Duration, Instant};

use super::transport::Transport;
use super::{
    DeviceStatus, Faults, Instrument, InstrumentError, InstrumentInfo, LedId, LedState, Result,
    Sense, TrayType,
};

/// USB vendor id: Bio-Rad Laboratories.
pub const VENDOR_ID: u16 = 0x0614;
/// USB product id of the Gel Doc EZ enclosure.
pub const PRODUCT_ID_GEL_DOC_EZ: u16 = 0x0467;
/// USB product id of the Criterion Stain Free enclosure (same command set).
pub const PRODUCT_ID_CRITERION_STAIN_FREE: u16 = 0x0466;

/// Command opcodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Opcode {
    GetFirmwareVersion = 0x00,
    GetHardwareVersion = 0x01,
    GetSenseInfo = 0x02,
    GetFaultStatus = 0x03,
    StartAcquire = 0x04,
    StopAcquire = 0x05,
    LedBlinkRate = 0x06,
    LedControl = 0x07,
    // The flat-field / pixel-map transfers. The opcodes are known; the payload
    // format is not decoded yet, so nothing here uses them. When it is, the
    // chunked transfer belongs behind a `read_flat_field` / `write_flat_field`
    // pair on `GelDocEz`, and flat fielding moves from a host-side correction to
    // the instrument's own stored map.
    DownloadPixelMap = 0x08,
    UploadPixelMap = 0x09,
    ClearFault = 0x0A,
    CommunicationTimeoutControl = 0x0B,
    DownloadExtendedPixelMap = 0x0C,
    UploadExtendedPixelMap = 0x0D,
}

// ---- sense word ------------------------------------------------------------

/// Bit 4: set means the door is *closed*. Note the polarity — the Universal
/// Hood family uses the opposite convention.
const SENSE_DOOR_CLOSED: u16 = 0x0010;
/// Bit 7: busy / not ready. Held while the lamps stabilise after `StartAcquire`.
const SENSE_BUSY: u16 = 0x0080;
/// The three bits that carry the tray code: 2, 8 and 10.
const SENSE_TRAY_MASK: u16 = 0x0504;
/// Every bit whose meaning is known. Anything outside this is surfaced raw —
/// the front Run button is believed to report in one of them, but which is not
/// established, so we show them rather than guess.
const SENSE_DECODED_MASK: u16 = SENSE_TRAY_MASK | SENSE_DOOR_CLOSED | SENSE_BUSY;

/// Decode the tray code. Four exact values; anything else means no tray, or one
/// this firmware does not know.
pub fn decode_tray(raw: u16) -> Option<TrayType> {
    match raw & SENSE_TRAY_MASK {
        0x0004 => Some(TrayType::Uv),
        0x0100 => Some(TrayType::White),
        0x0400 => Some(TrayType::Blue),
        0x0404 => Some(TrayType::StainFree),
        _ => None,
    }
}

/// Decode a whole sense word.
pub fn decode_sense(raw: u16) -> Sense {
    Sense {
        tray: decode_tray(raw),
        door_closed: raw & SENSE_DOOR_CLOSED != 0,
        busy: raw & SENSE_BUSY != 0,
        raw,
    }
}

/// The bits of a sense word nobody has decoded, as a mask.
pub fn undecoded_sense_bits(raw: u16) -> u16 {
    raw & !SENSE_DECODED_MASK
}

/// Encode a tray as the sense bits an instrument would report for it. Used by
/// the simulator, and by the codec tests to close the loop on [`decode_tray`].
pub fn encode_tray(tray: Option<TrayType>) -> u16 {
    match tray {
        Some(TrayType::Uv) => 0x0004,
        Some(TrayType::White) => 0x0100,
        Some(TrayType::Blue) => 0x0400,
        Some(TrayType::StainFree) => 0x0404,
        None => 0x0000,
    }
}

fn led_id_byte(led: LedId) -> u8 {
    match led {
        LedId::Amber => 0,
        LedId::Red => 1,
        LedId::Green => 2,
    }
}

fn led_state_byte(state: LedState) -> u8 {
    match state {
        LedState::Off => 0,
        LedState::On => 1,
        LedState::Blink => 2,
    }
}

// ---- device ----------------------------------------------------------------

/// How long to wait for the lamps to stabilise before giving up.
const READY_TIMEOUT: Duration = Duration::from_secs(30);
/// How often to re-read the sense word while waiting for ready.
const READY_POLL: Duration = Duration::from_millis(50);
/// Pause between the two readings of a debounced tray read.
pub const TRAY_SETTLE: Duration = Duration::from_millis(200);

/// A Gel Doc EZ enclosure over some [`Transport`].
pub struct GelDocEz<T: Transport> {
    transport: T,
    info: InstrumentInfo,
}

impl<T: Transport> GelDocEz<T> {
    /// Open the enclosure: read its firmware and hardware versions, which is
    /// also the only handshake there is — a device that answers opcode 0x00
    /// sensibly is talking to us.
    pub fn open(mut transport: T, model: impl Into<String>) -> Result<Self> {
        let firmware = read_version(&mut transport, Opcode::GetFirmwareVersion)?;
        let hardware = read_version(&mut transport, Opcode::GetHardwareVersion)?;
        Ok(Self {
            transport,
            info: InstrumentInfo {
                model: model.into(),
                firmware,
                hardware,
                // Serial numbers live in the device's storage area, reachable
                // through the pixel-map opcodes whose payload format is not
                // decoded yet. Left empty rather than filled with a guess.
                serial: String::new(),
                camera_serial: String::new(),
            },
        })
    }

    /// One command → one response, returning the response payload (everything
    /// after the status byte).
    fn transact(&mut self, opcode: Opcode, params: &[u8]) -> Result<Vec<u8>> {
        let size = self.transport.report_size();
        if params.len() + 1 > size {
            return Err(InstrumentError::Protocol(format!(
                "{} parameter bytes do not fit a {size}-byte report",
                params.len()
            )));
        }
        // Drop anything stale before asking a fresh question, or a leftover
        // reply gets read as this command's answer.
        self.transport.drain();

        let mut out = vec![0u8; size];
        out[0] = opcode as u8;
        out[1..1 + params.len()].copy_from_slice(params);
        self.transport.write_report(&out)?;

        let mut inp = vec![0u8; size];
        let n = self.transport.read_report(&mut inp)?;
        if n == 0 {
            return Err(InstrumentError::Protocol("empty response".into()));
        }
        let status = DeviceStatus(inp[0]);
        if !status.is_ok() {
            return Err(InstrumentError::Device(status));
        }
        inp.truncate(n);
        Ok(inp.split_off(1))
    }

    /// Raw sense word.
    fn sense_word(&mut self) -> Result<u16> {
        let data = self.transact(Opcode::GetSenseInfo, &[])?;
        if data.len() < 2 {
            return Err(InstrumentError::Protocol(
                "sense response shorter than 2 bytes".into(),
            ));
        }
        Ok(u16::from_le_bytes([data[0], data[1]]))
    }

    /// Block until the instrument clears its busy bit — the lamp warm-up wait.
    /// There is no completion interrupt, so this is a poll.
    fn wait_ready(&mut self) -> Result<()> {
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            if self.sense_word()? & SENSE_BUSY == 0 {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(InstrumentError::Timeout);
            }
            std::thread::sleep(READY_POLL);
        }
    }

    /// The transport, for callers that need to inspect the simulated one.
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }
}

/// Read a version pair: two little-endian u32s, major then minor.
fn read_version<T: Transport>(transport: &mut T, opcode: Opcode) -> Result<(u32, u32)> {
    let size = transport.report_size();
    transport.drain();
    let mut out = vec![0u8; size];
    out[0] = opcode as u8;
    transport.write_report(&out)?;
    let mut inp = vec![0u8; size];
    let n = transport.read_report(&mut inp)?;
    if n == 0 {
        return Err(InstrumentError::Protocol("empty version response".into()));
    }
    let status = DeviceStatus(inp[0]);
    if !status.is_ok() {
        return Err(InstrumentError::Device(status));
    }
    if n < 9 {
        return Err(InstrumentError::Protocol(format!(
            "version response is {n} bytes, need 9"
        )));
    }
    let major = u32::from_le_bytes([inp[1], inp[2], inp[3], inp[4]]);
    let minor = u32::from_le_bytes([inp[5], inp[6], inp[7], inp[8]]);
    Ok((major, minor))
}

impl<T: Transport> Instrument for GelDocEz<T> {
    fn info(&self) -> &InstrumentInfo {
        &self.info
    }

    fn sense(&mut self) -> Result<Sense> {
        Ok(decode_sense(self.sense_word()?))
    }

    fn faults(&mut self) -> Result<Faults> {
        let data = self.transact(Opcode::GetFaultStatus, &[])?;
        if data.len() < 2 {
            return Err(InstrumentError::Protocol(
                "fault response shorter than 2 bytes".into(),
            ));
        }
        Ok(Faults(u16::from_le_bytes([data[0], data[1]])))
    }

    fn clear_faults(&mut self) -> Result<()> {
        self.transact(Opcode::ClearFault, &[])?;
        // Clearing the fault also clears the fault light; the vendor software
        // does both, and leaving the red LED lit after the user has dealt with
        // the problem would be its own bug report.
        self.set_led(LedId::Red, LedState::Off)
    }

    fn start_acquire(&mut self, wait_ready: bool) -> Result<()> {
        self.transact(Opcode::StartAcquire, &[u8::from(wait_ready)])?;
        if wait_ready {
            self.wait_ready()?;
        }
        Ok(())
    }

    fn stop_acquire(&mut self) -> Result<()> {
        self.transact(Opcode::StopAcquire, &[])?;
        Ok(())
    }

    fn set_led(&mut self, led: LedId, state: LedState) -> Result<()> {
        self.transact(
            Opcode::LedControl,
            &[led_id_byte(led), led_state_byte(state), 0],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instrument::sim::SimulatedEnclosure;

    fn open_sim() -> GelDocEz<SimulatedEnclosure> {
        GelDocEz::open(SimulatedEnclosure::new(), "Simulated Gel Doc EZ").expect("open")
    }

    #[test]
    fn tray_codes_round_trip() {
        for tray in TrayType::ALL {
            assert_eq!(decode_tray(encode_tray(Some(tray))), Some(tray), "{tray:?}");
        }
        assert_eq!(decode_tray(encode_tray(None)), None);
    }

    #[test]
    fn tray_decode_ignores_unrelated_bits() {
        // Door and busy live outside the tray mask; setting them must not change
        // which tray is reported.
        let uv = encode_tray(Some(TrayType::Uv));
        assert_eq!(decode_tray(uv | SENSE_DOOR_CLOSED | SENSE_BUSY), Some(TrayType::Uv));
    }

    #[test]
    fn stain_free_is_not_confused_with_uv_or_blue() {
        // Stain-free is UV's bit *plus* blue's bit, so a sloppy decode that
        // masked one bit at a time would call it UV. This is the case that
        // decides which light source fires.
        let sf = encode_tray(Some(TrayType::StainFree));
        assert_eq!(decode_tray(sf), Some(TrayType::StainFree));
        assert_ne!(decode_tray(sf), Some(TrayType::Uv));
    }

    #[test]
    fn door_bit_is_closed_not_open() {
        assert!(decode_sense(SENSE_DOOR_CLOSED).door_closed);
        assert!(!decode_sense(0).door_closed);
    }

    #[test]
    fn undecoded_bits_are_reported_not_swallowed() {
        // Bit 0 is not decoded by anything we know; it must survive to the UI so
        // an unknown signal (the front Run button, most likely) is findable.
        let raw = SENSE_DOOR_CLOSED | 0x0001;
        assert_eq!(undecoded_sense_bits(raw), 0x0001);
        assert_eq!(undecoded_sense_bits(SENSE_DOOR_CLOSED | SENSE_BUSY), 0);
    }

    #[test]
    fn open_reads_versions() {
        let dev = open_sim();
        assert_eq!(dev.info().firmware, SimulatedEnclosure::FIRMWARE);
        assert_eq!(dev.info().hardware, SimulatedEnclosure::HARDWARE);
    }

    #[test]
    fn sense_reports_the_simulated_tray_and_door() {
        let mut dev = open_sim();
        dev.transport_mut().set_tray(Some(TrayType::Blue));
        dev.transport_mut().set_door_closed(true);
        let sense = dev.sense().expect("sense");
        assert_eq!(sense.tray, Some(TrayType::Blue));
        assert!(sense.door_closed);
    }

    #[test]
    fn start_acquire_waits_for_the_lamps_and_lights_them() {
        let mut dev = open_sim();
        dev.transport_mut().set_warmup(Duration::from_millis(20));
        dev.start_acquire(true).expect("start");
        // wait_ready only returns once the busy bit clears, so by here the
        // lamps are on *and* stable.
        assert!(dev.transport_mut().lamps_on());
        assert!(!dev.sense().expect("sense").busy);
        dev.stop_acquire().expect("stop");
        assert!(!dev.transport_mut().lamps_on());
    }

    #[test]
    fn faults_are_read_and_cleared() {
        let mut dev = open_sim();
        dev.transport_mut().set_faults(Faults(0x01 | 0x08));
        let faults = dev.faults().expect("faults");
        assert!(faults.no_sample_tray() && faults.lamp_bank_1());
        assert_eq!(faults.messages().len(), 2);
        dev.clear_faults().expect("clear");
        assert!(dev.faults().expect("faults").is_clear());
        // Clearing must also drop the red LED.
        assert_eq!(dev.transport_mut().led(LedId::Red), LedState::Off);
    }

    #[test]
    fn both_lamp_banks_failing_is_one_message_not_two() {
        let both = Faults(0x08 | 0x10).messages();
        assert_eq!(both.len(), 1);
        assert!(both[0].headline.contains("Both lamp banks"));
    }

    #[test]
    fn a_rejected_command_surfaces_as_a_device_error() {
        let mut dev = open_sim();
        dev.transport_mut().reject_next(DeviceStatus(0x02));
        let err = dev.sense().expect_err("the simulator rejected it");
        assert!(matches!(err, InstrumentError::Device(s) if s.rejected()), "{err}");
    }

    #[test]
    fn a_stale_reply_does_not_desynchronise_the_stream() {
        // The failure this guards against: an aborted operation leaves a reply
        // queued, and every answer afterwards is off by one — the driver would
        // report the *previous* tray forever, silently.
        let mut dev = open_sim();
        dev.transport_mut().set_tray(Some(TrayType::White));
        dev.transport_mut().queue_stale_reply(&[0x00, 0x04, 0x00]);
        assert_eq!(dev.sense().expect("sense").tray, Some(TrayType::White));
    }

    #[test]
    fn tray_debounce_ignores_a_bouncing_sensor() {
        // A tray sliding in bounces; a single read catches it mid-flight. The
        // debounced read must return the settled value, not the transient.
        let mut dev = open_sim();
        dev.transport_mut().set_tray(Some(TrayType::StainFree));
        dev.transport_mut()
            .bounce_sense(vec![encode_tray(Some(TrayType::Uv)), 0x0000]);
        let tray = dev
            .tray_debounced(Duration::from_millis(1))
            .expect("debounced read");
        assert_eq!(tray, Some(TrayType::StainFree));
    }
}
