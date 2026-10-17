//! Text helpers: question detection, de-duplication, and tidying of model output.

use std::collections::HashSet;

pub fn ends_sentence(text: &str) -> bool {
    let t = text.trim_end();
    t.ends_with('.') || t.ends_with('?') || t.ends_with('!')
}

pub fn is_question(text: &str) -> bool {
    let t = text.trim();
    let words = t.split_whitespace().count();
    if words < 4 {
        return false;
    }
    if t.ends_with('?') {
        return true;
    }
    let lower = t.to_lowercase();
    const STARTS: [&str; 16] = [
        "what is",
        "what's",
        "what are",
        "how do",
        "how does",
        "how much",
        "how many",
        "do we have",
        "does anyone know",
        "where is",
        "when did",
        "when is",
        "who is",
        "can someone",
        "is there",
        "did we",
    ];
    STARTS.iter().any(|s| lower.starts_with(s))
}

pub fn word_set(s: &str) -> HashSet<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .map(|w| w.to_lowercase())
        .collect()
}

pub fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    let inter = a.intersection(b).count() as f32;
    let union = a.union(b).count() as f32;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

pub fn parse_questions(raw: &str) -> Vec<String> {
    if let Some(start) = raw.find('[') {
        if let Some(end) = raw[start..].rfind(']') {
            if let Ok(v) = serde_json::from_str::<Vec<String>>(&raw[start..=start + end]) {
                return v
                    .into_iter()
                    .map(|s| s.trim().to_string())
                    .filter(|s| s.len() > 8)
                    .collect();
            }
        }
    }
    // Fallback: quoted strings or bullet lines.
    let mut out = Vec::new();
    for cap in regex::Regex::new(r#""([^"]{8,200})""#)
        .unwrap()
        .captures_iter(raw)
    {
        out.push(cap[1].to_string());
    }
    if out.is_empty() {
        for line in raw.lines() {
            let l = line.trim().trim_start_matches(['-', '*', '•']).trim();
            if l.len() > 8 && l.ends_with('?') {
                out.push(l.to_string());
            }
        }
    }
    out
}

pub const ENHANCE_SYSTEM: &str = "You are Marginalia, an on-device meeting assistant. The user types shorthand notes during a live meeting. Rewrite their notes into clear, complete meeting notes in Markdown. Rules: keep the user's structure, order and intent; expand abbreviations and fragments using the transcript for context; add names, numbers, decisions and action items that the transcript confirms; never invent facts that are in neither the notes nor the transcript; do not summarize the whole meeting, only enrich what the user wrote; no preamble, output only the notes.";

pub const SCAN_SYSTEM: &str = "You extract questions from a live meeting transcript. Return a JSON array of concise, self-contained questions or lookups that someone would answer from documents (specs, policies, contracts, pricing, past meetings). Rephrase fragments into full questions. Ignore small talk, rhetorical questions, questions already answered in the window, questions the speaker asks the audience for a show of hands or a poll (how many of you, who here), and questions about the personal characteristics of the people present. Speech recognition makes mistakes, so when a question sounds odd or out of place for the talk, leave it out. Return [] if there are none. Output only JSON.";

pub const CARD_SYSTEM: &str = "You answer a question raised during a live meeting. Use what was said in the meeting first (the transcript excerpts), and the documents only when they clearly help with this exact question. Reply in at most 60 words, plain and direct. Start with the answer itself and never with the source: do not begin with 'The transcript', 'The meeting', 'The speaker' or 'According to'. Cite a transcript excerpt by its time like [12:03] and a document by its number like [1]. If neither answers the question, reply exactly 'Not answered yet.' Output only the answer.";

pub const TITLE_SYSTEM: &str = "Write a short descriptive title for this meeting, 3 to 7 words, in sentence case (capitalise only the first word and proper nouns). No quotes, no trailing punctuation, no date, no prefix like 'Title:'. Output only the title.";


pub const SUMMARY_SYSTEM: &str = "You are Marginalia, an on-device meeting note-taker. Write the notes in Markdown. Begin directly with two to four plain sentences saying what the meeting was about and what was concluded: no heading before them. Then add only the sections that have real content, each as a '## ' heading with bullets: 'Decisions', 'Action items' (owner first when known), 'Open questions'. Never write a section that would say None or N/A. Never mention 'the transcript', 'the recording' or 'the provided text'; write as the person who took the notes. Base everything on the material; do not invent. If there is too little substance, write one sentence saying so.";

/// Drop sentences that talk about the input instead of the meeting, e.g.
/// "No decisions were made based on the provided material."
pub fn strip_meta(text: &str) -> String {
    const META: [&str; 6] = [
        "provided material",
        "provided transcript",
        "provided text",
        "the transcript",
        "the recording",
        "the material provided",
    ];
    let mut out = Vec::new();
    for line in text.lines() {
        let mut kept = String::new();
        for sentence in line.split_inclusive(['.', '!', '?']) {
            let lower = sentence.to_ascii_lowercase();
            let is_meta = META.iter().any(|m| lower.contains(m));
            let is_negative = lower.trim_start().starts_with("no ") || lower.contains("were made") || lower.contains("was made");
            if is_meta && is_negative {
                continue;
            }
            kept.push_str(sentence);
        }
        let kept = kept
            .replace("The provided transcript", "The discussion")
            .replace("the provided transcript", "the discussion")
            .replace("The transcript", "The discussion")
            .replace("the transcript", "the discussion");
        if !kept.trim().is_empty() || line.trim().is_empty() {
            out.push(kept.trim_end().to_string());
        }
    }
    out.join("\n")
}

/// How an answer must not begin: the model likes to open by naming its source ("The transcript mentions that ...")
/// instead of just answering.
const LEAD_INS: [&str; 22] = [
    "the transcript mentions that ", "the transcript states that ", "the transcript says that ", "the transcript indicates that ",
    "the transcript mentions ", "the transcript states ", "the transcript says ", "the transcript indicates ",
    "the meeting mentioned that ", "the meeting mentions that ", "the meeting noted that ", "the meeting discussed that ",
    "the speaker mentioned that ", "the speaker says that ", "the speaker states that ", "the speaker explained that ",
    "according to the transcript, ", "according to the meeting, ", "based on the transcript, ", "based on the meeting, ",
    "in the transcript, ", "from the transcript, ",
];

/// An answer for a card without its opening attribution, capitalised again. Used on what the person sees and what is saved,
/// also on a half written answer: while the start could still turn into an attribution nothing is shown yet.
pub fn card_answer(raw: &str) -> String {
    let t = raw.trim_start();
    let lower = t.to_lowercase();
    for lead in LEAD_INS {
        if lower.starts_with(lead) {
            let rest = t[lead.len()..].trim_start();
            return capitalise(rest);
        }
    }
    // Still being written: "The transcr" may become "The transcript mentions that ...", so wait for more words.
    if !lower.is_empty() && LEAD_INS.iter().any(|lead| lead.starts_with(lower.as_str())) {
        return String::new();
    }
    t.to_string()
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Remove the model's habits that add noise: a redundant leading "Summary" heading
/// and sections that only say "none".
pub fn clean_summary(md: &str) -> String {
    let mut blocks: Vec<(Option<String>, Vec<String>)> = vec![(None, Vec::new())];
    for line in md.lines() {
        if let Some(h) = line.trim_start().strip_prefix("## ") {
            blocks.push((Some(h.trim().to_string()), Vec::new()));
        } else {
            blocks.last_mut().unwrap().1.push(line.to_string());
        }
    }
    let is_none = |l: &str| {
        let t = l.trim().trim_start_matches(['-', '*', '•']).trim().to_ascii_lowercase();
        t.is_empty()
            || t.starts_with("none")
            || t.starts_with("n/a")
            || t.starts_with("no specific")
            || t.starts_with("no action")
            || t.starts_with("no open")
            || t.starts_with("no decisions")
            || t.starts_with("nothing")
    };
    let mut out = String::new();
    let blocks: Vec<(Option<String>, Vec<String>)> = blocks
        .into_iter()
        .map(|(h, l)| (h, strip_meta(&l.join("\n")).lines().map(String::from).collect()))
        .collect();
    for (i, (heading, lines)) in blocks.iter().enumerate() {
        let body: Vec<&String> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
        let empty = body.is_empty() || body.iter().all(|l| is_none(l));
        match heading {
            // A "Summary" heading above the lead paragraph just repeats the card title.
            Some(h) if h.eq_ignore_ascii_case("summary") => {
                if !empty {
                    out.push_str(&lines.join("\n").trim_matches('\n').to_string());
                    out.push_str("\n\n");
                }
            }
            Some(h) => {
                if !empty {
                    out.push_str(&format!("## {h}\n{}\n\n", lines.join("\n").trim_matches('\n')));
                }
            }
            None => {
                if i == 0 && !empty {
                    out.push_str(&lines.join("\n").trim_matches('\n').to_string());
                    out.push_str("\n\n");
                }
            }
        }
    }
    out.trim().to_string()
}

/// First line, no quotes/markup/trailing punctuation, at most ~60 characters.
pub fn clean_title(raw: &str) -> String {
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let line = line
        .trim_start_matches(['#', '*', '-', '>', ' '])
        .trim_start_matches("Title:")
        .trim()
        .trim_matches(['"', '\'', '“', '”', '*', '`'])
        .trim_end_matches(['.', ':', ';'])
        .trim();
    if line.chars().count() <= 60 {
        return line.to_string();
    }
    let mut cut: String = line.chars().take(60).collect();
    if let Some(i) = cut.rfind(' ') {
        cut.truncate(i);
    }
    cut
}


pub const CHAT_SYSTEM: &str = "You are Marginalia, a private on-device assistant. Answer the user's question using the provided passages from their documents and meeting transcripts. Cite passages inline with their number in square brackets, like [2], right after the sentence they support. If the passages are insufficient, say what is missing instead of guessing, and answer from general knowledge only when clearly labeled as such. Be concise and well formatted in Markdown.";

pub const SECTION_SYSTEM: &str = "You are taking notes on one part of a longer meeting. Write dense Markdown bullet notes in this priority order: (1) decisions, (2) action items with the owner first and any deadline, (3) dates, numbers, names and amounts, (4) open questions, (5) everything else in a few words. Never skip a decision, action item, date, amount or name, even when it comes in the last lines. Compress repeated status chatter into one bullet. At most 170 words. No preamble and no title. Do not invent anything.";

pub const MERGE_SYSTEM: &str = "You combine notes taken on consecutive parts of one meeting into a shorter set of notes in the same bullet style. Never drop a decision, action item, owner, deadline, date, amount or name: compress routine chatter instead. Keep the order of the parts. At most 280 words. No preamble and no title.";

/// Rough token count without a tokenizer: about 3.3 characters per token for plain Latin text,
/// about one per character for other scripts. Slightly pessimistic on purpose, because running
/// over the context window silently drops text.
pub fn est_tokens(s: &str) -> usize {
    let (mut ascii, mut other) = (0usize, 0usize);
    for c in s.chars() {
        if c.is_ascii() { ascii += 1 } else { other += 1 }
    }
    (ascii as f32 / 3.3 + other as f32 * 0.95).ceil() as usize + 1
}

/// The last `max_tokens` worth of `s`, starting on a line boundary.
pub fn tail_tokens(s: &str, max_tokens: usize) -> String {
    if est_tokens(s) <= max_tokens {
        return s.to_string();
    }
    let mut start = s.len();
    let mut acc = 0usize;
    for (i, line) in s.rmatch_indices('\n') {
        let _ = line;
        let seg = &s[i..start];
        acc += est_tokens(seg);
        if acc > max_tokens {
            break;
        }
        start = i;
    }
    let mut cut = start;
    if cut == s.len() {
        // A single enormous line: cut by characters.
        let keep = (max_tokens as f32 * 3.0) as usize;
        cut = s.len().saturating_sub(keep);
        while !s.is_char_boundary(cut) {
            cut += 1;
        }
    }
    s[cut..].trim_start_matches('\n').to_string()
}

/// The first `max_tokens` worth of `s`, ending on a line boundary.
pub fn head_tokens(s: &str, max_tokens: usize) -> String {
    if est_tokens(s) <= max_tokens {
        return s.to_string();
    }
    let keep = (max_tokens as f32 * 3.0) as usize;
    let mut cut = keep.min(s.len());
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    match s[..cut].rfind('\n') {
        Some(i) if i > cut / 2 => s[..i].to_string(),
        _ => s[..cut].to_string(),
    }
}

pub const JOT_SYSTEM: &str = "You expand a short jot, written quickly during a meeting, into one clear note of at most 35 words. The note is about what the jot says, nothing else. Use the transcript only to fill in missing detail about the jot's own subject: names, numbers, dates, who owns it, what was decided. Never change the subject, never replace the jot with something else the transcript says, and never add anything the transcript does not support. If the transcript has nothing on the jot's subject, only tidy the wording. Write plain text with no preamble, no quotes and no bullet.";

#[cfg(test)]
mod card_answer_tests {
    use super::card_answer;

    #[test]
    fn an_opening_that_names_the_source_is_dropped() {
        assert_eq!(card_answer("The transcript mentions that runways need one plane at a time [00:17]."), "Runways need one plane at a time [00:17].");
        assert_eq!(card_answer("The meeting mentioned that each flight needs a runway."), "Each flight needs a runway.");
        assert_eq!(card_answer("According to the transcript, the limit is 500."), "The limit is 500.");
        assert_eq!(card_answer("Based on the meeting, we ship Friday."), "We ship Friday.");
    }

    #[test]
    fn an_answer_that_starts_with_the_answer_is_left_alone() {
        assert_eq!(card_answer("More than half of all traffic goes through 30 airports."), "More than half of all traffic goes through 30 airports.");
        assert_eq!(card_answer("The transcript was cut off at 10 minutes."), "The transcript was cut off at 10 minutes.");
    }

    #[test]
    fn a_half_written_opening_waits_instead_of_flashing() {
        assert_eq!(card_answer("The transcr"), "");
        assert_eq!(card_answer("According to the"), "");
        assert_eq!(card_answer(""), "");
        assert_eq!(card_answer("The tran"), "");
        assert_eq!(card_answer("The team agreed"), "The team agreed");
    }
}
