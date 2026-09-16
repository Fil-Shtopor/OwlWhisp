//! Playing the start/stop cues on the default output device.
//!
//! The samples come from [`lw_core::sound`], which synthesizes them at whatever rate the device
//! negotiates — so there is no resampling step here, only format conversion and channel fan-out.
//!
//! Playback is **fire-and-forget and never fatal**. A cue is feedback, not function: if there is
//! no output device, or the user has muted it, or the stream fails to build, dictation must carry
//! on exactly as before. Every failure is logged at debug and swallowed.

use std::time::Duration;

use cpal::SampleFormat;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use lw_core::sound::{Cue, SoundTheme, render};

/// Play `cue` of `theme` on the default output device, at `volume` in `[0, 1]`.
///
/// Returns immediately; the sound plays on a short-lived thread. Errors are logged, not returned:
/// see the module docs for why a missing speaker must never interrupt dictation.
pub fn play(theme: SoundTheme, cue: Cue, volume: f32) {
    let volume = volume.clamp(0.0, 1.0);
    if volume <= 0.0 {
        return;
    }
    std::thread::Builder::new()
        .name("lw-cue".into())
        .spawn(move || {
            if let Err(e) = play_blocking(theme, cue, volume) {
                tracing::debug!("could not play the {cue:?} cue: {e}");
            }
        })
        .ok();
}

/// Render and play one cue, returning the first thing that went wrong.
fn play_blocking(theme: SoundTheme, cue: Cue, volume: f32) -> Result<(), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no default output device".to_string())?;
    let config = device
        .default_output_config()
        .map_err(|e| format!("default output config: {e}"))?;

    let sample_rate = config.sample_rate();
    let channels = config.channels() as usize;
    let mono = render(theme, cue, sample_rate);
    if mono.is_empty() {
        return Err("cue rendered to nothing".into());
    }
    // Long enough for the whole cue plus the device's own latency; the thread waits this out and
    // then drops the stream, which is what actually stops playback.
    let play_for = Duration::from_secs_f32(mono.len() as f32 / sample_rate as f32 + 0.25);

    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let mut cursor = 0usize;
    let stream_config = config.config();
    let err_fn = |e: cpal::Error| tracing::debug!("cue output stream error: {e}");

    // One closure shape per sample format. Each fills the device buffer from `mono`, repeating the
    // mono sample across all channels, and pads with silence once the cue is exhausted.
    macro_rules! build {
        ($t:ty, $convert:expr) => {{
            let samples = mono.clone();
            let tx = done_tx.clone();
            device.build_output_stream(
                stream_config.clone(),
                move |out: &mut [$t], _: &cpal::OutputCallbackInfo| {
                    for frame in out.chunks_mut(channels) {
                        let v = samples.get(cursor).copied().unwrap_or(0.0) * volume;
                        if cursor <= samples.len() {
                            cursor += 1;
                        }
                        if cursor == samples.len() {
                            let _ = tx.send(());
                        }
                        for slot in frame.iter_mut() {
                            *slot = $convert(v);
                        }
                    }
                },
                err_fn,
                None,
            )
        }};
    }

    let stream = match config.sample_format() {
        SampleFormat::F32 => build!(f32, |v: f32| v),
        SampleFormat::I16 => build!(i16, |v: f32| (v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16),
        SampleFormat::U16 => build!(u16, |v: f32| {
            ((v.clamp(-1.0, 1.0) * 0.5 + 0.5) * u16::MAX as f32) as u16
        }),
        other => return Err(format!("unsupported output sample format {other:?}")),
    }
    .map_err(|e| format!("build output stream: {e}"))?;

    stream.play().map_err(|e| format!("play: {e}"))?;
    // Wait for the callback to say it ran out of samples, or for the worst-case duration.
    let _ = done_rx.recv_timeout(play_for);
    // Let the tail actually reach the speaker before the stream is dropped.
    std::thread::sleep(Duration::from_millis(120));
    drop(stream);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silent_volume_does_not_touch_the_audio_device() {
        // Also the CI guarantee: this must not need an output device to run.
        play(SoundTheme::Chime, Cue::Start, 0.0);
        play(SoundTheme::Chime, Cue::Start, -1.0);
    }

    #[test]
    fn playing_without_a_device_is_not_fatal() {
        // On a machine with no output device `play_blocking` errors; `play` must swallow it.
        // Either way this returns, which is the property under test.
        play(SoundTheme::Blip, Cue::Stop, 0.5);
        std::thread::sleep(Duration::from_millis(50));
    }
}
