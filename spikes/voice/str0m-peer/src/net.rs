//! Signalling (WebSocket to spikes/voice/signal), ICE server config and a
//! minimal STUN binding request. str0m gathers no candidates by itself and
//! has no TURN client, so server-reflexive discovery lives here.

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use serde_json::Value;
use tungstenite::{stream::MaybeTlsStream, Message, WebSocket};

/// Runs the WebSocket on its own thread: frames from the server go to
/// `incoming`, JSON values sent on the returned channel go to the server.
pub fn connect(url: &str, incoming: Sender<Value>) -> std::io::Result<Sender<Value>> {
    let (mut ws, _) = tungstenite::connect(url).map_err(|e| std::io::Error::other(e.to_string()))?;
    if let MaybeTlsStream::Plain(s) = ws.get_ref() {
        s.set_read_timeout(Some(Duration::from_millis(20)))?;
    }
    let (tx, rx) = std::sync::mpsc::channel::<Value>();
    std::thread::spawn(move || pump(&mut ws, &rx, &incoming));
    Ok(tx)
}

fn pump(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>, out: &Receiver<Value>, incoming: &Sender<Value>) {
    loop {
        while let Ok(v) = out.try_recv() {
            if ws.send(Message::text(v.to_string())).is_err() {
                return;
            }
        }
        match ws.read() {
            Ok(Message::Text(t)) => {
                if let Ok(v) = serde_json::from_str(&t) {
                    let _ = incoming.send(v);
                }
            }
            Ok(Message::Close(_)) => return,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => {
                eprintln!("signal: {e}");
                return;
            }
        }
    }
}

/// GET <signal>/config.json and return the STUN server addresses in it.
pub fn stun_servers(signal_url: &str) -> Vec<SocketAddr> {
    let host = signal_url.trim_start_matches("ws://").trim_end_matches('/');
    let body = (|| -> std::io::Result<String> {
        let mut s = TcpStream::connect(host)?;
        s.set_read_timeout(Some(Duration::from_secs(3)))?;
        write!(s, "GET /config.json HTTP/1.0\r\nHost: {host}\r\n\r\n")?;
        let mut r = String::new();
        s.read_to_string(&mut r)?;
        Ok(r)
    })()
    .unwrap_or_default();
    let json: Value = body.split("\r\n\r\n").nth(1).and_then(|b| serde_json::from_str(b).ok()).unwrap_or_default();
    let mut out = vec![];
    for server in json["iceServers"].as_array().into_iter().flatten() {
        let urls = match &server["urls"] {
            Value::String(s) => vec![s.clone()],
            Value::Array(a) => a.iter().filter_map(|u| u.as_str().map(String::from)).collect(),
            _ => vec![],
        };
        for u in urls.iter().filter_map(|u| u.strip_prefix("stun:")) {
            out.extend(u.to_socket_addrs().into_iter().flatten().filter(|a| a.is_ipv4()));
        }
    }
    out
}

/// RFC 5389 binding request; returns our public address as the server sees it.
pub fn stun_binding(socket: &UdpSocket, server: SocketAddr) -> Option<SocketAddr> {
    let mut req = [0u8; 20];
    req[0..2].copy_from_slice(&0x0001u16.to_be_bytes());
    req[4..8].copy_from_slice(&0x2112_A442u32.to_be_bytes());
    for (i, b) in req[8..20].iter_mut().enumerate() {
        *b = (std::process::id() as u8).wrapping_add(i as u8 * 31);
    }
    let old = socket.read_timeout().ok().flatten();
    socket.set_read_timeout(Some(Duration::from_millis(700))).ok()?;
    let mut buf = [0u8; 512];
    let mut result = None;
    for _ in 0..2 {
        socket.send_to(&req, server).ok()?;
        if let Ok((n, from)) = socket.recv_from(&mut buf) {
            if from == server && n >= 20 && buf[8..20] == req[8..20] {
                result = parse_xor_mapped(&buf[20..n]);
                break;
            }
        }
    }
    socket.set_read_timeout(old).ok();
    result
}

fn parse_xor_mapped(mut attrs: &[u8]) -> Option<SocketAddr> {
    while attrs.len() >= 4 {
        let kind = u16::from_be_bytes([attrs[0], attrs[1]]);
        let len = u16::from_be_bytes([attrs[2], attrs[3]]) as usize;
        let value = attrs.get(4..4 + len)?;
        if kind == 0x0020 && value.len() >= 8 && value[1] == 0x01 {
            let port = u16::from_be_bytes([value[2], value[3]]) ^ 0x2112;
            let ip = u32::from_be_bytes([value[4], value[5], value[6], value[7]]) ^ 0x2112_A442;
            return Some(SocketAddr::from((ip.to_be_bytes(), port)));
        }
        attrs = &attrs[(4 + len + 3) / 4 * 4..];
    }
    None
}

/// The local address other peers can reach: the interface used for the
/// default route (no packet is sent by connecting a UDP socket).
pub fn local_ip() -> std::net::IpAddr {
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("192.0.2.1:9").map(|_| s))
        .and_then(|s| s.local_addr())
        .map(|a| a.ip())
        .unwrap_or([127, 0, 0, 1].into())
}
