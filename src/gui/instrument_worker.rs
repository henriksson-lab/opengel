//! A dedicated thread owning the imaging enclosure, so no instrument I/O ever
//! runs on the UI thread — a sense poll, a lamp warm-up wait or a 45-second
//! stain-free activation would otherwise freeze the window.
//!
//! The worker polls the instrument continuously (tray, door, faults) and pushes
//! [`InstEvent`]s the UI drains from the same timer that drains camera events.
//!
//! **A run spans both workers.** The enclosure owns the light and the camera
//! owns the exposure, and they are separate devices on separate threads, so a
//! run is sequenced by the UI as a small state machine:
//!
//! ```text
//!   UI  --BeginRun-->  instrument   (interlocks, activation, lamps on, warm up)
//!   UI  <-LightsReady- instrument
//!   UI  --capture---->  camera      (auto or manual exposure)
//!   UI  <-CaptureDone-  camera
//!   UI  --EndRun---->  instrument   (lamps off, read the latched faults)
//!   UI  <-RunFinished- instrument   (door violation? then discard the frames)
//! ```
//!
//! The frames are held, not adopted, until `RunFinished` confirms the door
//! stayed shut: an exposure interrupted by an opened door is not valid data.
//!
//! If the UI never sends `EndRun` — a panic, a wedged event loop — the worker
//! switches the lamps off itself after [`RUN_WATCHDOG`]. Leaving UV-B lamps lit
//! because a GUI thread died is not an acceptable failure mode.

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use opengel::instrument::geldoc_ez::{self, GelDocEz};
use opengel::instrument::sim::SimulatedEnclosure;
use opengel::instrument::{Faults, Instrument, InstrumentInfo, LedId, LedState, Sense, TrayType};

/// How often to re-read the instrument while idle.
const POLL_INTERVAL: Duration = Duration::from_millis(250);
/// How long the lamps may stay on without the UI confirming the run is over.
/// Generous enough for a long exposure plus overhead, short enough that a dead
/// UI does not leave the lamps burning.
const RUN_WATCHDOG: Duration = Duration::from_secs(120);
/// How long the lamps may burn for framing before they are switched off. Lamp
/// life is finite, and a preview left running overnight would spend it.
const PREVIEW_LAMP_LIMIT: Duration = Duration::from_secs(600);
/// How often the activation countdown reports progress.
const ACTIVATION_TICK: Duration = Duration::from_millis(250);

/// Commands from the UI thread to the instrument worker.
pub enum InstCommand {
    /// Re-enumerate the attached enclosures.
    List,
    /// Open the enclosure at this index in the last [`InstCommand::List`].
    Connect(usize),
    Disconnect,
    ClearFaults,
    /// Check the interlocks, light the lamps and let them settle. Answers with
    /// [`InstEvent::LightsReady`] or [`InstEvent::RunRefused`].
    BeginRun {
        warmup_s: f64,
    },
    /// Light the lamps for `seconds` and put them out again, taking no picture.
    ///
    /// This is the stain-free activation, and it is a deliberate act on the
    /// *gel* rather than a step of imaging it: the chemistry runs once, and no
    /// instrument can tell an activated gel from a fresh one. Only the person
    /// holding it knows, so only they can ask.
    ActivateGel {
        seconds: f64,
    },
    /// Lamps off; read the latched faults and report whether the run is valid.
    EndRun,
    /// Abandon a run in progress — lamps off now.
    Abort,
    /// Switch the illumination on or off outside a run, so the live preview
    /// shows the gel under the light source that is actually in. Which lamps
    /// fire is decided by the inserted tray, as always.
    Illuminate(bool),
    /// Whether a rising undecoded sense bit should be treated as the front Run
    /// button (see [`InstEvent::ButtonPressed`]).
    WatchRunButton(bool),
    // ---- simulated-enclosure bench controls ----
    SimSetTray(Option<TrayType>),
    SimSetDoor(bool),
    SimSetFaults(u16),
    SimPressButton,
    Shutdown,
}

/// Events from the instrument worker to the UI thread.
pub enum InstEvent {
    /// The enclosures found, in the order [`InstCommand::Connect`] indexes.
    Instruments(Vec<String>),
    Connected {
        info: InstrumentInfo,
        simulated: bool,
    },
    ConnectFailed(String),
    Disconnected,
    /// A fresh reading. Sent whenever anything in it changes.
    Status {
        sense: Sense,
        faults: Faults,
        /// Sense bits with no known meaning, as a mask.
        undecoded: u16,
    },
    /// An undecoded sense bit went high, which is most likely the front Run
    /// button. `mask` is the bit that moved, so the user can see *which* — the
    /// button's bit was never pinned down by the protocol analysis, so this is
    /// a heuristic the UI labels as one rather than a decoded signal.
    ButtonPressed {
        mask: u16,
    },
    /// The lamps are on and settling before the exposure — a plain warm-up, or
    /// a stain-free activation, which is the same window seen two ways.
    LampWindow {
        elapsed_s: f64,
        total_s: f64,
        /// Whether this window is driving the stain-free chemistry, rather than
        /// only letting the lamps settle. Changes what it is called, not what
        /// it does.
        activating: bool,
    },
    /// Lamps are lit and stable — expose now.
    LightsReady,
    /// The activation window finished and the lamps are out. No image was
    /// taken; the gel is simply ready to be imaged now.
    ActivationFinished,
    /// The illumination was switched on or off outside a run.
    Lamps(bool),
    /// The run never started; the reason is for the user.
    RunRefused(String),
    /// The run is over. `door_violation` means the door was opened mid-exposure
    /// and whatever the camera returned must be discarded.
    RunFinished {
        faults: Faults,
        door_violation: bool,
    },
    Error(String),
}

/// UI-side handle. All methods are non-blocking.
pub struct InstrumentHandle {
    tx: Sender<InstCommand>,
    _join: JoinHandle<()>,
}

impl InstrumentHandle {
    pub fn list(&self) {
        let _ = self.tx.send(InstCommand::List);
    }
    pub fn connect(&self, index: usize) {
        let _ = self.tx.send(InstCommand::Connect(index));
    }
    pub fn disconnect(&self) {
        let _ = self.tx.send(InstCommand::Disconnect);
    }
    pub fn clear_faults(&self) {
        let _ = self.tx.send(InstCommand::ClearFaults);
    }
    pub fn begin_run(&self, warmup_s: f64) {
        let _ = self.tx.send(InstCommand::BeginRun { warmup_s });
    }
    pub fn activate_gel(&self, seconds: f64) {
        let _ = self.tx.send(InstCommand::ActivateGel { seconds });
    }
    pub fn end_run(&self) {
        let _ = self.tx.send(InstCommand::EndRun);
    }
    pub fn abort(&self) {
        let _ = self.tx.send(InstCommand::Abort);
    }
    pub fn illuminate(&self, on: bool) {
        let _ = self.tx.send(InstCommand::Illuminate(on));
    }
    pub fn watch_run_button(&self, watch: bool) {
        let _ = self.tx.send(InstCommand::WatchRunButton(watch));
    }
    pub fn sim_set_tray(&self, tray: Option<TrayType>) {
        let _ = self.tx.send(InstCommand::SimSetTray(tray));
    }
    pub fn sim_set_door(&self, closed: bool) {
        let _ = self.tx.send(InstCommand::SimSetDoor(closed));
    }
    pub fn sim_set_faults(&self, faults: u16) {
        let _ = self.tx.send(InstCommand::SimSetFaults(faults));
    }
    pub fn sim_press_button(&self) {
        let _ = self.tx.send(InstCommand::SimPressButton);
    }
}

impl Drop for InstrumentHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(InstCommand::Shutdown);
    }
}

/// Spawn the worker. Returns the handle plus the event receiver.
pub fn spawn() -> (InstrumentHandle, Receiver<InstEvent>) {
    let (cmd_tx, cmd_rx) = channel::<InstCommand>();
    let (evt_tx, evt_rx) = channel::<InstEvent>();
    let join = std::thread::Builder::new()
        .name("instrument-worker".into())
        .spawn(move || worker_main(cmd_rx, evt_tx))
        .expect("spawn instrument worker");
    (
        InstrumentHandle {
            tx: cmd_tx,
            _join: join,
        },
        evt_rx,
    )
}

// ---- discovery -------------------------------------------------------------

/// An enclosure we could connect to.
#[derive(Debug, Clone)]
enum Candidate {
    #[cfg(target_os = "linux")]
    HidRaw {
        device: opengel::instrument::hidraw::HidDevice,
        model: String,
    },
    /// The same enclosure, reached through the host's own HID stack where
    /// `hidraw` does not exist.
    #[cfg(all(not(target_os = "linux"), numanager_backend))]
    OsHid {
        device: opengel::instrument::oshid::HidDevice,
        model: String,
    },
    Simulated,
}

/// Every attached enclosure, plus the simulator.
///
/// The simulator is always offered, and last: it makes the tab fully usable
/// with no hardware, which is how most of this gets developed and demonstrated,
/// but a real instrument should always win the default selection.
fn enumerate() -> Vec<(String, Candidate)> {
    let mut out = Vec::new();
    #[cfg(target_os = "linux")]
    {
        use opengel::instrument::hidraw;
        // Reached through the module rather than imported by name: these are
        // used only on Linux, so a plain `use` reads as dead on every other
        // platform and gets tidied away — taking the Linux build with it.
        for device in hidraw::find_devices(
            geldoc_ez::VENDOR_ID,
            &[
                geldoc_ez::PRODUCT_ID_GEL_DOC_EZ,
                geldoc_ez::PRODUCT_ID_CRITERION_STAIN_FREE,
            ],
        ) {
            let model = match device.product_id {
                geldoc_ez::PRODUCT_ID_GEL_DOC_EZ => "Gel Doc EZ",
                geldoc_ez::PRODUCT_ID_CRITERION_STAIN_FREE => "Criterion Stain Free",
                _ => "Bio-Rad enclosure",
            }
            .to_string();
            out.push((
                format!("{model} ({})", device.path.display()),
                Candidate::HidRaw { device, model },
            ));
        }
    }
    // Same enclosure, same ids — only the way in differs. Kept beside the Linux
    // branch rather than folded into it: `hidraw` needs no extra library on the
    // host that has it, and this needs the nu-manager device layer.
    #[cfg(all(not(target_os = "linux"), numanager_backend))]
    {
        use opengel::instrument::oshid;
        for device in oshid::find_devices(
            geldoc_ez::VENDOR_ID,
            &[
                geldoc_ez::PRODUCT_ID_GEL_DOC_EZ,
                geldoc_ez::PRODUCT_ID_CRITERION_STAIN_FREE,
            ],
        ) {
            let model = match device.product_id {
                geldoc_ez::PRODUCT_ID_GEL_DOC_EZ => "Gel Doc EZ",
                geldoc_ez::PRODUCT_ID_CRITERION_STAIN_FREE => "Criterion Stain Free",
                _ => "Bio-Rad enclosure",
            }
            .to_string();
            // The serial distinguishes two of the same model on one bench; with
            // none reported the model alone is all there is to show.
            let label = match &device.serial_number {
                Some(serial) if !serial.is_empty() => format!("{model} ({serial})"),
                _ => model.clone(),
            };
            out.push((label, Candidate::OsHid { device, model }));
        }
    }
    out.push(("Simulated Gel Doc EZ".into(), Candidate::Simulated));
    out
}

/// A connected enclosure.
///
/// An enum rather than `Box<dyn Instrument>` because the simulated one also
/// answers bench controls (move the tray, open the door) that a real instrument
/// has no equivalent for, and this keeps that reachable without downcasting.
enum Connected {
    #[cfg(target_os = "linux")]
    Hid(GelDocEz<opengel::instrument::hidraw::HidRawTransport>),
    #[cfg(all(not(target_os = "linux"), numanager_backend))]
    OsHid(GelDocEz<opengel::instrument::oshid::OsHidTransport>),
    Sim(GelDocEz<SimulatedEnclosure>),
}

impl Connected {
    fn instrument(&mut self) -> &mut dyn Instrument {
        match self {
            #[cfg(target_os = "linux")]
            Connected::Hid(dev) => dev,
            #[cfg(all(not(target_os = "linux"), numanager_backend))]
            Connected::OsHid(dev) => dev,
            Connected::Sim(dev) => dev,
        }
    }

    fn sim(&mut self) -> Option<&mut SimulatedEnclosure> {
        match self {
            #[cfg(target_os = "linux")]
            Connected::Hid(_) => None,
            #[cfg(all(not(target_os = "linux"), numanager_backend))]
            Connected::OsHid(_) => None,
            Connected::Sim(dev) => Some(dev.transport_mut()),
        }
    }

    fn is_simulated(&self) -> bool {
        match self {
            #[cfg(target_os = "linux")]
            Connected::Hid(_) => false,
            #[cfg(all(not(target_os = "linux"), numanager_backend))]
            Connected::OsHid(_) => false,
            Connected::Sim(_) => true,
        }
    }
}

fn connect(candidate: &Candidate) -> opengel::instrument::Result<Connected> {
    match candidate {
        #[cfg(target_os = "linux")]
        Candidate::HidRaw { device, model } => {
            let transport = opengel::instrument::hidraw::HidRawTransport::open(device.clone())?;
            Ok(Connected::Hid(GelDocEz::open(transport, model)?))
        }
        #[cfg(all(not(target_os = "linux"), numanager_backend))]
        Candidate::OsHid { device, model } => {
            let transport = opengel::instrument::oshid::OsHidTransport::open(device.clone())?;
            Ok(Connected::OsHid(GelDocEz::open(transport, model)?))
        }
        Candidate::Simulated => Ok(Connected::Sim(GelDocEz::open(
            SimulatedEnclosure::new(),
            "Simulated Gel Doc EZ",
        )?)),
    }
}

// ---- worker ----------------------------------------------------------------

/// State the poll loop carries between iterations.
struct Poller {
    last_sense: Option<Sense>,
    last_faults: Faults,
    /// Undecoded bits as of the previous reading, for edge detection.
    last_undecoded: u16,
    watch_button: bool,
    /// When the current run's lamps must go off if the UI has not said so.
    run_deadline: Option<Instant>,
}

impl Poller {
    fn new() -> Self {
        Self {
            last_sense: None,
            last_faults: Faults::NONE,
            last_undecoded: 0,
            watch_button: true,
            run_deadline: None,
        }
    }
}

fn worker_main(rx: Receiver<InstCommand>, tx: Sender<InstEvent>) {
    let mut listed: Vec<(String, Candidate)> = Vec::new();
    let mut connected: Option<Connected> = None;
    let mut poller = Poller::new();

    loop {
        // Block for a command, but never longer than one poll interval, so the
        // instrument keeps being read while the UI is quiet.
        match rx.recv_timeout(POLL_INTERVAL) {
            Ok(InstCommand::Shutdown) => {
                // Never leave the lamps on behind us.
                if let Some(dev) = connected.as_mut() {
                    let _ = dev.instrument().stop_acquire();
                }
                return;
            }
            Ok(cmd) => {
                handle(cmd, &mut listed, &mut connected, &mut poller, &tx);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(dev) = connected.as_mut() {
                    let _ = dev.instrument().stop_acquire();
                }
                return;
            }
        }

        if let Some(dev) = connected.as_mut() {
            // The lamps have been on too long with no word from the UI.
            if poller.run_deadline.is_some_and(|d| Instant::now() >= d) {
                poller.run_deadline = None;
                let _ = dev.instrument().stop_acquire();
                let _ = tx.send(InstEvent::Lamps(false));
                let _ = tx.send(InstEvent::Error(
                    "The lamps were switched off after their time limit.".into(),
                ));
            }
            poll_once(dev, &mut poller, &tx);
        }
    }
}

/// One sense + fault reading, emitting an event only when something changed.
fn poll_once(dev: &mut Connected, poller: &mut Poller, tx: &Sender<InstEvent>) {
    let sense = match dev.instrument().sense() {
        Ok(sense) => sense,
        Err(e) => {
            let _ = tx.send(InstEvent::Error(format!("Instrument read failed: {e}")));
            return;
        }
    };
    let faults = dev.instrument().faults().unwrap_or(poller.last_faults);
    let undecoded = geldoc_ez::undecoded_sense_bits(sense.raw);

    // A bit with no known meaning going high is, on the evidence available, the
    // front Run button — the vendor software has a button path, but which bit
    // carries it was never established. Reported as a candidate, and only acted
    // on if the user left the option enabled.
    let rising = undecoded & !poller.last_undecoded;
    if rising != 0 && poller.watch_button {
        let _ = tx.send(InstEvent::ButtonPressed { mask: rising });
    }
    poller.last_undecoded = undecoded;

    let changed = poller.last_sense != Some(sense) || poller.last_faults != faults;
    if changed {
        // The fault light belongs to the box, not the window: someone standing
        // at the bench with the app behind another window should still see that
        // this run is not to be trusted. Driven only on a change, because it is
        // a USB transaction and the poll runs four times a second.
        if poller.last_faults.is_clear() != faults.is_clear() {
            let state = if faults.is_clear() {
                LedState::Off
            } else {
                LedState::On
            };
            let _ = dev.instrument().set_led(LedId::Red, state);
        }
        poller.last_sense = Some(sense);
        poller.last_faults = faults;
        let _ = tx.send(InstEvent::Status {
            sense,
            faults,
            undecoded,
        });
    }
}

fn handle(
    cmd: InstCommand,
    listed: &mut Vec<(String, Candidate)>,
    connected: &mut Option<Connected>,
    poller: &mut Poller,
    tx: &Sender<InstEvent>,
) {
    match cmd {
        InstCommand::Shutdown => {}
        InstCommand::List => {
            *listed = enumerate();
            let _ = tx.send(InstEvent::Instruments(
                listed.iter().map(|(name, _)| name.clone()).collect(),
            ));
        }
        InstCommand::Connect(index) => {
            if listed.is_empty() {
                *listed = enumerate();
            }
            let Some((_, candidate)) = listed.get(index) else {
                let _ = tx.send(InstEvent::ConnectFailed("no such instrument".into()));
                return;
            };
            match connect(candidate) {
                Ok(dev) => {
                    let mut dev = dev;
                    let info = dev.instrument().info().clone();
                    let simulated = dev.is_simulated();
                    *poller = Poller {
                        watch_button: poller.watch_button,
                        ..Poller::new()
                    };
                    *connected = Some(dev);
                    let _ = tx.send(InstEvent::Connected { info, simulated });
                }
                Err(e) => {
                    let _ = tx.send(InstEvent::ConnectFailed(e.to_string()));
                }
            }
        }
        InstCommand::Disconnect => {
            if let Some(dev) = connected.as_mut() {
                let _ = dev.instrument().stop_acquire();
            }
            *connected = None;
            // Nothing is driving the simulated bench any more.
            opengel::simbench::clear();
            let _ = tx.send(InstEvent::Lamps(false));
            let _ = tx.send(InstEvent::Disconnected);
        }
        InstCommand::ClearFaults => {
            let Some(dev) = connected.as_mut() else {
                return;
            };
            // Deliberately *not* recording the cleared value here: the poll loop
            // only emits when something changed, so pre-empting it would leave
            // the UI showing a fault the user has already cleared.
            if let Err(e) = dev.instrument().clear_faults() {
                let _ = tx.send(InstEvent::Error(format!("Clearing faults failed: {e}")));
            }
        }
        InstCommand::WatchRunButton(watch) => poller.watch_button = watch,
        InstCommand::BeginRun { warmup_s } => {
            let Some(dev) = connected.as_mut() else {
                let _ = tx.send(InstEvent::RunRefused("No instrument connected.".into()));
                return;
            };
            begin_run(dev, warmup_s, poller, tx);
        }
        InstCommand::ActivateGel { seconds } => {
            let Some(dev) = connected.as_mut() else {
                let _ = tx.send(InstEvent::RunRefused("No instrument connected.".into()));
                return;
            };
            activate_gel(dev, seconds, poller, tx);
        }
        InstCommand::EndRun => {
            let Some(dev) = connected.as_mut() else {
                return;
            };
            poller.run_deadline = None;
            let _ = dev.instrument().set_led(LedId::Green, LedState::Off);
            if let Err(e) = dev.instrument().stop_acquire() {
                let _ = tx.send(InstEvent::Error(format!(
                    "Switching the lamps off failed: {e}"
                )));
            }
            let faults = dev.instrument().faults().unwrap_or(Faults::NONE);
            poller.last_faults = faults;
            let _ = tx.send(InstEvent::RunFinished {
                faults,
                door_violation: faults.door_opened_during_imaging(),
            });
        }
        InstCommand::Abort => {
            let Some(dev) = connected.as_mut() else {
                return;
            };
            poller.run_deadline = None;
            let _ = dev.instrument().set_led(LedId::Green, LedState::Off);
            let _ = dev.instrument().stop_acquire();
            let _ = tx.send(InstEvent::Lamps(false));
        }
        InstCommand::Illuminate(on) => {
            let Some(dev) = connected.as_mut() else {
                let _ = tx.send(InstEvent::Error("No instrument connected.".into()));
                return;
            };
            if on {
                // The door interlock is the instrument's own: a start with the
                // door open lights nothing and is refused, and the refusal is
                // the honest thing to show.
                match dev.instrument().start_acquire(false) {
                    Ok(()) => {
                        // Lit is lit: the panel light means the lamps are on,
                        // whether for a run or for framing.
                        let _ = dev.instrument().set_led(LedId::Green, LedState::On);
                        // Lamps lit for framing rather than for a run still get
                        // a deadline: 302 nm lamps have a finite life and a
                        // preview left running overnight would spend it.
                        poller.run_deadline = Some(Instant::now() + PREVIEW_LAMP_LIMIT);
                        let _ = tx.send(InstEvent::Lamps(true));
                    }
                    Err(e) => {
                        let _ = tx.send(InstEvent::Error(format!(
                            "The lamps could not be switched on: {e}"
                        )));
                        let _ = tx.send(InstEvent::Lamps(false));
                    }
                }
            } else {
                poller.run_deadline = None;
                let _ = dev.instrument().set_led(LedId::Green, LedState::Off);
                if let Err(e) = dev.instrument().stop_acquire() {
                    let _ = tx.send(InstEvent::Error(format!(
                        "Switching the lamps off failed: {e}"
                    )));
                }
                let _ = tx.send(InstEvent::Lamps(false));
            }
        }
        InstCommand::SimSetTray(tray) => {
            if let Some(sim) = connected.as_mut().and_then(Connected::sim) {
                sim.set_tray(tray);
            }
        }
        InstCommand::SimSetDoor(closed) => {
            if let Some(sim) = connected.as_mut().and_then(Connected::sim) {
                sim.set_door_closed(closed);
            }
        }
        InstCommand::SimSetFaults(faults) => {
            if let Some(sim) = connected.as_mut().and_then(Connected::sim) {
                sim.set_faults(Faults(faults));
            }
        }
        InstCommand::SimPressButton => {
            if let Some(sim) = connected.as_mut().and_then(Connected::sim) {
                sim.press_run_button();
            }
        }
    }
}

/// Light the lamps for a fixed time and put them out, taking no picture.
///
/// The stain-free activation. It shares every mechanism with a warm-up — same
/// lamps, same tray, same door watch — and differs only in why it is being
/// done, which is why it is not folded into a run: the chemistry happens once
/// per gel, and a run that activated every time would burn UV into a gel that
/// was already activated ten minutes ago.
fn activate_gel(dev: &mut Connected, seconds: f64, poller: &mut Poller, tx: &Sender<InstEvent>) {
    let inst = dev.instrument();

    match inst.tray_debounced(geldoc_ez::TRAY_SETTLE) {
        Ok(None) => {
            let _ = tx.send(InstEvent::RunRefused(
                "No sample tray is inserted — the tray is the light source.".into(),
            ));
            return;
        }
        Err(e) => {
            let _ = tx.send(InstEvent::RunRefused(format!(
                "Could not read the tray: {e}"
            )));
            return;
        }
        Ok(Some(_)) => {}
    }
    match inst.sense() {
        Ok(sense) if !sense.door_closed => {
            let _ = tx.send(InstEvent::RunRefused(
                "The door is open. Close it before activating — the lamps will not light otherwise."
                    .into(),
            ));
            return;
        }
        Err(e) => {
            let _ = tx.send(InstEvent::RunRefused(format!(
                "Could not read the instrument: {e}"
            )));
            return;
        }
        Ok(_) => {}
    }

    if let Err(e) = inst.start_acquire(true) {
        let _ = tx.send(InstEvent::RunRefused(format!(
            "The lamps did not come on: {e}"
        )));
        return;
    }
    // The lamps must not be left burning if the UI dies mid-activation.
    poller.run_deadline = Some(Instant::now() + RUN_WATCHDOG);
    let _ = inst.set_led(LedId::Green, LedState::On);

    let started = Instant::now();
    let total = Duration::from_secs_f64(seconds.max(0.0));
    while started.elapsed() < total {
        std::thread::sleep(ACTIVATION_TICK.min(total - started.elapsed()));
        // An activation interrupted halfway has not activated the gel, so say
        // so rather than letting the user believe it is ready.
        if let Ok(sense) = inst.sense() {
            if !sense.door_closed {
                let _ = inst.stop_acquire();
                let _ = inst.set_led(LedId::Green, LedState::Off);
                poller.run_deadline = None;
                let _ = tx.send(InstEvent::RunRefused(
                    "The door was opened during gel activation. The gel is not activated — \
                     close the door and activate it again."
                        .into(),
                ));
                return;
            }
        }
        let _ = tx.send(InstEvent::LampWindow {
            elapsed_s: started.elapsed().as_secs_f64().min(seconds),
            total_s: seconds,
            activating: true,
        });
    }

    poller.run_deadline = None;
    let _ = inst.set_led(LedId::Green, LedState::Off);
    if let Err(e) = inst.stop_acquire() {
        let _ = tx.send(InstEvent::Error(format!(
            "Switching the lamps off failed: {e}"
        )));
    }
    let _ = tx.send(InstEvent::ActivationFinished);
}

/// The interlocks, the activation step, and lighting the lamps.
///
/// The door check is deliberately ours as well as the hardware's. The lamps are
/// gated on the door sensor in the instrument, but relying on that alone would
/// mean asking for a UV exposure and finding out afterwards; refusing here also
/// lets us say *why*.
fn begin_run(dev: &mut Connected, warmup_s: f64, poller: &mut Poller, tx: &Sender<InstEvent>) {
    let inst = dev.instrument();

    let tray = match inst.tray_debounced(geldoc_ez::TRAY_SETTLE) {
        Ok(tray) => tray,
        Err(e) => {
            let _ = tx.send(InstEvent::RunRefused(format!(
                "Could not read the tray: {e}"
            )));
            return;
        }
    };
    if tray.is_none() {
        let _ = tx.send(InstEvent::RunRefused(
            "No sample tray is inserted. Slide one in until the magnet holds it.".into(),
        ));
        return;
    }

    let sense = match inst.sense() {
        Ok(sense) => sense,
        Err(e) => {
            let _ = tx.send(InstEvent::RunRefused(format!(
                "Could not read the instrument: {e}"
            )));
            return;
        }
    };
    if !sense.door_closed {
        let _ = tx.send(InstEvent::RunRefused(
            "The door is open. Close it before imaging — the lamps will not light otherwise."
                .into(),
        ));
        return;
    }

    // Any latched fault from a previous run would be read back at the end of
    // this one and blamed on it. Clear first so what we report afterwards is
    // about this exposure.
    if let Err(e) = inst.clear_faults() {
        let _ = tx.send(InstEvent::Error(format!("Clearing old faults failed: {e}")));
    }

    // One lamp window before the exposure, not two.
    //
    // A stain-free activation is a UV pre-exposure that drives the gel's
    // chemistry; a warm-up lets the lamps settle. Both are "lamps on, wait,
    // do not expose yet", on the same lamps through the same tray — so they are
    // one wait, as long as the longer of the two asks for.
    //
    // Not the sum: once the gel has had its 45 seconds the chemistry is done,
    // and further UV is dose nobody asked for. And emphatically not two windows
    // with the lamps cycled between them — that was the old shape, and it threw
    // away the very thing the warm-up exists to establish, re-striking lamps
    // that forty-five seconds of activation had already brought to temperature.
    // Lights on for the exposure, waiting for the lamps to stabilise.
    if let Err(e) = inst.start_acquire(true) {
        let _ = tx.send(InstEvent::RunRefused(format!(
            "The lamps did not come on: {e}"
        )));
        return;
    }
    poller.run_deadline = Some(Instant::now() + RUN_WATCHDOG);
    // The front panel says what the box is doing, for anyone looking at the box
    // rather than the screen.
    let _ = inst.set_led(LedId::Green, LedState::On);

    // The instrument's busy bit clears when the lamps *strike*; they go on
    // brightening for seconds after that, so an exposure taken immediately sits
    // somewhere on the ramp and is not comparable with the next one. Waited
    // through once per acquisition, with the lamps left burning: an HDR bracket
    // shoots every frame after this single wait, which is the point —
    // re-striking between frames would put each exposure at a different place
    // on the same ramp.
    if warmup_s > 0.0 {
        let started = Instant::now();
        let total = Duration::from_secs_f64(warmup_s);
        while started.elapsed() < total {
            std::thread::sleep(ACTIVATION_TICK.min(total - started.elapsed()));
            // The door is watched throughout: it kills the light, so anything
            // exposed afterwards is dark — and an activation interrupted
            // halfway has not activated the gel.
            if let Ok(sense) = inst.sense() {
                if !sense.door_closed {
                    let _ = inst.stop_acquire();
                    let _ = inst.set_led(LedId::Green, LedState::Off);
                    let _ = tx.send(InstEvent::RunRefused(
                        "The door was opened while the lamps warmed up. The run was abandoned."
                            .into(),
                    ));
                    return;
                }
            }
            // Silence here reads as a hung program: from the outside this is
            // just lamps on and nothing happening.
            let _ = tx.send(InstEvent::LampWindow {
                elapsed_s: started.elapsed().as_secs_f64().min(warmup_s),
                total_s: warmup_s,
                activating: false,
            });
        }
    }
    let _ = tx.send(InstEvent::LightsReady);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::RecvTimeoutError;

    const WAIT: Duration = Duration::from_secs(5);

    /// Connect the worker to the simulated enclosure. It is always offered, and
    /// always last, so its index is the end of the list.
    fn connect_to_simulator() -> (InstrumentHandle, Receiver<InstEvent>) {
        let (handle, events) = spawn();
        handle.list();
        let index = loop {
            match events.recv_timeout(WAIT).expect("instrument list") {
                InstEvent::Instruments(names) => {
                    assert!(!names.is_empty(), "the simulator is always offered");
                    break names.len() - 1;
                }
                _ => continue,
            }
        };
        handle.connect(index);
        loop {
            match events.recv_timeout(WAIT).expect("connect result") {
                InstEvent::Connected { simulated, .. } => {
                    assert!(simulated, "expected the simulated enclosure");
                    break;
                }
                InstEvent::ConnectFailed(e) => panic!("connect failed: {e}"),
                _ => continue,
            }
        }
        (handle, events)
    }

    /// Drain events until one matches, or fail.
    fn wait_for<T>(
        events: &Receiver<InstEvent>,
        what: &str,
        mut f: impl FnMut(&InstEvent) -> Option<T>,
    ) -> T {
        let deadline = Instant::now() + WAIT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match events.recv_timeout(remaining) {
                Ok(evt) => {
                    if let Some(value) = f(&evt) {
                        return value;
                    }
                }
                Err(RecvTimeoutError::Timeout) => panic!("timed out waiting for {what}"),
                Err(RecvTimeoutError::Disconnected) => panic!("worker died waiting for {what}"),
            }
        }
    }

    #[test]
    fn a_run_with_the_door_open_is_refused_before_anything_is_exposed() {
        // The interlock, end to end. The hardware gates the lamps too, but
        // refusing here is what lets the user be told why.
        let (handle, events) = connect_to_simulator();
        handle.sim_set_door(false);
        handle.begin_run(0.0);
        let reason = wait_for(&events, "a refusal", |evt| match evt {
            InstEvent::RunRefused(reason) => Some(reason.clone()),
            InstEvent::LightsReady => panic!("the lamps must not light with the door open"),
            _ => None,
        });
        assert!(reason.contains("door"), "{reason}");
    }

    #[test]
    fn a_run_with_no_tray_is_refused() {
        let (handle, events) = connect_to_simulator();
        handle.sim_set_tray(None);
        handle.begin_run(0.0);
        let reason = wait_for(&events, "a refusal", |evt| match evt {
            InstEvent::RunRefused(reason) => Some(reason.clone()),
            InstEvent::LightsReady => panic!("the lamps must not light with no tray"),
            _ => None,
        });
        assert!(reason.contains("tray"), "{reason}");
    }

    #[test]
    fn a_clean_run_lights_the_lamps_and_finishes_without_faults() {
        let (handle, events) = connect_to_simulator();
        handle.sim_set_tray(Some(TrayType::StainFree));
        handle.sim_set_door(true);
        handle.begin_run(0.0);
        wait_for(&events, "lights ready", |evt| {
            matches!(evt, InstEvent::LightsReady).then_some(())
        });
        handle.end_run();
        let (faults, violation) = wait_for(&events, "the run to finish", |evt| match evt {
            InstEvent::RunFinished {
                faults,
                door_violation,
            } => Some((*faults, *door_violation)),
            _ => None,
        });
        assert!(faults.is_clear(), "unexpected faults: {faults:?}");
        assert!(!violation);
    }

    #[test]
    fn opening_the_door_mid_exposure_reports_a_violation() {
        // The whole reason frames are held rather than adopted: this run's image
        // is not data, and the run must say so.
        let (handle, events) = connect_to_simulator();
        handle.sim_set_tray(Some(TrayType::StainFree));
        handle.sim_set_door(true);
        handle.begin_run(0.0);
        wait_for(&events, "lights ready", |evt| {
            matches!(evt, InstEvent::LightsReady).then_some(())
        });
        // The exposure would be happening here.
        handle.sim_set_door(false);
        handle.end_run();
        let violation = wait_for(&events, "the run to finish", |evt| match evt {
            InstEvent::RunFinished { door_violation, .. } => Some(*door_violation),
            _ => None,
        });
        assert!(violation, "an interrupted exposure must be flagged");
    }

    #[test]
    fn activating_the_gel_is_its_own_act_and_takes_no_picture() {
        // A run must never activate by itself: the reaction happens once per
        // gel, and nothing the instrument reads can tell an activated gel from
        // a fresh one, so imaging twice would dose it twice.
        let (handle, events) = connect_to_simulator();
        handle.sim_set_tray(Some(TrayType::StainFree));
        handle.sim_set_door(true);
        handle.activate_gel(0.4);

        // Progress must be reported, or a 45-second activation looks like a hang.
        let total = wait_for(&events, "activation progress", |evt| match evt {
            InstEvent::LampWindow {
                total_s,
                activating: true,
                ..
            } => Some(*total_s),
            InstEvent::LightsReady => panic!("activation must not start an exposure"),
            _ => None,
        });
        assert!((total - 0.4).abs() < 1e-9);

        // It ends by itself, with the lamps out and no frame requested.
        wait_for(&events, "the activation to finish", |evt| {
            matches!(evt, InstEvent::ActivationFinished).then_some(())
        });
    }

    #[test]
    fn a_run_warms_up_without_activating() {
        // Same tray, but a *run*: the lamps settle and the exposure is allowed
        // to start, and nothing reports itself as an activation.
        let (handle, events) = connect_to_simulator();
        handle.sim_set_tray(Some(TrayType::StainFree));
        handle.sim_set_door(true);
        handle.begin_run(0.2);
        wait_for(&events, "lights ready", |evt| match evt {
            InstEvent::LampWindow {
                activating: true, ..
            } => panic!("a run must not activate the gel"),
            InstEvent::LightsReady => Some(()),
            _ => None,
        });
        handle.end_run();
    }

    #[test]
    fn the_lamps_warm_up_before_the_exposure_is_allowed_to_start() {
        // The instrument's busy bit clears when the lamps strike, not when they
        // are stable. Exposing on the ramp is the error the wait exists to
        // prevent, so `LightsReady` must not arrive until it is over.
        let (handle, events) = connect_to_simulator();
        handle.sim_set_tray(Some(TrayType::Uv));
        handle.sim_set_door(true);
        let started = Instant::now();
        handle.begin_run(0.4);

        // Progress is reported throughout: an unexplained pause with the lamps
        // lit reads as a hung program.
        let total = wait_for(&events, "warm-up progress", |evt| match evt {
            InstEvent::LampWindow {
                total_s,
                activating: false,
                ..
            } => Some(*total_s),
            InstEvent::LightsReady => panic!("the exposure started before the lamps had settled"),
            _ => None,
        });
        assert!((total - 0.4).abs() < 1e-9, "got {total}");

        wait_for(&events, "lights ready", |evt| {
            matches!(evt, InstEvent::LightsReady).then_some(())
        });
        assert!(
            started.elapsed() >= Duration::from_secs_f64(0.4),
            "the wait was skipped: {:?}",
            started.elapsed()
        );
        handle.end_run();
    }

    #[test]
    fn faults_are_reported_and_can_be_cleared() {
        let (handle, events) = connect_to_simulator();
        handle.sim_set_faults(0x08); // lamp bank 1
        let faults = wait_for(&events, "a fault report", |evt| match evt {
            InstEvent::Status { faults, .. } if !faults.is_clear() => Some(*faults),
            _ => None,
        });
        assert!(faults.lamp_bank_1());
        handle.clear_faults();
        wait_for(&events, "the faults to clear", |evt| match evt {
            InstEvent::Status { faults, .. } if faults.is_clear() => Some(()),
            _ => None,
        });
    }

    /// The USB ids `enumerate` matches on, reached exactly as the Linux-only
    /// branch reaches them.
    ///
    /// That branch compiles on one platform and is developed on another, so
    /// nothing else here would notice its names going stale — which is how the
    /// import backing it was once tidied away as unused, leaving a build that
    /// was green on macOS and broken on Linux. This fails everywhere instead.
    #[test]
    fn the_enclosure_usb_ids_resolve_through_the_module() {
        assert_eq!(geldoc_ez::VENDOR_ID, 0x0614);
        // As a *pattern*, which is where a stale path is quietly dangerous: a
        // bare name that is no longer a constant in scope binds anything and
        // matches everything, rather than failing to compile.
        for (product_id, expected) in [
            (geldoc_ez::PRODUCT_ID_GEL_DOC_EZ, "Gel Doc EZ"),
            (
                geldoc_ez::PRODUCT_ID_CRITERION_STAIN_FREE,
                "Criterion Stain Free",
            ),
            (0xffff, "Bio-Rad enclosure"),
        ] {
            let model = match product_id {
                geldoc_ez::PRODUCT_ID_GEL_DOC_EZ => "Gel Doc EZ",
                geldoc_ez::PRODUCT_ID_CRITERION_STAIN_FREE => "Criterion Stain Free",
                _ => "Bio-Rad enclosure",
            };
            assert_eq!(model, expected, "product id 0x{product_id:04x}");
        }
    }
}
