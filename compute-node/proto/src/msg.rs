use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMsg {
    pub role: Role,
    pub content: String,
}

/// How urgent a job is. The node serves these in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Priority {
    /// Someone is waiting on the screen: Ask answers, live help.
    Interactive,
    /// Notes and write-ups.
    Background,
    /// Indexing documents.
    Bulk,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Request {
    /// Who are you and how busy are you. Answered for any paired phone.
    Status,
    /// First contact. `proof` shows that the sender knows the code the node printed (see [`crate::pair`]).
    Pair { name: String, proof: [u8; 32] },
    Chat(ChatRequest),
    Embed(EmbedRequest),
    /// Speech to text on recorded audio. After this frame the phone keeps sending [`AudioFrame`]s on the same
    /// stream until [`AudioFrame::End`], and the node answers with [`TranscribeEvent`]s.
    Transcribe(TranscribeRequest),
    /// A phone that is not paired yet asks to be let in. The person has already said on the phone
    /// that they trust this compute node.
    Knock { name: String },
    /// Asks how a knock went. Answered for phones that are waiting or already let in.
    KnockStatus,
    /// A paired phone answers another phone's knock. `who` starts with the waiting phone's id.
    Decide { who: String, allow: bool },
    /// Sent by a compute node right after it paired with this one (as a phone would): it says it is a compute
    /// node, not a phone, and where it listens. `proof` is made with [`crate::pair::Side::Peer`], so only
    /// whoever knows the code can turn a pairing into a peer. Answered with [`PeerReply`].
    Join { name: String, port: u16, proof: [u8; 32] },
    /// A peer sends the phone records it holds and asks for this node's. `port` is where the sender listens, so
    /// a peer that moved is found again. Only answered for peers. Answered with [`RecordsReply`].
    Records { port: u16, records: Vec<PhoneRecord> },
    /// A peer says it leaves the cluster and asks to be forgotten. Only answered for peers. Answered with [`PeerReply`].
    Leave,
    /// Reads a photo and says what it shows. Only sent to a node whose status says `vision`. Answered with [`ChatEvent`]s
    /// (the description arrives as tokens, then `Done`). Nothing is kept on the node.
    Describe(DescribeRequest),
    /// Tells speakers apart on a recording. After this frame the phone sends [`AudioFrame`]s on the same stream
    /// until [`AudioFrame::End`], and the node answers with [`DiarizeEvent`]s. Only sent to a node whose status says `speakers`.
    Diarize(DiarizeRequest),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiarizeRequest {
    /// How many people spoke, or 0 to let the node decide.
    pub people: u32,
    /// How different two voices must be to count as two people when `people` is 0. Higher merges more. 0 means the node's own choice.
    pub threshold: f32,
    /// Length of the audio that follows, in milliseconds.
    pub total_ms: u64,
    pub priority: Priority,
}

/// Who spoke when, in milliseconds from the start of the audio. `speaker` starts at 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeakerSpan {
    pub start_ms: u64,
    pub end_ms: u64,
    pub speaker: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DiarizeEvent {
    Queued { ahead: u32 },
    /// Heartbeat while the node works. Telling speakers apart has no steps to report, the node is busy until it is done.
    Working,
    /// The whole answer, once.
    Spans { spans: Vec<SpeakerSpan>, elapsed_ms: u32 },
    Error(String),
}

/// A photo to read. The picture is sent as a JPEG, already scaled down on the phone.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DescribeRequest {
    /// What to ask about the picture. The node puts the picture in front of it.
    pub prompt: String,
    pub jpeg: Vec<u8>,
    pub max_tokens: u32,
    pub priority: Priority,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<ChatMsg>,
    pub max_tokens: u32,
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: i32,
    pub priority: Priority,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ChatEvent {
    /// The node has other work first. `ahead` is how many jobs are in front of this one.
    Queued { ahead: u32 },
    /// Sent every couple of seconds while the node is busy with the job and has nothing else to say.
    /// It lets the phone tell a slow node from a lost one: silence for a few seconds means lost.
    Working,
    Token(String),
    /// `elapsed_ms` is how long the node spent generating, so the phone can show the node's own speed.
    Done { prompt_tokens: u32, generated_tokens: u32, elapsed_ms: u32 },
    Error(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TranscribeRequest {
    /// ISO code such as "en" or "hi", or None to detect it.
    pub language: Option<String>,
    /// Length of the audio that will follow, in milliseconds (for progress).
    pub total_ms: u64,
    pub priority: Priority,
    /// What was said just before this audio, so a short window starts with the right names and spelling. None for a whole recording.
    pub prompt: Option<String>,
}

/// Audio sent by the phone after a [`TranscribeRequest`]: raw PCM, 16 bit little endian, mono, 16 kHz.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AudioFrame {
    Pcm(Vec<u8>),
    End,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TranscribeEvent {
    Queued { ahead: u32 },
    /// Heartbeat while the node is busy with the job.
    Working,
    /// How much of the audio has been transcribed so far.
    Progress { done_ms: u64, total_ms: u64 },
    /// One finished stretch of speech. Times are from the start of the audio.
    Segment { t0_ms: u64, t1_ms: u64, text: String },
    Done { language: String, segments: u32, elapsed_ms: u32 },
    Error(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EmbedRequest {
    /// Already formatted with the model's task prefix.
    pub texts: Vec<String>,
    pub priority: Priority,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum EmbedReply {
    Queued { ahead: u32 },
    Vectors(Vec<Vec<f32>>),
    Error(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PairReply {
    /// `proof` shows the node knows the code too, so the phone can trust what it pins.
    Ok { node_name: String, proof: [u8; 32] },
    Refused(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum KnockReply {
    Accepted { node_name: String },
    /// Waiting for a phone that is already paired, or someone at the node, to allow it.
    Pending { node_name: String },
    Refused(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DecideReply {
    Ok,
    Refused(String),
}

/// A phone that has knocked and not been answered yet.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PendingPhone {
    /// The first 16 characters of its certificate fingerprint.
    pub id: String,
    pub name: String,
    pub waited_s: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Capabilities {
    /// Name of the chat model, if this node serves chat.
    pub chat: Option<String>,
    /// Name of the search model, if this node serves embeddings.
    pub embed: Option<String>,
    pub embed_dim: u32,
    /// Measured at start. Zero means not measured.
    pub chat_tok_s: f32,
    pub embed_per_s: f32,
    /// Name of the speech model, if this node transcribes audio.
    pub transcribe: Option<String>,
    /// How many times faster than real time it transcribes (0 means not measured).
    pub transcribe_x: f32,
    /// The longest audio window, in seconds, this node treats as a quick job for live correction (0 means not stated,
    /// the phone then assumes 120). The phone keeps its window length at or under this.
    pub window_max_s: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StatusReply {
    pub proto: u32,
    pub node_name: String,
    pub version: String,
    pub caps: Capabilities,
    pub running: u32,
    pub waiting: u32,
    pub clients: u32,
    /// Jobs finished since the node started, for the phone's "what has it done" line.
    pub served: u64,
    pub uptime_s: u64,
    pub pending: Vec<PendingPhone>,
    /// The node keeps its models loaded but turns new chat and search jobs away. Last on purpose:
    /// a reader of the older layout ignores the extra byte.
    pub paused: bool,
    /// The node can read photos ([`Request::Describe`]). After `paused` for the same reason.
    pub vision: bool,
    /// The node can tell speakers apart ([`Request::Diarize`]): built with it and the speaker models were downloaded.
    pub speakers: bool,
}

/// What a record says about a phone. Allow and Revoke are one register (the later one wins), Block and Unblock
/// another. A phone is let in when it is allowed and not blocked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RecordKind {
    Allow,
    Revoke,
    Block,
    Unblock,
}

/// One fact about one phone that compute nodes copy to each other. Never anything about a job.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneRecord {
    /// The phone's certificate fingerprint.
    pub fp: String,
    pub name: String,
    pub kind: RecordKind,
    /// Milliseconds since 1970 when it was written. The later record wins.
    pub at_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PeerReply {
    Ok,
    Refused(String),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RecordsReply {
    pub records: Vec<PhoneRecord>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first(r: &Request) -> u8 {
        postcard::to_stdvec(r).unwrap()[0]
    }

    /// Phones in the field send and read these numbers. New messages go at the end and never shift an old one.
    #[test]
    fn the_messages_phones_know_keep_their_numbers() {
        let chat = ChatRequest { messages: vec![], max_tokens: 1, temperature: 0.0, top_p: 0.0, top_k: 0, priority: Priority::Bulk };
        let embed = EmbedRequest { texts: vec![], priority: Priority::Bulk };
        let transcribe = TranscribeRequest { language: None, total_ms: 0, priority: Priority::Bulk, prompt: None };
        let old = [
            Request::Status,
            Request::Pair { name: String::new(), proof: [0; 32] },
            Request::Chat(chat),
            Request::Embed(embed),
            Request::Transcribe(transcribe),
            Request::Knock { name: String::new() },
            Request::KnockStatus,
            Request::Decide { who: String::new(), allow: true },
        ];
        for (i, r) in old.iter().enumerate() {
            assert_eq!(first(r) as usize, i);
        }
        assert_eq!(first(&Request::Join { name: String::new(), port: 1, proof: [0; 32] }), 8);
        assert_eq!(first(&Request::Records { port: 1, records: vec![] }), 9);
        assert_eq!(first(&Request::Leave), 10);
        assert_eq!(first(&Request::Describe(DescribeRequest { prompt: String::new(), jpeg: vec![], max_tokens: 1, priority: Priority::Bulk })), 11);
        assert_eq!(first(&Request::Diarize(DiarizeRequest { people: 0, threshold: 0.0, total_ms: 0, priority: Priority::Bulk })), 12);
    }

    #[test]
    fn records_survive_the_wire() {
        let rec = PhoneRecord { fp: "ab".repeat(32), name: "phone".into(), kind: RecordKind::Unblock, at_ms: 1_700_000_000_123 };
        let bytes = postcard::to_stdvec(&Request::Records { port: 47615, records: vec![rec.clone()] }).unwrap();
        match postcard::from_bytes::<Request>(&bytes).unwrap() {
            Request::Records { port, records } => assert_eq!((port, records), (47615, vec![rec])),
            other => panic!("{other:?}"),
        }
    }
}
