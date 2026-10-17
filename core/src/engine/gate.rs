//! A single switch for the phone-style speech setup.
//!
//! When it is on, speech recognition turns on flash attention, trims the audio window for live
//! snippets and keeps a vote on the detected language, so transcription stays real time on a phone.

use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);

pub fn enable(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// True on a phone-style setup, where live speech also uses a trimmed audio window.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}
