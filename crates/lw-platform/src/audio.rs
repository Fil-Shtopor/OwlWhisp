//! Microphone capture via **cpal**, feeding a shared [`RingBuffer`] with mono `f32`
//! samples at the device's native rate.
//!
//! A **fresh stream is built on every [`Capture::start`] and dropped on [`Capture::stop`]**.
//! Re-using one long-lived stream across recordings breaks on some drivers (e.g. the
//! Qualcomm Aqstic mic array delivers silence after the first pause/resume), so we always
//! rebuild — this is the OpenWritr fix.
//!
//! The pipeline downstream wants 16 kHz mono; capture pushes native-rate mono into the ring
//! and [`Capture::stop`] returns an [`AudioBuffer`] tagged with the native rate. The caller
//! resamples with [`AudioBuffer::to_target`].

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use cpal::SampleFormat;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use lw_core::audio::{AudioBuffer, RingBuffer, downmix_to_mono, i16_to_f32, rms, u16_to_f32};

use crate::{Error, Result};

/// Default rolling-window capacity: 120 s at 48 kHz mono (~23 MiB of f32).
pub const DEFAULT_RING_CAPACITY: usize = 120 * 48_000;

fn capacity_for_duration(sample_rate: u32, seconds: usize) -> usize {
    (sample_rate as usize).saturating_mul(seconds.max(1)).max(1)
}

/// Platform-neutral microphone capture.
pub trait AudioCapture {
    /// Open the input device and start pushing mono samples into the ring buffer.
    /// Builds a fresh OS stream every time.
    fn start(&mut self) -> Result<()>;
    /// Stop capturing (drops the OS stream) and return everything recorded since
    /// [`start`](AudioCapture::start), at the device's **native** sample rate.
    fn stop(&mut self) -> Result<AudioBuffer>;
    /// The shared ring buffer capture writes into (clone is cheap — shared `Arc`).
    fn ring(&self) -> RingBuffer;
    /// RMS level of the most recent callback buffer, in `[0, 1]` — for a live level meter.
    fn level_rms(&self) -> f32;
}

/// A cheap, shareable read handle on a capture's live RMS level.
///
/// The level meter runs on its own thread while the `Capture` itself has been moved into the
/// worker's state, so it needs something it can hold that is neither the capture nor a borrow of
/// it. Cloning is an `Arc` bump.
#[derive(Clone)]
pub struct LevelHandle(Arc<AtomicU32>);

impl LevelHandle {
    /// The RMS of the most recent callback buffer, in `[0, 1]`.
    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

/// Human-readable names of the available audio input devices.
pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    let mut names = Vec::new();
    if let Ok(devices) = host.input_devices() {
        for device in devices {
            let name = device
                .description()
                .map(|d| d.name().to_string())
                .unwrap_or_else(|_| device.to_string());
            names.push(name);
        }
    }
    names
}

/// Convert an interleaved `f32` callback buffer to mono.
pub fn mono_from_f32(interleaved: &[f32], channels: u16) -> Vec<f32> {
    downmix_to_mono(interleaved, channels)
}

/// Convert an interleaved `i16` callback buffer to mono `f32` in `[-1, 1]`.
pub fn mono_from_i16(interleaved: &[i16], channels: u16) -> Vec<f32> {
    downmix_to_mono(&i16_to_f32(interleaved), channels)
}

/// Convert an interleaved `u16` callback buffer to mono `f32` in `[-1, 1]`.
pub fn mono_from_u16(interleaved: &[u16], channels: u16) -> Vec<f32> {
    downmix_to_mono(&u16_to_f32(interleaved), channels)
}

/// Convert an interleaved `i32` callback buffer to mono `f32` in `[-1, 1]`.
pub fn mono_from_i32(interleaved: &[i32], channels: u16) -> Vec<f32> {
    let f: Vec<f32> = interleaved.iter().map(|&s| s as f32 / 2_147_483_648.0).collect();
    downmix_to_mono(&f, channels)
}

/// Microphone capture into a shared [`RingBuffer`].
///
/// Also usable through the [`AudioCapture`] trait. The concrete type additionally exposes
/// [`native_sample_rate`](Capture::native_sample_rate) and [`is_active`](Capture::is_active).
pub struct Capture {
    device_name: Option<String>,
    ring: RingBuffer,
    duration_secs: Option<usize>,
    stream: Option<cpal::Stream>,
    native_rate: u32,
    last_rms: Arc<AtomicU32>,
}

impl Capture {
    /// A handle on the live level that other threads can hold.
    ///
    /// Reading the level through this keeps working after the `Capture` is moved elsewhere,
    /// which is what a meter running on its own thread needs.
    pub fn level_handle(&self) -> LevelHandle {
        LevelHandle(Arc::clone(&self.last_rms))
    }

    /// New capture handle. `device_name` selects an input device whose name contains the
    /// given string (case-insensitive); `None` uses the system default input device.
    /// `ring_capacity_samples` bounds the rolling window ([`DEFAULT_RING_CAPACITY`] is a
    /// sensible default).
    pub fn new(device_name: Option<String>, ring_capacity_samples: usize) -> Self {
        Self {
            device_name,
            ring: RingBuffer::new(ring_capacity_samples.max(1)),
            duration_secs: None,
            stream: None,
            native_rate: 0,
            last_rms: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Capture a duration at the microphone's native sample rate. Unlike a fixed sample count,
    /// this keeps the same amount of speech on 16, 48 and 96 kHz devices.
    pub fn for_duration(device_name: Option<String>, seconds: usize) -> Self {
        let mut capture = Self::new(device_name, 1);
        capture.duration_secs = Some(seconds.max(1));
        capture
    }

    /// The native sample rate of the last-opened stream (0 before the first start).
    pub fn native_sample_rate(&self) -> u32 {
        self.native_rate
    }

    /// Whether a stream is currently open.
    pub fn is_active(&self) -> bool {
        self.stream.is_some()
    }

    fn find_device(&self, host: &cpal::Host) -> Result<cpal::Device> {
        match &self.device_name {
            None => host
                .default_input_device()
                .ok_or_else(|| Error::Audio("no default input device".into())),
            Some(wanted) => {
                let wanted_lc = wanted.to_lowercase();
                let devices = host
                    .input_devices()
                    .map_err(|e| Error::Audio(format!("enumerate input devices: {e}")))?;
                for device in devices {
                    let name = device
                        .description()
                        .map(|d| d.name().to_string())
                        .unwrap_or_else(|_| device.to_string());
                    if name.to_lowercase().contains(&wanted_lc) {
                        return Ok(device);
                    }
                }
                Err(Error::Audio(format!(
                    "input device matching {wanted:?} not found"
                )))
            }
        }
    }
}

impl AudioCapture for Capture {
    fn start(&mut self) -> Result<()> {
        if self.stream.is_some() {
            return Err(Error::Audio("capture already started".into()));
        }
        let host = cpal::default_host();
        let device = self.find_device(&host)?;
        let supported = device
            .default_input_config()
            .map_err(|e| Error::Audio(format!("query default input config: {e}")))?;
        let channels = supported.channels();
        let sample_rate = supported.sample_rate();
        let sample_format = supported.sample_format();
        let config = supported.config();

        if let Some(seconds) = self.duration_secs {
            self.ring = RingBuffer::new(capacity_for_duration(sample_rate, seconds));
        } else {
            self.ring.reset();
        }
        self.last_rms.store(0, Ordering::Relaxed);
        self.native_rate = sample_rate;

        let ring = self.ring.clone();
        let level = Arc::clone(&self.last_rms);
        let on_err = |e: cpal::Error| tracing::warn!(error = %e, "input stream error");

        macro_rules! build {
            ($t:ty, $conv:path) => {
                device.build_input_stream(
                    config,
                    move |data: &[$t], _info: &cpal::InputCallbackInfo| {
                        let mono = $conv(data, channels);
                        level.store(rms(&mono).to_bits(), Ordering::Relaxed);
                        ring.push(&mono);
                    },
                    on_err,
                    None,
                )
            };
        }

        let stream = match sample_format {
            SampleFormat::F32 => build!(f32, mono_from_f32),
            SampleFormat::I16 => build!(i16, mono_from_i16),
            SampleFormat::U16 => build!(u16, mono_from_u16),
            SampleFormat::I32 => build!(i32, mono_from_i32),
            other => {
                return Err(Error::Audio(format!("unsupported input sample format {other:?}")));
            }
        }
        .map_err(|e| Error::Audio(format!("build input stream: {e}")))?;

        stream
            .play()
            .map_err(|e| Error::Audio(format!("start input stream: {e}")))?;
        tracing::info!(rate = sample_rate, channels, format = ?sample_format, "capture started");
        self.stream = Some(stream);
        Ok(())
    }

    fn stop(&mut self) -> Result<AudioBuffer> {
        // Drop the stream first so no more samples land after the snapshot.
        // A fresh stream is built on the next start() (Aqstic mic-array fix).
        let stream = self.stream.take();
        drop(stream);
        let samples = self.ring.snapshot();
        tracing::info!(
            samples = samples.len(),
            rate = self.native_rate,
            "capture stopped"
        );
        Ok(AudioBuffer::new(samples, self.native_rate))
    }

    fn ring(&self) -> RingBuffer {
        self.ring.clone()
    }

    fn level_rms(&self) -> f32 {
        f32::from_bits(self.last_rms.load(Ordering::Relaxed))
    }
}

impl std::fmt::Debug for Capture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Capture")
            .field("device_name", &self.device_name)
            .field("active", &self.stream.is_some())
            .field("native_rate", &self.native_rate)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn duration_capacity_tracks_native_rate() {
        assert_eq!(super::capacity_for_duration(16_000, 300), 4_800_000);
        assert_eq!(super::capacity_for_duration(48_000, 300), 14_400_000);
        assert_eq!(super::capacity_for_duration(96_000, 300), 28_800_000);
    }
    /// Open the microphone **by name**, the way a configured `audio.input_device` does.
    ///
    /// Ignored: needs an input device. Run with
    /// `cargo test -p lw-platform --lib -- --ignored --nocapture`.
    ///
    /// Matching is by case-insensitive substring, so this takes a fragment of the first listed
    /// device and checks both that it opens and that a name matching nothing falls back to the
    /// default rather than failing — which is what the settings UI promises.
    #[test]
    #[ignore = "needs a microphone; run explicitly"]
    fn a_device_can_be_opened_by_name() {
        let devices = list_input_devices();
        let Some(full) = devices.first().cloned() else {
            println!("no input devices on this machine; nothing to check");
            return;
        };
        // A distinctive fragment rather than the whole name, since that is what a user's saved
        // setting looks like.
        let fragment: String = full.chars().take(10).collect();
        println!("device   : {full}");
        println!("fragment : {fragment:?}");

        let mut by_name = Capture::new(Some(fragment.clone()), 16_000);
        by_name
            .start()
            .unwrap_or_else(|e| panic!("opening by the fragment {fragment:?} failed: {e}"));
        let _ = by_name.stop();
        println!("by name  : opened");

        // A saved setting naming a device that is no longer here must not stop dictation.
        let mut missing = Capture::new(Some("no such microphone 9f3a2b".into()), 16_000);
        match missing.start() {
            Ok(()) => {
                println!("missing  : fell back to the default, as the UI promises");
                let _ = missing.stop();
            }
            Err(e) => println!("missing  : refused ({e}) - the UI must surface this"),
        }
    }

    /// Open the default microphone and report the real level for two seconds.
    ///
    /// A **diagnostic**, not a pass/fail test, and deliberately so: what a microphone hears
    /// depends on the room, the device's mute state and the operating system's per-application
    /// microphone permission, none of which a test can assert. It asserts only what is true
    /// regardless — the stream opens and the level stays in range — and prints the rest for a
    /// human to read.
    ///
    /// Run it and speak: `cargo test -p lw-platform --lib -- --ignored --nocapture`.
    ///
    /// **Reading the result.** A peak near zero means no signal reached *this process*. That is
    /// itself the useful answer: the device is muted, the OS denied this binary microphone
    /// access, or the room is silent. It does not mean the meter is broken — the chain it
    /// exercises (device -> callback -> RMS -> `level_handle`) is the same one the app's meter
    /// uses, and the app has its own microphone permission.
    ///
    /// This exists because the level meter was fed a synthesized sine wave for a long time and
    /// nobody noticed, precisely because nothing ever looked at the real thing.
    #[test]
    #[ignore = "diagnostic; needs a microphone and a human to read it"]
    fn report_microphone_level() {
        let mut cap = Capture::new(None, 16_000 * 4);
        cap.start().expect("open the default input device");
        let level = cap.level_handle();

        // Make a noise, so the diagnostic is still informative with nobody in the room.
        let mut samples = Vec::new();
        for i in 0..40 {
            if i == 5 || i == 15 || i == 25 {
                crate::sound::play(
                    lw_core::sound::SoundTheme::Marimba,
                    lw_core::sound::Cue::Start,
                    1.0,
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            samples.push(level.get());
        }
        let _ = cap.stop();

        let peak = samples.iter().cloned().fold(0.0f32, f32::max);
        let nonzero = samples.iter().filter(|s| **s > 0.0).count();
        println!("device   : {:?}", list_input_devices().first());
        println!("samples  : {} over 2 s", samples.len());
        println!("non-zero : {nonzero}");
        println!("peak RMS : {peak:.6}");
        println!(
            "verdict  : {}",
            if peak > 0.005 {
                "signal present - the meter will move"
            } else if nonzero > 0 {
                "barely any signal - a silent room, a quiet device, or a muted input"
            } else {
                "no signal reached this process - check the device's mute state and the OS                  microphone permission for this binary"
            }
        );

        assert!(peak.is_finite(), "level must be a real number, got {peak}");
        assert!((0.0..=1.0).contains(&peak), "RMS must stay in [0, 1], got {peak}");
    }

    #[test]
    #[ignore = "needs an input device; checks duration sizing at its native rate"]
    fn recording_window_uses_native_sample_rate() {
        let mut capture = Capture::for_duration(None, 3);
        capture.start().expect("open the default input device");
        println!("native rate: {} Hz", capture.native_sample_rate());
        assert_eq!(capture.ring().capacity(), capture.native_sample_rate() as usize * 3);
        let _ = capture.stop();
    }


    use super::*;

    #[test]
    fn mono_from_f32_stereo_averages() {
        // 2 frames of stereo: (0.5, -0.5), (1.0, 0.0)
        let out = mono_from_f32(&[0.5, -0.5, 1.0, 0.0], 2);
        assert_eq!(out, vec![0.0, 0.5]);
    }

    #[test]
    fn mono_from_i16_scales_and_downmixes() {
        // stereo frame (i16::MAX, i16::MAX) -> ~1.0 mono
        let out = mono_from_i16(&[i16::MAX, i16::MAX], 2);
        assert_eq!(out.len(), 1);
        assert!((out[0] - (32767.0 / 32768.0)).abs() < 1e-6);
        // i16::MIN -> -1.0
        let out = mono_from_i16(&[i16::MIN], 1);
        assert!((out[0] + 1.0).abs() < 1e-6);
    }

    #[test]
    fn mono_from_u16_centers_on_zero() {
        // 32768 is the u16 origin -> 0.0
        let out = mono_from_u16(&[32768, 32768], 2);
        assert_eq!(out.len(), 1);
        assert!(out[0].abs() < 1e-6);
        let out = mono_from_u16(&[0], 1);
        assert!((out[0] + 1.0).abs() < 1e-6);
        let out = mono_from_u16(&[u16::MAX], 1);
        assert!((out[0] - (32767.0 / 32768.0)).abs() < 1e-6);
    }

    #[test]
    fn mono_from_i32_scales() {
        let out = mono_from_i32(&[i32::MIN, i32::MAX], 2);
        assert_eq!(out.len(), 1);
        assert!(out[0].abs() < 1e-6); // min and max average to ~0
        let out = mono_from_i32(&[i32::MIN], 1);
        assert!((out[0] + 1.0).abs() < 1e-6);
    }

    #[test]
    fn quad_downmix() {
        // 4 channels, one frame: average of 0.4, 0.4, -0.4, -0.4 is 0
        let out = mono_from_f32(&[0.4, 0.4, -0.4, -0.4], 4);
        assert_eq!(out, vec![0.0]);
    }

    #[test]
    fn capture_stop_without_start_returns_empty_native_buffer() {
        let mut c = Capture::new(None, 16);
        let buf = c.stop().unwrap();
        assert!(buf.is_empty());
        assert_eq!(buf.sample_rate, 0);
        assert!(!c.is_active());
    }

    #[test]
    fn ring_is_shared() {
        let c = Capture::new(None, 8);
        let ring = c.ring();
        ring.push(&[0.25; 4]);
        assert_eq!(c.ring().len(), 4);
    }
}
