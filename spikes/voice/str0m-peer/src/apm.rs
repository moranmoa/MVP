//! Echo cancellation, noise suppression and gain control with WebRTC's own
//! AudioProcessing module (the one inside Chromium), built from source by
//! the `webrtc-audio-processing` crate. Opt-in: `--features apm`.
//! Without the feature these calls do nothing.

#[cfg(feature = "apm")]
pub struct Apm(std::sync::Mutex<webrtc_audio_processing::Processor>);

#[cfg(feature = "apm")]
impl Apm {
    pub fn new() -> Option<Apm> {
        use webrtc_audio_processing_config as c;
        let p = webrtc_audio_processing::Processor::new(48_000).ok()?;
        p.set_config(c::Config {
            echo_canceller: Some(c::EchoCanceller::default()),
            noise_suppression: Some(c::NoiseSuppression::default()),
            gain_controller: Some(c::GainController::GainController2(c::GainController2::default())),
            high_pass_filter: Some(c::HighPassFilter::default()),
            ..Default::default()
        });
        Some(Apm(std::sync::Mutex::new(p)))
    }

    /// Microphone side, in place; 10 ms chunks.
    pub fn capture(&self, pcm: &mut [f32]) {
        let p = self.0.lock().unwrap();
        for chunk in pcm.chunks_mut(480) {
            let mut frame = vec![chunk.to_vec()];
            if p.process_capture_frame(&mut frame).is_ok() {
                chunk.copy_from_slice(&frame[0]);
            }
        }
    }

    /// Speaker side: what we play is the echo to remove from the microphone.
    pub fn render(&self, pcm: &[f32]) {
        let p = self.0.lock().unwrap();
        for chunk in pcm.chunks(480) {
            let _ = p.process_render_frame(&mut vec![chunk.to_vec()]);
        }
    }
}

#[cfg(not(feature = "apm"))]
pub struct Apm;

#[cfg(not(feature = "apm"))]
impl Apm {
    pub fn new() -> Option<Apm> {
        None
    }
    pub fn capture(&self, _: &mut [f32]) {}
    pub fn render(&self, _: &[f32]) {}
}
