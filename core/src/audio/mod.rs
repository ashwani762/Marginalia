//! Microphone capture happens in Kotlin (AudioRecord); only the decoder and the
//! voice-activity segmenter live here.

pub mod decode;
pub mod vad;
