//! The tray's only link to the node: one JSON line out, one JSON line back, on 127.0.0.1.
//! Same channel `marginalia-node pair` and `status` use.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

/// Sends `{"cmd": cmd}` to the node's admin port and returns its answer.
/// An error means nothing answered, or the node answered with an error text.
pub fn ask(port: u16, cmd: &str) -> Result<Value, String> {
    ask_with(port, &json!({ "cmd": cmd }))
}

/// Like [`ask`] for a command that takes arguments, such as `{"cmd": "block", "who": "..."}`.
pub fn ask_with(port: u16, cmd: &Value) -> Result<Value, String> {
    ask_at(SocketAddr::from((Ipv4Addr::LOCALHOST, port)), cmd)
}

pub fn ask_at(addr: SocketAddr, cmd: &Value) -> Result<Value, String> {
    let mut sock = TcpStream::connect_timeout(&addr, Duration::from_millis(800)).map_err(|_| "The node is not running.".to_string())?;
    let _ = sock.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = sock.set_write_timeout(Some(Duration::from_secs(3)));
    sock.write_all(format!("{cmd}\n").as_bytes()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(sock).read_line(&mut line).map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(line.trim()).map_err(|_| "the node sent something unreadable".to_string())?;
    if let Some(e) = v["error"].as_str() {
        return Err(e.to_string());
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A tiny stand-in for the node: answers every connection with the reply picked by the command.
    fn fake_node(reply: fn(&str) -> String) -> (SocketAddr, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let h = std::thread::spawn(move || {
            let mut seen = vec![];
            for _ in 0..3 {
                let (mut s, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(s.try_clone().unwrap()).read_line(&mut line).unwrap();
                let v: Value = serde_json::from_str(&line).unwrap();
                let cmd = v["cmd"].as_str().unwrap_or("").to_string();
                s.write_all(format!("{}\n", reply(&cmd)).as_bytes()).unwrap();
                seen.push(cmd);
            }
            seen
        });
        (addr, h)
    }

    #[test]
    fn sends_one_line_and_reads_one_line() {
        let (addr, h) = fake_node(|c| match c {
            "status" => r#"{"name":"falcon","paused":false}"#.into(),
            "boom" => r#"{"error":"unknown command"}"#.into(),
            _ => "this is not json".into(),
        });
        let ok = ask_at(addr, &json!({"cmd": "status"})).unwrap();
        assert_eq!(ok["name"], "falcon");
        assert_eq!(ask_at(addr, &json!({"cmd": "boom"})).unwrap_err(), "unknown command");
        assert!(ask_at(addr, &json!({"cmd": "other"})).is_err());
        assert_eq!(h.join().unwrap(), vec!["status", "boom", "other"]);
    }

    #[test]
    fn nothing_listening_is_an_error_not_a_hang() {
        // Port 1 on loopback is closed on any normal machine.
        let started = std::time::Instant::now();
        assert!(ask_at(SocketAddr::from((Ipv4Addr::LOCALHOST, 1)), &json!({"cmd": "status"})).is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
