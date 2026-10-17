//! Keeps live speech recognition real time when the language model is working at the same time.
//!
//! Whisper and the language model each run a pool of busy-waiting threads. On a phone, with few cores,
//! two such pools running together do not merely share the processor: each one's barriers wait
//! on threads the other has descheduled, and both slow down many times over. A one second snippet
//! of speech can take half a minute to transcribe. So speech gets the processor first, and the
//! language model yields between its steps while any speech is being decoded.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

static ENABLED: AtomicBool = AtomicBool::new(false);
static SPEECH: AtomicUsize = AtomicUsize::new(0);

pub fn enable(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// True on a phone-style setup, where live speech also uses a trimmed audio window.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Held by the speech recognizer while it decodes.
pub struct Speech;

impl Speech {
    pub fn begin() -> Self {
        SPEECH.fetch_add(1, Ordering::Relaxed);
        Speech
    }
}

impl Drop for Speech {
    fn drop(&mut self) {
        SPEECH.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Called by the language model before each decode step: waits while speech is being decoded
/// (at most a few seconds, so it can never stall for good).
pub fn yield_to_speech() {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let mut waited = 0;
    while SPEECH.load(Ordering::Relaxed) > 0 && waited < 1500 {
        std::thread::sleep(Duration::from_millis(2));
        waited += 1;
    }
}
