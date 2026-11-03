//! A deliberately small adaptive jitter buffer with Opus loss concealment.
//!
//! The browser gets this for free (NetEQ inside libwebrtc). With str0m we
//! have to bring our own; this is the minimum that makes a fair comparison:
//! adaptive target delay from RFC 3550 jitter, FEC from the next packet,
//! Opus PLC for gaps, and a crude frame drop when the buffer runs long.

use std::collections::BTreeMap;

pub const FRAME: usize = 960; // 20 ms at 48 kHz
const FRAME_MS: f64 = 20.0;

pub struct JitterBuffer {
    packets: BTreeMap<u64, Vec<u8>>,
    decoder: opus::Decoder,
    /// RTP timestamp to play next; None until enough audio is buffered.
    next: Option<u64>,
    last_transit: Option<f64>,
    /// RFC 3550 inter-arrival jitter in ms.
    pub jitter_ms: f64,
    first_ts: Option<u64>,
    max_ts: u64,
    pub received: u64,
    pub played: u64,
    pub concealed: u64,
    pub late: u64,
    /// Sum of buffered ms at every played frame, for the average delay.
    delay_sum: f64,
    misses: u32,
}

impl JitterBuffer {
    pub fn new() -> Self {
        JitterBuffer {
            packets: BTreeMap::new(),
            decoder: opus::Decoder::new(48_000, opus::Channels::Mono).expect("opus decoder"),
            next: None,
            last_transit: None,
            jitter_ms: 0.0,
            first_ts: None,
            max_ts: 0,
            received: 0,
            played: 0,
            concealed: 0,
            late: 0,
            delay_sum: 0.0,
            misses: 0,
        }
    }

    /// `ts` is the RTP timestamp (48 kHz), `arrival_ms` wall-clock ms.
    pub fn push(&mut self, ts: u64, payload: &[u8], arrival_ms: f64) {
        self.received += 1;
        let first = *self.first_ts.get_or_insert(ts);
        self.max_ts = self.max_ts.max(ts);
        let transit = arrival_ms - (ts.wrapping_sub(first)) as f64 / 48.0;
        if let Some(prev) = self.last_transit {
            self.jitter_ms += ((transit - prev).abs() - self.jitter_ms) / 16.0;
        }
        self.last_transit = Some(transit);
        if matches!(self.next, Some(n) if ts < n) {
            self.late += 1;
            return;
        }
        self.packets.insert(ts, payload.to_vec());
    }

    fn target_ms(&self) -> f64 {
        // At least one frame of slack, more as jitter grows.
        (FRAME_MS + 3.0 * self.jitter_ms).clamp(FRAME_MS, 200.0)
    }

    fn buffered_ms(&self) -> f64 {
        match (self.next, self.packets.keys().next_back()) {
            (Some(n), Some(&last)) if last >= n => (last - n) as f64 / 48.0 + FRAME_MS,
            (None, Some(&last)) => (last - self.packets.keys().next().unwrap()) as f64 / 48.0 + FRAME_MS,
            _ => 0.0,
        }
    }

    /// Called every 20 ms by the playout clock. Returns one frame of audio,
    /// or None while (re)buffering.
    pub fn pop(&mut self) -> Option<[i16; FRAME]> {
        let next = match self.next {
            Some(n) => n,
            None => {
                if self.buffered_ms() < self.target_ms() {
                    return None;
                }
                *self.packets.keys().next()?
            }
        };
        // Running long (jitter went down, or a burst arrived): skip a frame.
        if self.buffered_ms() > self.target_ms() + 3.0 * FRAME_MS && self.packets.len() > 2 {
            self.packets.remove(&next);
            return self.advance_and_pop(next);
        }
        self.delay_sum += self.buffered_ms();
        let mut out = [0i16; FRAME];
        let decoded = if let Some(p) = self.packets.remove(&next) {
            self.misses = 0;
            self.decoder.decode(&p, &mut out, false).is_ok()
        } else {
            self.concealed += 1;
            self.misses += 1;
            // In-band FEC carries a coarse copy of this frame in the next one.
            match self.packets.get(&(next + FRAME as u64)) {
                Some(p) => self.decoder.decode(&p.clone(), &mut out, true).is_ok(),
                None => self.decoder.decode(&[], &mut out, false).is_ok(),
            }
        };
        self.played += 1;
        self.next = Some(next + FRAME as u64);
        // Long outage: forget the clock and rebuffer from the next packet.
        if self.misses > 10 {
            self.next = None;
            self.misses = 0;
        }
        decoded.then_some(out)
    }

    fn advance_and_pop(&mut self, skipped: u64) -> Option<[i16; FRAME]> {
        self.next = Some(skipped + FRAME as u64);
        self.pop()
    }

    pub fn avg_delay_ms(&self) -> Option<f64> {
        (self.played > 0).then(|| self.delay_sum / self.played as f64)
    }

    pub fn loss_pct(&self) -> Option<f64> {
        let first = self.first_ts?;
        let expected = (self.max_ts - first) / FRAME as u64 + 1;
        Some(100.0 * expected.saturating_sub(self.received) as f64 / expected as f64)
    }
}
