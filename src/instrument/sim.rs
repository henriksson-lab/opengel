//! A simulated Gel Doc EZ enclosure.
//!
//! It simulates at the **wire level** — it implements [`Transport`], not
//! [`Instrument`] — so the real codec in [`super::geldoc_ez`] runs unmodified on
//! top of it. Opcode encoding, the status byte, the sense-word layout, the
//! busy-bit warm-up poll and the stale-report drain are all exercised by the
//! same code that will drive the hardware; only the USB is missing.
//!
//! It also models the things that are hard to produce on demand with real
//! hardware and expensive to get wrong: a bouncing tray sensor, a door opened
//! mid-exposure, a lamp bank dying, a command rejected in the wrong state.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::geldoc_ez::{encode_tray, Opcode};
use super::transport::Transport;
use super::{DeviceStatus, Faults, InstrumentError, LedId, LedState, Result, TrayType};

/// Report size the simulated device declares. Real hardware answers 63 or 64
/// depending on its HID descriptor; the codec reads it from the transport
/// rather than assuming, and this exercises that path.
const REPORT_SIZE: usize = 64;

const SENSE_DOOR_CLOSED: u16 = 0x0010;
const SENSE_BUSY: u16 = 0x0080;

/// A simulated enclosure answering the Gel Doc EZ protocol.
pub struct SimulatedEnclosure {
    tray: Option<TrayType>,
    door_closed: bool,
    lamps_on: bool,
    /// When the lamps were switched on, and how long they claim to need. While
    /// inside that window the busy bit is set, exactly as a warm-up looks.
    lit_at: Option<Instant>,
    warmup: Duration,
    faults: Faults,
    leds: [LedState; 3],
    /// Sense words to hand out before the settled one — a bouncing tray sensor.
    bounce: VecDeque<u16>,
    /// Raw reports queued to be read, ahead of any real reply.
    stale: VecDeque<Vec<u8>>,
    /// Replies produced by commands, waiting to be read.
    replies: VecDeque<Vec<u8>>,
    /// A status to fail the next command with.
    reject_next: Option<DeviceStatus>,
    /// Which sense bit the front Run button reports in. Unknown on real
    /// hardware — see the note on [`SimulatedEnclosure::set_button_bit`].
    button_bit: u16,
    button_pressed: bool,
}

impl Default for SimulatedEnclosure {
    fn default() -> Self {
        Self::new()
    }
}

impl SimulatedEnclosure {
    pub const FIRMWARE: (u32, u32) = (2, 14);
    pub const HARDWARE: (u32, u32) = (1, 3);

    pub fn new() -> Self {
        Self {
            // Only the stain-free tray ships with the instrument, so that is the
            // one most likely to be sitting in a fresh machine.
            tray: Some(TrayType::StainFree),
            door_closed: true,
            lamps_on: false,
            lit_at: None,
            warmup: Duration::from_millis(400),
            faults: Faults::NONE,
            leds: [LedState::Off, LedState::Off, LedState::On],
            bounce: VecDeque::new(),
            stale: VecDeque::new(),
            replies: VecDeque::new(),
            reject_next: None,
            button_bit: 0x0001,
            button_pressed: false,
        }
    }

    // ---- bench controls (what a person would do to the real box) ----

    pub fn set_tray(&mut self, tray: Option<TrayType>) {
        self.tray = tray;
        self.publish_light();
    }

    pub fn tray(&self) -> Option<TrayType> {
        self.tray
    }

    /// Open or close the door. Opening it while the lamps are on latches the
    /// "door opened during imaging" fault, which is what invalidates the
    /// exposure in progress.
    pub fn set_door_closed(&mut self, closed: bool) {
        if !closed && self.lamps_on {
            self.faults = Faults(self.faults.0 | 0x04);
        }
        self.door_closed = closed;
    }

    /// Tell the rest of the simulated bench what is lighting the gel, so the
    /// mock camera photographs the light source that is actually on.
    fn publish_light(&self) {
        crate::simbench::set_light(self.tray, self.lamps_on);
    }

    pub fn door_closed(&self) -> bool {
        self.door_closed
    }

    pub fn lamps_on(&self) -> bool {
        self.lamps_on
    }

    pub fn set_warmup(&mut self, warmup: Duration) {
        self.warmup = warmup;
    }

    pub fn set_faults(&mut self, faults: Faults) {
        self.faults = faults;
    }

    pub fn led(&self, led: LedId) -> LedState {
        self.leds[led_index(led)]
    }

    /// Queue sense words to return before the settled one, simulating a tray
    /// sensor bouncing as the tray slides in.
    pub fn bounce_sense(&mut self, words: Vec<u16>) {
        self.bounce = words.into();
    }

    /// Queue a raw report to be read *before* the next real reply — what an
    /// aborted operation leaves behind on the real device.
    pub fn queue_stale_reply(&mut self, report: &[u8]) {
        let mut padded = vec![0u8; REPORT_SIZE];
        padded[..report.len()].copy_from_slice(report);
        self.stale.push_back(padded);
    }

    /// Fail the next command with this status.
    pub fn reject_next(&mut self, status: DeviceStatus) {
        self.reject_next = Some(status);
    }

    /// Which sense bit a front-button press should set.
    ///
    /// The vendor software has an `IsFrontButtonPressed` path, but the decoded
    /// sense bits (tray, door, busy) do not include it and static analysis never
    /// pinned it down. So this is configurable rather than guessed: point it at
    /// whichever bit a capture from real hardware shows moving when the button
    /// is pressed.
    pub fn set_button_bit(&mut self, mask: u16) {
        self.button_bit = mask;
    }

    /// Press the front Run button. The bit is reported once and then clears,
    /// like a momentary contact.
    pub fn press_run_button(&mut self) {
        self.button_pressed = true;
    }

    fn sense_word(&mut self) -> u16 {
        if let Some(word) = self.bounce.pop_front() {
            return word;
        }
        let mut sense = encode_tray(self.tray);
        if self.door_closed {
            sense |= SENSE_DOOR_CLOSED;
        }
        // Busy while the lamps are still warming up.
        if let Some(lit_at) = self.lit_at {
            if lit_at.elapsed() < self.warmup {
                sense |= SENSE_BUSY;
            }
        }
        if self.button_pressed {
            sense |= self.button_bit;
            self.button_pressed = false;
        }
        sense
    }

    /// Build a reply: status byte, then payload.
    fn reply(&mut self, status: DeviceStatus, payload: &[u8]) {
        let mut report = vec![0u8; REPORT_SIZE];
        report[0] = status.0;
        report[1..1 + payload.len()].copy_from_slice(payload);
        self.replies.push_back(report);
    }

    fn handle(&mut self, opcode: u8, params: &[u8]) {
        if let Some(status) = self.reject_next.take() {
            self.reply(status, &[]);
            return;
        }
        match opcode {
            x if x == Opcode::GetFirmwareVersion as u8 => {
                let payload = version_payload(Self::FIRMWARE);
                self.reply(DeviceStatus::OK, &payload);
            }
            x if x == Opcode::GetHardwareVersion as u8 => {
                let payload = version_payload(Self::HARDWARE);
                self.reply(DeviceStatus::OK, &payload);
            }
            x if x == Opcode::GetSenseInfo as u8 => {
                let sense = self.sense_word();
                self.reply(DeviceStatus::OK, &sense.to_le_bytes());
            }
            x if x == Opcode::GetFaultStatus as u8 => {
                let faults = self.faults.0;
                self.reply(DeviceStatus::OK, &faults.to_le_bytes());
            }
            x if x == Opcode::StartAcquire as u8 => {
                // The hardware gates the lamps on the door sensor. Model that:
                // a start with the door open lights nothing and is refused,
                // which is what makes the interlock testable.
                if !self.door_closed {
                    self.reply(DeviceStatus(0x02), &[]);
                    return;
                }
                self.lamps_on = true;
                self.lit_at = Some(Instant::now());
                self.publish_light();
                let _wait_ready = params.first().copied().unwrap_or(0);
                self.reply(DeviceStatus::OK, &[]);
            }
            x if x == Opcode::StopAcquire as u8 => {
                self.lamps_on = false;
                self.lit_at = None;
                self.publish_light();
                self.reply(DeviceStatus::OK, &[]);
            }
            x if x == Opcode::ClearFault as u8 => {
                self.faults = Faults::NONE;
                self.reply(DeviceStatus::OK, &[]);
            }
            x if x == Opcode::LedControl as u8 => {
                let (id, state) = (params.first().copied(), params.get(1).copied());
                match (
                    id.and_then(led_from_byte),
                    state.and_then(led_state_from_byte),
                ) {
                    (Some(id), Some(state)) => {
                        self.leds[led_index(id)] = state;
                        self.reply(DeviceStatus::OK, &[]);
                    }
                    _ => self.reply(DeviceStatus(0x04), &[]),
                }
            }
            // Known opcodes whose payload format is not decoded yet, plus
            // anything else: answer the way a device answers a command it will
            // not run, rather than pretending success.
            _ => self.reply(DeviceStatus(0x01), &[]),
        }
    }
}

fn version_payload((major, minor): (u32, u32)) -> Vec<u8> {
    let mut payload = Vec::with_capacity(8);
    payload.extend_from_slice(&major.to_le_bytes());
    payload.extend_from_slice(&minor.to_le_bytes());
    payload
}

fn led_index(led: LedId) -> usize {
    match led {
        LedId::Amber => 0,
        LedId::Red => 1,
        LedId::Green => 2,
    }
}

fn led_from_byte(b: u8) -> Option<LedId> {
    match b {
        0 => Some(LedId::Amber),
        1 => Some(LedId::Red),
        2 => Some(LedId::Green),
        _ => None,
    }
}

fn led_state_from_byte(b: u8) -> Option<LedState> {
    match b {
        0 => Some(LedState::Off),
        1 => Some(LedState::On),
        2 => Some(LedState::Blink),
        _ => None,
    }
}

impl Transport for SimulatedEnclosure {
    fn report_size(&self) -> usize {
        REPORT_SIZE
    }

    fn write_report(&mut self, report: &[u8]) -> Result<()> {
        let opcode = report.first().copied().unwrap_or(0xff);
        let params: Vec<u8> = report.get(1..).unwrap_or(&[]).to_vec();
        self.handle(opcode, &params);
        Ok(())
    }

    fn read_report(&mut self, buf: &mut [u8]) -> Result<usize> {
        let report = self
            .stale
            .pop_front()
            .or_else(|| self.replies.pop_front())
            .ok_or(InstrumentError::Timeout)?;
        let n = report.len().min(buf.len());
        buf[..n].copy_from_slice(&report[..n]);
        Ok(n)
    }

    fn drain(&mut self) {
        self.stale.clear();
        self.replies.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instrument::{GelDocEz, Instrument};

    #[test]
    fn opening_the_door_mid_exposure_latches_the_fault() {
        // The case that must never be reported as a good image.
        let mut dev = GelDocEz::open(SimulatedEnclosure::new(), "sim").expect("open");
        dev.start_acquire(false).expect("start");
        dev.transport_mut().set_door_closed(false);
        dev.stop_acquire().expect("stop");
        assert!(dev.faults().expect("faults").door_opened_during_imaging());
    }

    #[test]
    fn the_lamps_refuse_to_light_with_the_door_open() {
        let mut dev = GelDocEz::open(SimulatedEnclosure::new(), "sim").expect("open");
        dev.transport_mut().set_door_closed(false);
        assert!(dev.start_acquire(false).is_err());
        assert!(!dev.transport_mut().lamps_on());
    }

    #[test]
    fn an_undecoded_opcode_is_refused_rather_than_faked() {
        // The pixel-map opcodes land here until their payload format is known.
        let mut dev = GelDocEz::open(SimulatedEnclosure::new(), "sim").expect("open");
        let size = dev.transport_mut().report_size();
        let mut report = vec![0u8; size];
        report[0] = Opcode::UploadPixelMap as u8;
        dev.transport_mut().write_report(&report).expect("write");
        let mut buf = vec![0u8; size];
        dev.transport_mut().read_report(&mut buf).expect("read");
        assert!(DeviceStatus(buf[0]).unknown_command());
    }

    #[test]
    fn a_button_press_is_reported_once() {
        let mut dev = GelDocEz::open(SimulatedEnclosure::new(), "sim").expect("open");
        dev.transport_mut().set_button_bit(0x0001);
        dev.transport_mut().press_run_button();
        assert_eq!(dev.sense().expect("sense").raw & 0x0001, 0x0001);
        assert_eq!(dev.sense().expect("sense").raw & 0x0001, 0);
    }
}
