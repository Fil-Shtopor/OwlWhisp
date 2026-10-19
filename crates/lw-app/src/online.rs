//! Optional, explicitly selected OpenAI-compatible audio transcription. No requests at startup.

use std::io::{Cursor, Read};
use std::time::Instant;

use lw_core::audio::{AudioBuffer, TARGET_SAMPLE_RATE};
use lw_core::engine::{
    Acceleration, DeviceInfo, EngineInitContext, HealthReport, Language, Provider, SpeechEngine, Transcript,
};
use reqwest::blocking::{Client, multipart};
use serde::Deserialize;

/// Accept HTTPS endpoints and HTTP loopback servers, without credentials hidden in the URL.
pub fn validate_endpoint(base_url: &str, model: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(base_url.trim()).map_err(|_| "enter a valid API base URL".to_string())?;
    let loopback = url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if url.host_str().is_none() || !(url.scheme() == "https" || (url.scheme() == "http" && loopback)) {
        return Err("use HTTPS for an online API, or HTTP for a local loopback server".into());
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "API URL must not contain credentials, a query, or a fragment; put the key in API key".into(),
        );
    }
    if model.trim().is_empty() || model.chars().count() > 200 || model.chars().any(char::is_control) {
        return Err("enter the transcription model ID supplied by your API provider".into());
    }
    let base = url.as_str().trim_end_matches('/');
    if base.ends_with("/audio/transcriptions") {
        return Err("enter the API base URL (usually ending in /v1), without /audio/transcriptions".into());
    }
    Ok(base.to_string())
}

pub struct OnlineEngine {
    base_url: String,
    model: String,
    key: String,
    client: Client,
    ready: bool,
}

impl OnlineEngine {
    pub fn new(base_url: &str, model: &str, key: String) -> Result<Self, String> {
        let base_url = validate_endpoint(base_url, model)?;
        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(120))
            // A redirect must never send the user's recording to a different endpoint.
            .redirect(reqwest::redirect::Policy::none())
            .build().map_err(|_| "could not initialize the transcription HTTP client".to_string())?;
        Ok(Self {
            base_url,
            model: model.trim().to_string(),
            key,
            client,
            ready: false,
        })
    }

    fn request(&self, audio: &AudioBuffer) -> Result<Transcript, String> {
        if audio.samples.len() > (24 * 1024 * 1024 - 44) / 2 {
            return Err("recording exceeds the 24 MiB online upload limit; use shorter recordings".into());
        }
        let wav = encode_wav(audio)?;
        if wav.len() > 24 * 1024 * 1024 {
            return Err("recording exceeds the 24 MiB online upload limit; use shorter recordings".into());
        }
        let part = multipart::Part::bytes(wav)
            .file_name("dictation.wav")
            .mime_str("audio/wav")
            .map_err(|_| "could not prepare the audio upload".to_string())?;
        let form = multipart::Form::new()
            .text("model", self.model.clone())
            .text("response_format", "json")
            .part("file", part);
        let mut request = self
            .client
            .post(format!("{}/audio/transcriptions", self.base_url))
            .multipart(form);
        if !self.key.is_empty() {
            request = request.bearer_auth(&self.key);
        }
        let response = request.send().map_err(|error| {
            // reqwest errors include URLs; neither raw errors nor response bodies are surfaced.
            if error.is_timeout() {
                "online transcription timed out; check the connection and retry".to_string()
            } else {
                "could not reach the transcription API; check the URL and connection".to_string()
            }
        })?;
        let status = response.status();
        if !status.is_success() {
            let reason = match status.as_u16() {
                401 | 403 => "check the API key and your access to this model",
                404 => "check the API base URL and model ID",
                413 => "the recording is too large for this service",
                429 => "the service rate limit or API quota was reached; check your API account",
                500..=599 => "the transcription service is temporarily unavailable",
                _ => "check that the service supports the audio transcription API",
            };
            return Err(format!("online transcription HTTP {status}: {reason}"));
        }
        let mut bytes = Vec::new();
        response
            .take(2 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "could not read the transcription response".to_string())?;
        if bytes.len() > 2 * 1024 * 1024 {
            return Err("transcription response is too large".into());
        }
        let response: TranscriptionResponse = serde_json::from_slice(&bytes)
            .map_err(|_| "the API did not return a JSON transcription with a text field".to_string())?;
        Ok(Transcript::from_text(response.text.trim()))
    }
}

impl SpeechEngine for OnlineEngine {
    fn backend_name(&self) -> &str {
        &self.model
    }
    fn provider(&self) -> Provider {
        Provider::Remote
    }
    fn device(&self) -> DeviceInfo {
        DeviceInfo::new(format!("Online — {}", self.base_url))
    }
    fn acceleration(&self) -> Acceleration {
        Acceleration::Remote
    }
    fn supported_languages(&self) -> &[Language] {
        &[]
    }
    fn supports_streaming(&self) -> bool {
        false
    }
    fn initialize(&mut self, _ctx: &EngineInitContext) -> lw_core::Result<()> {
        self.ready = true;
        Ok(())
    }
    fn health_check(&mut self) -> HealthReport {
        // Only the explicit Test connection action calls this, never preloading or benchmarks.
        let started = Instant::now();
        let answer = self.request(&AudioBuffer::new(
            vec![0.0; TARGET_SAMPLE_RATE as usize],
            TARGET_SAMPLE_RATE,
        ));
        HealthReport {
            ok: answer.is_ok(),
            provider: Provider::Remote,
            probe_latency_ms: answer
                .as_ref()
                .ok()
                .map(|_| started.elapsed().as_secs_f32() * 1000.0),
            message: answer
                .map(|_| "connection verified with one second of silent audio".into())
                .unwrap_or_else(|e| e),
        }
    }
    fn transcribe(&mut self, audio: &AudioBuffer) -> lw_core::Result<Transcript> {
        if !self.ready {
            return Err(lw_core::Error::Unavailable(
                "online engine is not initialized".into(),
            ));
        }
        if audio.is_empty() {
            return Ok(Transcript::from_text(""));
        }
        self.request(audio).map_err(lw_core::Error::other)
    }
}

#[derive(Deserialize)]
struct TranscriptionResponse {
    text: String,
}

fn encode_wav(audio: &AudioBuffer) -> Result<Vec<u8>, String> {
    if audio.sample_rate == 0 || audio.samples.iter().any(|s| !s.is_finite()) {
        return Err("recording contains invalid audio samples or sample rate".into());
    }
    let mut buffer = Cursor::new(Vec::with_capacity(audio.samples.len().saturating_mul(2) + 44));
    {
        let mut writer = hound::WavWriter::new(
            &mut buffer,
            hound::WavSpec {
                channels: 1,
                sample_rate: audio.sample_rate,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .map_err(|e| e.to_string())?;
        for sample in &audio.samples {
            writer
                .write_sample((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
                .map_err(|e| e.to_string())?;
        }
        writer.finalize().map_err(|e| e.to_string())?;
    }
    Ok(buffer.into_inner())
}

/// An explicit test uploads synthetic silence and may consume the provider's API quota.
pub fn test_connection(base_url: &str, model: &str, key: String) -> Result<String, String> {
    let mut engine = OnlineEngine::new(base_url, model, key)?;
    let report = engine.health_check();
    if report.ok {
        Ok(report.message)
    } else {
        Err(report.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn endpoint_validation_rejects_credentials_and_insecure_remote_hosts() {
        for url in [
            "http://example.com/v1",
            "https://user:secret@example.com/v1",
            "https://example.com/v1?key=secret",
            "https://example.com/v1#secret",
            "file:///model",
            "https://example.com/v1/audio/transcriptions",
        ] {
            assert!(validate_endpoint(url, "whisper-1").is_err(), "{url}");
        }
        assert_eq!(
            validate_endpoint(" https://example.com/v1/ ", "whisper-1").unwrap(),
            "https://example.com/v1"
        );
        assert!(validate_endpoint("http://127.0.0.1:8080/v1", "mine").is_ok());
        assert!(validate_endpoint("http://[::1]:8080/v1", "mine").is_ok());
    }

    #[test]
    fn wav_is_pcm16_mono_and_rejects_invalid_audio() {
        let bytes = encode_wav(&AudioBuffer::new(vec![-2.0, 0.0, 2.0], 16000)).unwrap();
        let mut reader = hound::WavReader::new(Cursor::new(bytes)).unwrap();
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.spec().bits_per_sample, 16);
        assert_eq!(
            reader.samples::<i16>().map(Result::unwrap).collect::<Vec<_>>(),
            vec![-32767, 0, 32767]
        );
        assert!(encode_wav(&AudioBuffer::new(vec![f32::NAN], 16000)).is_err());
    }

    fn mock_server(status: &str, body: &str) -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let redirect = if status.starts_with("302") {
            "Location: http://127.0.0.1:9/never-upload\r\n"
        } else {
            ""
        };
        let response = format!(
            "HTTP/1.1 {status}\r\n{redirect}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buf = [0; 4096];
            loop {
                let n = stream.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&buf[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                    let len = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length: "))
                        .unwrap()
                        .parse::<usize>()
                        .unwrap();
                    if bytes.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8_lossy(&bytes).to_string()
        });
        (url, handle)
    }

    #[test]
    fn uploads_wav_with_auth_and_parses_transcription_without_network_at_initialize() {
        let (url, server) = mock_server("200 OK", r#"{"text":" Hello world "}"#);
        let mut engine = OnlineEngine::new(&url, "my-whisper", "test-key".into()).unwrap();
        engine.initialize(&EngineInitContext::default()).unwrap();
        let result = engine
            .transcribe(&AudioBuffer::new(vec![0.0; 160], 16000))
            .unwrap();
        assert_eq!(result.text, "Hello world");
        assert_eq!(engine.provider(), Provider::Remote);
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /v1/audio/transcriptions "));
        assert!(request.to_lowercase().contains("authorization: bearer test-key"));
        assert!(request.contains("my-whisper"));
        assert!(request.contains("dictation.wav"));
        assert!(request.contains("RIFF"));
    }

    #[test]
    fn errors_do_not_echo_response_secrets_and_redirects_are_not_followed() {
        for (status, body) in [
            ("401 Unauthorized", "secret-transcript"),
            ("429 Too Many Requests", "test-key"),
            ("302 Found", "redirect"),
            ("200 OK", "not json"),
        ] {
            let (url, server) = mock_server(status, body);
            let engine = OnlineEngine::new(&url, "test", "test-key".into()).unwrap();
            let error = engine
                .request(&AudioBuffer::new(vec![0.0; 160], 16000))
                .unwrap_err();
            assert!(!error.contains("test-key"));
            assert!(!error.contains("secret-transcript"));
            if status.starts_with("302") {
                assert!(error.contains("302"), "redirect must not be followed: {error}");
            }
            server.join().unwrap();
        }
    }
}
