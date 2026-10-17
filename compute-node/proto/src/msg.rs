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
    /// A phone that is not paired yet asks to be let in. The person has already said on the phone
    /// that they trust this compute node.
    Knock { name: String },
    /// Asks how a knock went. Answered for phones that are waiting or already let in.
    KnockStatus,
    /// A paired phone answers another phone's knock. `who` starts with the waiting phone's id.
    Decide { who: String, allow: bool },
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
}
