//! Sound card I/O through cpal (WASAPI on Windows, CoreAudio on macOS, ALSA
//! on Linux; PulseAudio and PipeWire are reached through their ALSA plugins).
//! Everything is converted to 48 kHz mono f32 at the edges.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

pub type Ring = Arc<Mutex<VecDeque<f32>>>;

pub fn list_devices() {
    let host = cpal::default_host();
    println!("audio host: {:?}", host.id());
    let name = |d: &cpal::Device| d.name().unwrap_or_else(|_| "?".into());
    let default_in = host.default_input_device().map(|d| name(&d));
    let default_out = host.default_output_device().map(|d| name(&d));
    for (label, devices, default) in [
        ("input", host.input_devices().map(|d| d.collect::<Vec<_>>()), &default_in),
        ("output", host.output_devices().map(|d| d.collect::<Vec<_>>()), &default_out),
    ] {
        match devices {
            Ok(list) => {
                for d in list {
                    let n = name(&d);
                    let mark = if Some(&n) == default.as_ref() { " (default)" } else { "" };
                    println!("{label}: {n}{mark}");
                }
            }
            Err(e) => println!("{label}: error {e}"),
        }
    }
}

fn find(input: bool, wanted: Option<&str>) -> Option<cpal::Device> {
    let host = cpal::default_host();
    let Some(wanted) = wanted else {
        return if input { host.default_input_device() } else { host.default_output_device() };
    };
    let mut devices = if input { host.input_devices().ok()? } else { host.output_devices().ok()? };
    devices.find(|d| d.name().map(|n| n.contains(wanted)).unwrap_or(false))
}

/// Linear resampler between the device rate and 48 kHz.
struct Resampler {
    step: f64,
    pos: f64,
    last: f32,
}

impl Resampler {
    fn new(from: u32, to: u32) -> Self {
        Resampler { step: from as f64 / to as f64, pos: 0.0, last: 0.0 }
    }
    fn run(&mut self, input: &[f32], out: &mut impl Extend<f32>) {
        let mut produced = vec![];
        while self.pos < input.len() as f64 {
            let i = self.pos.floor() as usize;
            let frac = (self.pos - i as f64) as f32;
            let a = if i == 0 { self.last } else { input[i - 1] };
            let b = input[i];
            produced.push(a + (b - a) * frac);
            self.pos += self.step;
        }
        self.pos -= input.len() as f64;
        self.last = *input.last().unwrap_or(&self.last);
        out.extend(produced);
    }
}

pub fn start_input(name: Option<&str>, ring: Ring) -> Result<cpal::Stream, String> {
    let device = find(true, name).ok_or("no input device")?;
    let config = device.default_input_config().map_err(|e| e.to_string())?;
    let channels = config.channels() as usize;
    let mut rs = Resampler::new(config.sample_rate().0, 48_000);
    eprintln!("input: {} @ {} Hz x{}", device.name().unwrap_or_default(), config.sample_rate().0, channels);
    let stream = device
        .build_input_stream(
            &config.into(),
            move |data: &[f32], _| {
                let mono: Vec<f32> = data.chunks(channels).map(|c| c.iter().sum::<f32>() / channels as f32).collect();
                let mut r = ring.lock().unwrap();
                rs.run(&mono, &mut *r);
                // Never let capture latency build up behind a stalled sender.
                while r.len() > 48_000 / 5 {
                    r.pop_front();
                }
            },
            |e| eprintln!("input error: {e}"),
            None,
        )
        .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

pub fn start_output(name: Option<&str>, ring: Ring) -> Result<cpal::Stream, String> {
    let device = find(false, name).ok_or("no output device")?;
    let config = device.default_output_config().map_err(|e| e.to_string())?;
    let channels = config.channels() as usize;
    let rate = config.sample_rate().0;
    let mut rs = Resampler::new(48_000, rate);
    let mut pending: VecDeque<f32> = VecDeque::new();
    eprintln!("output: {} @ {} Hz x{}", device.name().unwrap_or_default(), rate, channels);
    let stream = device
        .build_output_stream(
            &config.into(),
            move |data: &mut [f32], _| {
                let frames = data.len() / channels;
                while pending.len() < frames {
                    let chunk: Vec<f32> = {
                        let mut r = ring.lock().unwrap();
                        let n = r.len().min(480);
                        r.drain(..n).collect()
                    };
                    if chunk.is_empty() {
                        break;
                    }
                    rs.run(&chunk, &mut pending);
                }
                for frame in data.chunks_mut(channels) {
                    let v = pending.pop_front().unwrap_or(0.0);
                    frame.iter_mut().for_each(|s| *s = v);
                }
            },
            |e| eprintln!("output error: {e}"),
            None,
        )
        .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}
