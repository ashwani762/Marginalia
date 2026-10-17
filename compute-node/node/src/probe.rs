//! `marginalia-node probe`: behaves like a phone, to check a node end to end from the command line.

use anyhow::{Context, Result};
use marginalia_proto::{client, tls::Identity, wire, ChatEvent, ChatMsg, ChatRequest, EmbedReply, EmbedRequest, Priority, Request, Role};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub async fn run(addr: std::net::SocketAddr, dir: PathBuf, code: Option<String>, knock: bool, decide: Option<String>, deny: bool) -> Result<()> {
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
