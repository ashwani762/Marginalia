//! Carrying on an answer that was cut off: the request that asks for the rest, and the filter that joins the new words
//! onto the words already shown. The phone and a node that forwards jobs both use it.

use crate::msg::{ChatMsg, Role};

/// What the model is told when it has to carry on an answer another node began.
pub const CONTINUE_PROMPT: &str = "Your answer was cut off. Carry on from exactly where it stopped, without repeating anything and without a new introduction.";

/// The same chat with the answer so far added, asking for the rest.
pub fn continuation(messages: &[ChatMsg], partial: &str) -> Vec<ChatMsg> {
    let mut m = messages.to_vec();
    m.push(ChatMsg { role: Role::Assistant, content: partial.to_string() });
    m.push(ChatMsg { role: Role::User, content: CONTINUE_PROMPT.to_string() });
    m
}

/// Joins a resumed answer onto the words already shown. A model asked to carry on tends to say the last
/// sentence again, so the first words of the new text are held back until it is clear whether they repeat
/// the end of what was shown, and any repeat is dropped.
pub struct SeamFilter {
    tail: String,
    buf: String,
    released: bool,
}

impl SeamFilter {
    const HOLD: usize = 100;
    const MIN_OVERLAP: usize = 12;

    pub fn new(shown: &str) -> SeamFilter {
        let start = shown.char_indices().rev().nth(239).map(|(i, _)| i).unwrap_or(0);
        SeamFilter { tail: shown[start..].to_string(), buf: String::new(), released: false }
    }

    pub fn push(&mut self, text: &str, emit: &mut dyn FnMut(&str)) {
        if self.released {
            emit(text);
            return;
        }
        self.buf.push_str(text);
        if self.buf.chars().count() >= Self::HOLD {
            self.release(emit);
        }
    }

    pub fn finish(&mut self, emit: &mut dyn FnMut(&str)) {
        if !self.released {
            self.release(emit);
        }
    }

    fn release(&mut self, emit: &mut dyn FnMut(&str)) {
        self.released = true;
        let buf = std::mem::take(&mut self.buf);
        let mut rest = buf.as_str();
        // The longest end of what was shown that the new text starts with.
        let starts: Vec<usize> = self.tail.char_indices().map(|(i, _)| i).collect();
        for &i in &starts {
            let suffix = &self.tail[i..];
            if suffix.chars().count() < Self::MIN_OVERLAP {
                break;
            }
            if rest.starts_with(suffix) {
                rest = &rest[suffix.len()..];
                break;
            }
        }
        if rest.is_empty() {
            return;
        }
        // The shown text may stop in the middle of a word or right after a sentence: keep words apart.
        let last = self.tail.chars().last();
        let first = rest.chars().next();
        if let (Some(l), Some(f)) = (last, first) {
            let needs_space = !l.is_whitespace() && !f.is_whitespace() && f.is_alphanumeric() && (".!?:;,)\"".contains(l) || (l.is_alphanumeric() && f.is_uppercase()));
            if needs_space {
                emit(" ");
            }
        }
        emit(rest);
    }
}
