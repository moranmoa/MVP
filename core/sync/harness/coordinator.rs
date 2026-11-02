//! Harness coordinator: drives the peers through the spike-2 scenarios and
//! prints the measurements as Markdown.
//!
//! `sync-harness <p1:7000,p2:7000,p3:7000> <p4:7000>` — the first list is
//! the three peers that write; the last one joins fresh at the end.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;

const POLL: Duration = Duration::from_millis(20);
const OFFLINE_WRITES: u64 = 10_000;

struct Peer {
    name: String,
    lines: Lines<BufReader<OwnedReadHalf>>,
    write: OwnedWriteHalf,
}

impl Peer {
    async fn connect(addr: &str) -> Result<Self> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let stream = loop {
            match TcpStream::connect(addr).await {
                Ok(stream) => break stream,
                Err(_) if Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(200)).await,
                Err(err) => return Err(err).context(format!("connecting to {addr}")),
            }
        };
        let (read, write) = stream.into_split();
        let name = addr.split(':').next().unwrap().to_string();
        Ok(Self { name, lines: BufReader::new(read).lines(), write })
    }

    async fn call(&mut self, command: Value) -> Result<Value> {
        self.write.write_all(format!("{command}\n").as_bytes()).await?;
        let line = self.lines.next_line().await?.context("peer closed the control connection")?;
        let reply: Value = serde_json::from_str(&line)?;
        if let Some(err) = reply.get("error") {
            bail!("{}: {err}", self.name);
        }
        Ok(reply)
    }

    async fn state(&mut self) -> Result<Value> {
        self.call(json!({ "cmd": "state" })).await
    }
}

fn count(state: &Value) -> u64 {
    state["count"].as_u64().unwrap_or(0)
}

/// Polls until every peer holds `expected` blobs with the same state hash.
/// Returns the time it took and the hash.
async fn wait_converged(peers: &mut [&mut Peer], expected: u64, timeout: Duration) -> Result<(Duration, String)> {
    let started = Instant::now();
    loop {
        let mut states = Vec::new();
        for peer in peers.iter_mut() {
            states.push(peer.state().await?);
        }
        let hash = states[0]["hash"].as_str().unwrap_or_default().to_string();
        if states.iter().all(|s| count(s) == expected && s["hash"] == states[0]["hash"]) {
            return Ok((started.elapsed(), hash));
        }
        if started.elapsed() > timeout {
            let counts: Vec<_> = states.iter().map(count).collect();
            bail!("no convergence after {timeout:?}: counts {counts:?}, expected {expected}");
        }
        tokio::time::sleep(POLL).await;
    }
}

async fn wait_writes_done(peers: &mut [&mut Peer]) -> Result<()> {
    for peer in peers.iter_mut() {
        while peer.state().await?["writing"].as_bool() == Some(true) {
            tokio::time::sleep(POLL).await;
        }
    }
    Ok(())
}

async fn collect_latencies(peers: &mut [&mut Peer]) -> Result<(Vec<f64>, u64, u64)> {
    let mut all = Vec::new();
    let (mut push, mut pull) = (0, 0);
    for peer in peers.iter_mut() {
        let reply = peer.call(json!({ "cmd": "latencies" })).await?;
        all.extend(reply["latencies_ms"].as_array().unwrap().iter().filter_map(Value::as_f64));
        push += reply["via_push"].as_u64().unwrap_or(0);
        pull += reply["via_pull"].as_u64().unwrap_or(0);
    }
    all.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Ok((all, push, pull))
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn latency_row(label: &str, (lat, push, pull): &(Vec<f64>, u64, u64)) -> String {
    format!(
        "| {label} | {} | {push} / {pull} | {:.1} | {:.1} | {:.1} | {:.1} |",
        lat.len(),
        percentile(lat, 0.5),
        percentile(lat, 0.95),
        percentile(lat, 0.99),
        lat.last().copied().unwrap_or(f64::NAN),
    )
}

fn ms(d: Duration) -> String {
    format!("{} ms", d.as_millis())
}

fn opt_ms(v: &Value) -> String {
    v.as_u64().map_or("-".into(), |v| format!("{v} ms"))
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [writers, joiner] = args.as_slice() else {
        bail!("usage: sync-harness <p1:7000,p2:7000,p3:7000> <p4:7000>");
    };
    let mut writers_vec = Vec::new();
    for addr in writers.split(',') {
        writers_vec.push(Peer::connect(addr).await?);
    }
    let [a, b, c] = writers_vec.as_mut_slice() else { bail!("need exactly 3 writer peers") };
    let mut d = Peer::connect(joiner).await?;
    let mut report = Vec::new();
    let mut total = 0u64;

    // 1. Start the three peers together; they find each other via the relay.
    for peer in [&mut *a, &mut *b, &mut *c] {
        peer.call(json!({ "cmd": "start" })).await?;
    }
    let started = Instant::now();
    loop {
        let mut connected = 0;
        for peer in [&mut *a, &mut *b, &mut *c] {
            if peer.state().await?["peers"].as_u64().unwrap_or(0) >= 2 {
                connected += 1;
            }
        }
        if connected == 3 {
            break;
        }
        if started.elapsed() > Duration::from_secs(30) {
            bail!("peers did not all connect within 30 s");
        }
        tokio::time::sleep(POLL).await;
    }
    let mesh_time = started.elapsed();
    let mut first_peer = Vec::new();
    for peer in [&mut *a, &mut *b, &mut *c] {
        let state = peer.state().await?;
        first_peer.push(format!("{} {}", peer.name, opt_ms(&state["first_peer_ms"])));
    }
    report.push(format!(
        "## Discovery\n\nAll 3 peers connected to each other {} after the last one was started (start → first peer, per node: {}).\n",
        ms(mesh_time),
        first_peer.join(", ")
    ));
    // Let gossipsub exchange subscriptions before the first writes.
    tokio::time::sleep(Duration::from_secs(2)).await;

    // 2. 1,000 messages written concurrently across the 3 peers, as fast as
    //    each can write.
    for peer in [&mut *a, &mut *b, &mut *c] {
        peer.call(json!({ "cmd": "reset" })).await?;
    }
    let started = Instant::now();
    for (peer, n) in [(&mut *a, 334), (&mut *b, 333), (&mut *c, 333)] {
        peer.call(json!({ "cmd": "write", "n": n, "interval_ms": 0 })).await?;
    }
    total += 1000;
    let (took, hash) = wait_converged(&mut [&mut *a, &mut *b, &mut *c], total, Duration::from_secs(60)).await?;
    let burst = collect_latencies(&mut [&mut *a, &mut *b, &mut *c]).await?;
    report.push(format!(
        "## 1. Concurrent writes\n\n1,000 messages written at full speed across 3 peers (334/333/333). \
         All 3 peers reached the same state {} after the writes began. State hash: `{}` (identical on all 3).\n",
        ms(started.elapsed().max(took)),
        &hash[..16]
    ));

    // 3. Live latency at a chat-like pace: 3 peers × 200 messages, one every
    //    50 ms each (60 msg/s overall).
    for peer in [&mut *a, &mut *b, &mut *c] {
        peer.call(json!({ "cmd": "reset" })).await?;
    }
    for peer in [&mut *a, &mut *b, &mut *c] {
        peer.call(json!({ "cmd": "write", "n": 200, "interval_ms": 50 })).await?;
    }
    total += 600;
    wait_writes_done(&mut [&mut *a, &mut *b, &mut *c]).await?;
    wait_converged(&mut [&mut *a, &mut *b, &mut *c], total, Duration::from_secs(60)).await?;
    let live = collect_latencies(&mut [&mut *a, &mut *b, &mut *c]).await?;
    report.push(format!(
        "## 2. Delivery latency\n\nTime from `append` on the writer to the blob being stored on another peer \
         (all peers share the host clock).\n\n\
         | Load | Deliveries | push / pull | p50 ms | p95 ms | p99 ms | max ms |\n|---|---|---|---|---|---|---|\n{}\n{}\n",
        latency_row("live: 3 × 200 msgs, 20 msg/s per peer", &live),
        latency_row("burst: the 1,000 concurrent writes above", &burst),
    ));

    // 4. Peer c goes offline; a and b write 10,000 messages; c comes back.
    c.call(json!({ "cmd": "stop" })).await?;
    let started = Instant::now();
    a.call(json!({ "cmd": "write", "n": OFFLINE_WRITES / 2, "interval_ms": 0 })).await?;
    b.call(json!({ "cmd": "write", "n": OFFLINE_WRITES / 2, "interval_ms": 0 })).await?;
    total += OFFLINE_WRITES;
    wait_converged(&mut [&mut *a, &mut *b], total, Duration::from_secs(120)).await?;
    let write_time = started.elapsed();
    let restarted = Instant::now();
    c.call(json!({ "cmd": "start" })).await?;
    let (took, _) = wait_converged(&mut [&mut *a, &mut *b, &mut *c], total, Duration::from_secs(120)).await?;
    let polled = restarted.elapsed().max(took);
    let c_state = c.state().await?;
    report.push(format!(
        "## 3. Offline peer catches up\n\n{} stopped (store closed, all connections dropped). The other two wrote \
         {OFFLINE_WRITES} messages ({} until both had them). {} restarted and caught up:\n\n\
         | Measure | Time |\n|---|---|\n\
         | restart → first peer connected | {} |\n\
         | restart → first missed message stored | {} |\n\
         | restart → last missed message stored | {} |\n\
         | restart → harness saw identical state hash on all 3 | {} |\n",
        c.name,
        ms(write_time),
        c.name,
        opt_ms(&c_state["first_peer_ms"]),
        opt_ms(&c_state["first_remote_ms"]),
        opt_ms(&c_state["last_remote_ms"]),
        ms(polled),
    ));

    // 5. A brand-new peer joins.
    let joined = Instant::now();
    d.call(json!({ "cmd": "start" })).await?;
    let (took, hash) =
        wait_converged(&mut [&mut *a, &mut *b, &mut *c, &mut d], total, Duration::from_secs(120)).await?;
    let polled = joined.elapsed().max(took);
    let d_state = d.state().await?;
    report.push(format!(
        "## 4. New peer joins\n\n{} started with an empty store while the log held {total} messages.\n\n\
         | Measure | Time |\n|---|---|\n\
         | join → first peer connected | {} |\n\
         | join → first messages stored | {} |\n\
         | join → full history stored | {} |\n\
         | join → harness saw identical state hash on all 4 | {} |\n\n\
         Final state hash on all 4 peers: `{}`.\n",
        d.name,
        opt_ms(&d_state["first_peer_ms"]),
        opt_ms(&d_state["first_remote_ms"]),
        opt_ms(&d_state["last_remote_ms"]),
        ms(polled),
        &hash[..16]
    ));

    println!("\n===== RESULTS =====\n{}", report.join("\n"));
    if let Ok(dir) = std::env::var("OUT_DIR") {
        std::fs::write(format!("{dir}/results.md"), report.join("\n"))?;
    }
    Ok(())
}
