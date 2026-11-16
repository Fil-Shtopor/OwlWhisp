use std::{
    io::Read,
    net::TcpListener,
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use lw_core::{settings::TranslationBackend, text::TextLanguage};
use serde_json::{Value, json};

use super::{is_ready, manifest, root, runtime_backend, runtime_dir, server_in};

type ChildRef = Arc<Mutex<Child>>;
static CHILDREN: OnceLock<Mutex<Vec<Weak<Mutex<Child>>>>> = OnceLock::new();
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

/// A local, authenticated loopback worker. Dropping it releases its model and accelerator memory.
pub struct Translator {
    child: ChildRef,
    client: reqwest::blocking::Client,
    url: String,
    key: String,
    backend: &'static str,
    stopped: bool,
    #[cfg(windows)]
    _job: Option<Job>,
}

impl Translator {
    /// Start only an explicitly installed, pinned runtime/model. Called while recording starts.
    pub fn start(settings_path: &Path, preference: TranslationBackend) -> Result<Self, String> {
        if !is_ready(settings_path, preference) {
            return Err(
                "Local translator is not ready. Open Settings > Text processing and download the translator."
                    .into(),
            );
        }
        let backend = runtime_backend(preference);
        let directory = runtime_dir(settings_path, backend).ok_or("Translation runtime is missing")?;
        let server = server_in(&directory).ok_or("Translation server is missing")?;
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        drop(listener);
        let key = uuid::Uuid::new_v4().to_string();
        // Build fallible resources before spawning, so any early error cannot orphan a worker.
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_millis(500))
            .build()
            .map_err(|e| e.to_string())?;
        let mut command = Command::new(server);
        command
            .args([
                "--host",
                "127.0.0.1",
                "--port",
                &port.to_string(),
                "-c",
                "4096",
                "-np",
                "1",
                "-t",
                "8",
                "-tb",
                "8",
                "-ngl",
                if backend == "cuda" { "999" } else { "0" },
                "--reasoning",
                "off",
                "--jinja",
                "--cache-ram",
                "0",
                "--no-ui",
                "--api-key",
                &key,
            ])
            .arg("-m")
            .arg(root(settings_path).join(manifest().model.path))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if backend == "cuda" {
            command.args(["--device", "CUDA0"]);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let child = {
            let mut children = CHILDREN
                .get_or_init(Mutex::default)
                .lock()
                .map_err(|e| e.to_string())?;
            if SHUTTING_DOWN.load(Ordering::Acquire) {
                return Err("Application is closing".into());
            }
            let child = Arc::new(Mutex::new(
                command
                    .spawn()
                    .map_err(|e| format!("Cannot start local translator: {e}"))?,
            ));
            children.retain(|w| w.strong_count() > 0);
            children.push(Arc::downgrade(&child));
            child
        };
        let mut worker = Self {
            child,
            // A system HTTP proxy must never receive dictated text destined for loopback.
            client,
            url: format!("http://127.0.0.1:{port}"),
            key,
            backend,
            stopped: false,
            #[cfg(windows)]
            _job: None,
        };
        #[cfg(windows)]
        {
            worker._job = Some(Job::attach(&worker.child)?);
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if SHUTTING_DOWN.load(Ordering::Acquire) {
                return Err("Application is closing".into());
            }
            if !worker.is_alive() {
                return Err(
                    "Local translator stopped during loading. Choose CPU or check the NVIDIA driver.".into(),
                );
            }
            let healthy = worker
                .client
                .get(format!("{}/health", worker.url))
                .bearer_auth(&worker.key)
                .timeout(Duration::from_millis(500))
                .send()
                .is_ok_and(|r| r.status().is_success());
            if healthy {
                return Ok(worker);
            }
            if Instant::now() >= deadline {
                return Err("Local translator loading timed out.".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Actual explicitly selected runtime; CUDA selection requires a CUDA device.
    pub fn backend(&self) -> &'static str {
        self.backend
    }

    /// Whether the owned process is still running after a timeout or manual shutdown.
    pub fn is_alive(&mut self) -> bool {
        if self.stopped {
            return false;
        }
        self.child
            .lock()
            .is_ok_and(|mut c| c.try_wait().is_ok_and(|s| s.is_none()))
    }

    /// Complete one translation within the remaining end-to-end text-stage budget.
    pub fn translate(
        &mut self,
        input: &str,
        target: TextLanguage,
        deadline: Instant,
    ) -> Result<String, String> {
        if input.trim().is_empty() {
            return Ok(input.to_owned());
        }
        if input.chars().count() > 3000 {
            return Err("Dictation is too long for the local translator's bounded context.".into());
        }
        if target == TextLanguage::Auto {
            return Err("Choose a translation target language.".into());
        }
        let result = (|| {
            let message = format!(
                "Translate the following text into {}. Note that you should only output the translated result without any additional explanation:\n{input}",
                target.label()
            );
            let template = self.post(
                "/apply-template",
                &json!({"messages":[{"role":"user","content":message}]}),
                deadline,
            )?;
            let prompt = template["prompt"]
                .as_str()
                .ok_or("Local translator returned no prompt")?;
            let response = self.post("/completion", &json!({"prompt":prompt,"n_predict":1024,"temperature":0,"seed":42,"cache_prompt":false,"stream":false}), deadline)?;
            accept_response(input, &response)
        })();
        if result.is_err() {
            self.stop();
        }
        result
    }

    fn post(&self, path: &str, body: &Value, deadline: Instant) -> Result<Value, String> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("Translation exceeded the maximum extra wait.".into());
        }
        let response = self
            .client
            .post(format!("{}{path}", self.url))
            .bearer_auth(&self.key)
            .timeout(remaining)
            .json(body)
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|e| {
                if e.is_timeout() {
                    "Translation exceeded the maximum extra wait.".to_owned()
                } else {
                    "Local translation request failed.".to_owned()
                }
            })?;
        let mut text = String::new();
        response
            .take(1024 * 1024)
            .read_to_string(&mut text)
            .map_err(|_| "Cannot read the local translation response")?;
        serde_json::from_str(&text).map_err(|_| "Local translator returned an invalid response.".into())
    }

    fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
        // Windows can spend hundreds of milliseconds unmapping model pages. Reap away from
        // the dictation worker so a failed translation doesn't delay delivery of the source.
        let child = self.child.clone();
        if std::thread::Builder::new()
            .name("lw-translation-reap".into())
            .spawn(move || {
                if let Ok(mut child) = child.lock() {
                    let _ = child.wait();
                }
            })
            .is_err()
            && let Ok(mut child) = self.child.lock()
        {
            let _ = child.wait();
        }
    }
}

impl Drop for Translator {
    fn drop(&mut self) {
        self.stop();
    }
}

fn accept_response(input: &str, response: &Value) -> Result<String, String> {
    if response["truncated"] == true || response["stopped_limit"] == true || response["stop_type"] == "limit"
    {
        return Err("Translation was truncated; the original transcript was kept.".into());
    }
    let output = response["content"].as_str().unwrap_or_default().trim();
    if output.is_empty() || output.contains("<think>") || output.contains("<|im_") {
        return Err("Local translator returned no usable text.".into());
    }
    for span in lw_core::text::technical_spans(input) {
        let literal = &input[span];
        let extension = literal.rsplit('.').next().unwrap_or("");
        let technical = literal.contains(['_', '=', '/', '\\', '@'])
            || [
                "json", "toml", "yaml", "yml", "rs", "py", "js", "ts", "tsx", "html", "css", "md", "txt",
                "csv", "xlsx", "docx", "pdf", "log", "ini", "exe", "dll", "com", "org", "net", "io",
            ]
            .contains(&extension)
            || literal.starts_with('v') && literal.chars().nth(1).is_some_and(|c| c.is_ascii_digit());
        if technical && !output.contains(literal) {
            return Err("Translation changed a technical term; the original transcript was kept.".into());
        }
    }
    Ok(output.to_owned())
}

/// Release workers before the GUI's immediate-exit path, including workers still loading.
pub fn stop_all() {
    SHUTTING_DOWN.store(true, Ordering::Release);
    if let Some(children) = CHILDREN.get()
        && let Ok(mut children) = children.lock()
    {
        for child in children.drain(..).filter_map(|w| w.upgrade()) {
            if let Ok(mut c) = child.lock() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }
}

#[cfg(windows)]
struct Job(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
unsafe impl Send for Job {}

#[cfg(windows)]
impl Job {
    fn attach(child: &ChildRef) -> Result<Self, String> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let process = child.lock().map_err(|e| e.to_string())?;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of_val(&info) as u32,
            ) == 0
                || AssignProcessToJobObject(job, process.as_raw_handle() as _) == 0
            {
                let error = std::io::Error::last_os_error().to_string();
                CloseHandle(job);
                return Err(error);
            }
            Ok(Self(job))
        }
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_truncated_empty_or_technical_corruption_but_translates_quoted_prose() {
        assert!(accept_response("Hello", &json!({"content":"Hi","stop_type":"limit"})).is_err());
        assert!(accept_response("Hello", &json!({"content":" "})).is_err());
        assert!(
            accept_response(
                "Use config.json and /api/v2/users",
                &json!({"content":"Use configjson and /api/v2/user"})
            )
            .is_err()
        );
        assert_eq!(
            accept_response(
                "Скажи «привет».",
                &json!({"content":"Say 'hello'.","stop_type":"eos"})
            )
            .unwrap(),
            "Say 'hello'."
        );
        assert_eq!(
            accept_response(
                "Set debug_mode=true in settings.toml.",
                &json!({"content":"Установи debug_mode=true в settings.toml.","stop_type":"eos"})
            )
            .unwrap(),
            "Установи debug_mode=true в settings.toml."
        );
    }
}
