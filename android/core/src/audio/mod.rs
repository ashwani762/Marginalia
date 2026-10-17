//! Microphone capture happens in Kotlin (AudioRecord); only the decoder and the
//! voice-activity segmenter are shared with the desktop build.

#[path = "../../../../src/audio/decode.rs"]
pub mod decode;
#[path = "../../../../src/audio/vad.rs"]
pub mod vad;
