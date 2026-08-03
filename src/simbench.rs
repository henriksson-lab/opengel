//! The simulated bench: one virtual gel, lit by the simulated enclosure and
//! photographed by the mock camera.
//!
//! The two simulators are otherwise unaware of each other — one speaks the
//! enclosure's wire protocol, the other is a camera backend — but a darkroom
//! where the picture never changes when the lamps do is not a useful darkroom.
//! This is the one thing they share: which light source is currently burning.
//!
//! Kept deliberately tiny (one atomic, no locks) because it is written from the
//! instrument worker thread and read from the camera worker thread on every
//! preview frame.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use crate::instrument::TrayType;

/// The light currently on the virtual gel: `0` = no enclosure has said anything
/// yet, `1` = darkness, otherwise two more than the tray's position in
/// [`TrayType::ALL`].
///
/// "Nothing has said anything" is deliberately distinct from "dark": with no
/// simulated enclosure in the picture the mock camera is standing in for a plain
/// camera, and must show a gel rather than an unexplained black frame.
static LIGHT: AtomicU8 = AtomicU8::new(0);

/// Whether the bench link is live. Off until the application switches it on.
///
/// Without this, any test that happens to connect a simulated enclosure would
/// darken the mock camera for every other test in the process — the link is
/// process-wide by nature, so it is the application, not a library call, that
/// decides it is in force.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Let the simulated enclosure drive what the mock camera sees. Called once by
/// the application at startup.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

/// Publish the enclosure's illumination state. `tray` is what is inserted;
/// `lamps_on` whether an acquisition is lighting it. With no tray there is no
/// light source, so the gel is dark whatever the lamps are told to do.
pub fn set_light(tray: Option<TrayType>, lamps_on: bool) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let code = match (tray, lamps_on) {
        (Some(tray), true) => TrayType::ALL
            .iter()
            .position(|&t| t == tray)
            .map(|i| i as u8 + 2)
            .unwrap_or(1),
        _ => 1,
    };
    LIGHT.store(code, Ordering::Relaxed);
}

/// Forget everything an enclosure published — back to "nobody is driving the
/// bench". Used when the simulator disconnects, and by tests.
pub fn clear() {
    LIGHT.store(0, Ordering::Relaxed);
}

/// What is on the virtual gel right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchLight {
    /// No simulated enclosure is driving the bench.
    Unset,
    /// An enclosure is driving it, and the gel is in the dark.
    Dark,
    Lit(TrayType),
}

pub fn light() -> BenchLight {
    match LIGHT.load(Ordering::Relaxed) {
        0 => BenchLight::Unset,
        1 => BenchLight::Dark,
        code => TrayType::ALL
            .get(code as usize - 2)
            .copied()
            .map(BenchLight::Lit)
            .unwrap_or(BenchLight::Dark),
    }
}
