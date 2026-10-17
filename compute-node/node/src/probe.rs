//! `marginalia-node probe`: behaves like a phone, to check a node end to end from the command line.

use anyhow::{Context, Result};
use marginalia_proto::{client, tls::Identity, wire, AudioFrame, ChatEvent, ChatMsg, ChatRequest, EmbedReply, EmbedRequest, Priority, Request, Role, TranscribeEvent, TranscribeRequest};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[allow(clippy::too_many_arguments)]
pub async fn run(addr: std::net::SocketAddr, dir: PathBuf, code: Option<String>, knock: bool, decide: Option<String>, deny: bool, transcribe: Option<PathBuf>, language: Option<String>, window: bool, prompt: Option<String>, hang_up_after: Option<f32>) -> Result<()> {
    std::fs::create_dir_all(&dir)?;
    let id_path = dir.join("probe-identity.json");
    let id: Identity = match std::fs::read(&id_path).ok().and_then(|b| serde_json::from_slice(&b).ok()) {
        Some(i) => i,
        None => {
            let i = Identity::generate()?;
            std::fs::write(&id_path, serde_json::to_vec(&i)?)?;
            i
        }
    };
    let pin_path = dir.join("probe-pin.txt");
    let ep = client::new_endpoint()?;
    if let Some(code) = code {
        let (conn, node_fp) = client::connect(&ep, &id, addr, None, Duration::from_secs(5)).await?;
        let name = client::pair(&conn, &id, &node_fp, "probe", &code).await?;
        std::fs::write(&pin_path, &node_fp)?;
        println!("paired with {name} ({})", &node_fp[..16]);
        conn.close(0u32.into(), b"done");
    }
    if knock {
        let (conn, node_fp) = client::connect(&ep, &id, addr, None, Duration::from_secs(5)).await?;
        std::fs::write(&pin_path, &node_fp)?;
        let mut reply = client::knock(&conn, "probe").await?;
        loop {
            match &reply {
                marginalia_proto::KnockReply::Accepted { node_name } => {
                    println!("let in by {node_name}");
                    break;
                }
                marginalia_proto::KnockReply::Pending { node_name } => {
                    println!("waiting for {node_name} to allow this phone...");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    reply = client::knock_status(&conn).await?;
                }
                marginalia_proto::KnockReply::Refused(why) => anyhow::bail!(why.clone()),
            }
        }
        conn.close(0u32.into(), b"done");
    }
    if let Some(who) = decide {
        let pin = std::fs::read_to_string(&pin_path).context("not paired yet")?;
        let (conn, _) = client::connect(&ep, &id, addr, Some(pin.trim().to_string()), Duration::from_secs(5)).await?;
        let st = client::status(&conn).await?;
        println!("waiting phones: {:?}", st.pending);
        client::decide(&conn, &who, !deny).await?;
        println!("{} {who}", if deny { "turned down" } else { "allowed" });
        return Ok(());
    }
    let pin = std::fs::read_to_string(&pin_path).context("not paired yet: pass --code")?;
    let (conn, _) = client::connect(&ep, &id, addr, Some(pin.trim().to_string()), Duration::from_secs(5)).await?;
    let st = client::status(&conn).await?;
    println!("status: {st:#?}");
    if let Some(file) = transcribe {
        return transcribe_file(&conn, &file, language, window, prompt, hang_up_after).await;
    }

    let t = Instant::now();
    let texts: Vec<String> = (0..48).map(|i| format!("title: none | text: Probe passage {i}, about a launch date and a migration.")).collect();
    let (mut send, mut recv) = client::open(&conn, &Request::Embed(EmbedRequest { texts, priority: Priority::Bulk })).await?;
    let _ = send.finish();
    loop {
        match wire::read_frame::<EmbedReply>(&mut recv).await?.context("closed")? {
            EmbedReply::Queued { ahead } => println!("embed queued behind {ahead}"),
            EmbedReply::Vectors(v) => {
                println!("embedded {} texts, dim {}, norm {:.3}, in {:.2}s", v.len(), v[0].len(), v[0].iter().map(|x| x * x).sum::<f32>().sqrt(), t.elapsed().as_secs_f32());
                break;
            }
            EmbedReply::Error(e) => anyhow::bail!(e),
        }
    }

    let t = Instant::now();
    let req = ChatRequest {
        messages: vec![ChatMsg { role: Role::User, content: "In two sentences, why do meeting notes help?".into() }],
        max_tokens: 120,
        temperature: 0.4,
        top_p: 0.95,
        top_k: 64,
        priority: Priority::Interactive,
    };
    let (mut send, mut recv) = client::open(&conn, &Request::Chat(req)).await?;
    let _ = send.finish();
    let mut first = None;
    let mut text = String::new();
    loop {
        match wire::read_frame::<ChatEvent>(&mut recv).await?.context("closed")? {
            ChatEvent::Queued { ahead } => println!("chat queued behind {ahead}"),
            ChatEvent::Working => {}
            ChatEvent::Token(s) => {
                first.get_or_insert_with(|| t.elapsed());
                text.push_str(&s);
            }
            ChatEvent::Done { prompt_tokens, generated_tokens, .. } => {
                println!("chat: first token {:.2}s, {generated_tokens} tokens ({prompt_tokens} prompt) in {:.2}s\n{text}", first.unwrap_or_default().as_secs_f32(), t.elapsed().as_secs_f32());
                break;
            }
            ChatEvent::Error(e) => anyhow::bail!(e),
        }
    }
    Ok(())
}

/// Reads 16 kHz mono 16 bit PCM from a `.wav` (the header is parsed here) or a raw `.pcm` file.
pub fn read_pcm(path: &std::path::Path) -> Result<Vec<u8>> {
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    if !(bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE") {
        return Ok(bytes);
    }
    let mut at = 12;
    let mut fmt_ok = false;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = at + 8;
        if id == b"fmt " && body + 16 <= bytes.len() {
            let tag = u16::from_le_bytes([bytes[body], bytes[body + 1]]);
            let channels = u16::from_le_bytes([bytes[body + 2], bytes[body + 3]]);
            let rate = u32::from_le_bytes(bytes[body + 4..body + 8].try_into().unwrap());
            let bits = u16::from_le_bytes([bytes[body + 14], bytes[body + 15]]);
            if tag != 1 || channels != 1 || rate != 16_000 || bits != 16 {
                anyhow::bail!("the wav file must be 16 kHz, mono, 16 bit (this one has {channels} channel(s), {rate} Hz, {bits} bit)");
            }
            fmt_ok = true;
        } else if id == b"data" {
            anyhow::ensure!(fmt_ok, "the wav file has no format chunk before its data");
            // A streamed file can carry a length that is too large or zero: take what is there.
            let end = if len == 0 || body + len > bytes.len() { bytes.len() } else { body + len };
            return Ok(bytes[body..end].to_vec());
        }
        at = body + len + (len & 1);
    }
    anyhow::bail!("the wav file has no data chunk")
}

async fn transcribe_file(conn: &quinn::Connection, file: &std::path::Path, language: Option<String>, window: bool, prompt: Option<String>, hang_up_after: Option<f32>) -> Result<()> {
    let pcm = read_pcm(file)?;
    let total_ms = pcm.len() as u64 / 32;
    println!("sending {:.1} s of audio from {}", total_ms as f64 / 1000.0, file.display());
    let t = Instant::now();
    let (mut send, mut recv) = client::open(conn, &Request::Transcribe(TranscribeRequest { language, total_ms, priority: if window { Priority::Interactive } else { Priority::Background }, prompt })).await?;
    // When the last audio frame left, in milliseconds after the start, for the wait after it.
    let sent_ms = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let sent_ms2 = sent_ms.clone();
    // The audio goes in 1 MB frames while the answers are read, like the phone does.
    tokio::spawn(async move {
        for chunk in pcm.chunks(1_000_000) {
            if wire::write_frame(&mut send, &AudioFrame::Pcm(chunk.to_vec())).await.is_err() {
                return;
            }
        }
        if wire::write_frame(&mut send, &AudioFrame::End).await.is_ok() {
            sent_ms2.store(t.elapsed().as_millis() as u64, std::sync::atomic::Ordering::Relaxed);
            println!("[{:6.1}s] audio sent", t.elapsed().as_secs_f32());
        }
        let _ = send.finish();
    });
    let mut beats = 0u32;
    let hang_up = async {
        match hang_up_after {
            Some(s) => tokio::time::sleep(Duration::from_secs_f32(s)).await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(hang_up);
    loop {
        let ev = tokio::select! {
            ev = wire::read_frame::<TranscribeEvent>(&mut recv) => ev?.context("the node closed without an answer")?,
            _ = &mut hang_up => {
                println!("hanging up after {:.1} s", t.elapsed().as_secs_f32());
                conn.close(0u32.into(), b"hang up");
                // Gives the close message time to leave before the program ends.
                tokio::time::sleep(Duration::from_millis(300)).await;
                return Ok(());
            }
        };
        match ev {
            TranscribeEvent::Queued { ahead } => println!("[{:6.1}s] queued behind {ahead}", t.elapsed().as_secs_f32()),
            TranscribeEvent::Working => beats += 1,
            TranscribeEvent::Progress { done_ms, total_ms } => println!("[{:6.1}s] progress {:.0}%", t.elapsed().as_secs_f32(), done_ms as f64 * 100.0 / total_ms.max(1) as f64),
            TranscribeEvent::Segment { t0_ms, t1_ms, text } => println!("[{:6.1}s] {} -> {}  {text}", t.elapsed().as_secs_f32(), stamp(t0_ms), stamp(t1_ms)),
            TranscribeEvent::Done { language, segments, elapsed_ms } => return done(t, total_ms, &language, segments, elapsed_ms, beats, sent_ms.load(std::sync::atomic::Ordering::Relaxed)),
            TranscribeEvent::Error(e) => anyhow::bail!(e),
        }
    }
}

fn done(t: Instant, total_ms: u64, language: &str, segments: u32, elapsed_ms: u32, beats: u32, sent_ms: u64) -> Result<()> {
    let wall = t.elapsed().as_secs_f64();
    println!(
        "\ndone: language {language}, {segments} segments, node worked {:.2} s, {:.2} s from start to end, {beats} heartbeats\naudio {:.1} s: {:.1} times faster than real time on the node, {:.1} times counting the sending",
        elapsed_ms as f64 / 1000.0,
        wall,
        total_ms as f64 / 1000.0,
        total_ms as f64 / elapsed_ms.max(1) as f64,
        total_ms as f64 / 1000.0 / wall.max(0.001),
    );
    println!("last audio frame sent to Done: {:.3} s", (t.elapsed().as_millis() as u64).saturating_sub(sent_ms) as f64 / 1000.0);
    Ok(())
}

fn stamp(ms: u64) -> String {
    format!("{:02}:{:02}:{:05.2}", ms / 3_600_000, ms / 60_000 % 60, (ms % 60_000) as f64 / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(rate: u32, channels: u16, data: &[u8], extra_chunk: bool) -> Vec<u8> {
        let mut v = b"RIFF\0\0\0\0WAVE".to_vec();
        if extra_chunk {
            v.extend_from_slice(b"LIST");
            v.extend_from_slice(&3u32.to_le_bytes());
            v.extend_from_slice(&[1, 2, 3, 0]); // three bytes and the pad byte
        }
        v.extend_from_slice(b"fmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        for x in [1u16, channels] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v.extend_from_slice(&rate.to_le_bytes());
        v.extend_from_slice(&(rate * 2).to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(data);
        v
    }

    fn read(bytes: Vec<u8>, name: &str) -> Result<Vec<u8>> {
        let p = std::env::temp_dir().join(format!("marginalia-probe-test-{}-{name}", std::process::id()));
        std::fs::write(&p, bytes).unwrap();
        let r = read_pcm(&p);
        let _ = std::fs::remove_file(&p);
        r
    }

    #[test]
    fn reads_wav_data_after_other_chunks() {
        assert_eq!(read(wav(16_000, 1, &[1, 0, 2, 0], false), "a").unwrap(), vec![1, 0, 2, 0]);
        assert_eq!(read(wav(16_000, 1, &[5, 0], true), "b").unwrap(), vec![5, 0]);
    }

    #[test]
    fn refuses_other_formats_and_passes_raw_pcm_through() {
        assert!(read(wav(44_100, 1, &[0, 0], false), "c").is_err());
        assert!(read(wav(16_000, 2, &[0, 0], false), "d").is_err());
        assert_eq!(read(vec![9, 9, 9, 9], "e").unwrap(), vec![9, 9, 9, 9]);
    }
}
