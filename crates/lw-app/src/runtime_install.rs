//! Hardware-selected, pinned runtime add-ons, installed privately and activated in a worker.
use lw_core::capabilities::{Accelerator, Architecture, OperatingSystem, Platform};
use serde::Deserialize;
use sha2::{Digest, Sha256, Sha512};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Debug, Deserialize)]
struct Package {
    id: String,
    url: String,
    #[serde(default)]
    sha256: String,
    #[serde(default)]
    sha512: String,
    size: u64,
    files: BTreeMap<String, String>,
    #[serde(default)]
    bundled_sha256: BTreeMap<String, String>,
}

fn packages() -> Vec<Package> {
    serde_json::from_str(include_str!("runtime-packages.json")).expect("reviewed runtime package lock")
}

/// The action which can actually make a detected accelerator usable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum SetupAction {
    DownloadRuntime,
    InstallDriver,
}

/// Missing runtime packages and missing drivers are independent of missing hardware.
pub fn setup_action(accel: Accelerator, hardware: bool, usable: bool) -> Option<SetupAction> {
    let devices = if matches!(accel, Accelerator::Cuda | Accelerator::TensorRt)
        && hardware
        && !usable
        && downloadable_platform(Platform::current())
    {
        lw_ort::nvidia::devices().unwrap_or_default()
    } else {
        Vec::new()
    };
    setup_action_on(Platform::current(), accel, hardware, usable, &devices)
}

fn downloadable_platform(platform: Platform) -> bool {
    platform.os == OperatingSystem::Windows
        && matches!(platform.arch, Architecture::X64 | Architecture::Arm64)
}

/// Select an actual installation action from explicit platform and driver observations.
pub fn setup_action_on(
    platform: Platform,
    accel: Accelerator,
    hardware: bool,
    usable: bool,
    devices: &[lw_ort::nvidia::NvidiaDevice],
) -> Option<SetupAction> {
    if !hardware || usable || !downloadable_platform(platform) {
        return None;
    }
    if platform.arch == Architecture::Arm64 && !matches!(accel, Accelerator::DirectMl | Accelerator::WebGpu) {
        return None;
    }
    match accel {
        Accelerator::Cuda | Accelerator::TensorRt => match devices.first() {
            None => Some(SetupAction::InstallDriver),
            Some(gpu) if gpu.major * 10 + gpu.minor < 75 => None,
            Some(gpu) if gpu.driver_cuda_version < 13000 => Some(SetupAction::InstallDriver),
            Some(gpu) if accel == Accelerator::TensorRt && gpu.tensorrt_resource().is_none() => None,
            Some(_) => Some(SetupAction::DownloadRuntime),
        },
        Accelerator::DirectMl | Accelerator::WebGpu => Some(SetupAction::DownloadRuntime),
        _ => None,
    }
}

fn selected_packages(accel: Accelerator, resource: Option<&str>) -> Result<Vec<Package>, String> {
    let keys: Vec<&str> = match accel {
        Accelerator::Cuda => vec!["cuda-core", "cudart", "cublas", "cufft", "curand", "cudnn"],
        Accelerator::TensorRt => vec![
            "cuda-core",
            "cudart",
            "cublas",
            "cufft",
            "curand",
            "cudnn",
            "nvinfer_10",
            "nvinfer_plugin_10",
            "nvonnxparser_10",
            resource.ok_or("no pinned TensorRT resource for this GPU")?,
        ],
        Accelerator::DirectMl => vec!["directml"],
        Accelerator::WebGpu => vec!["webgpu"],
        _ => return Err("no downloadable runtime for this accelerator".into()),
    };
    let catalog = packages();
    keys.into_iter()
        .map(|key| {
            catalog
                .iter()
                .find(|p| p.id == key)
                .cloned()
                .ok_or_else(|| format!("missing pinned package {key}"))
        })
        .collect()
}

fn plan(accel: Accelerator) -> Result<Vec<Package>, String> {
    let platform = Platform::current();
    if !downloadable_platform(platform) {
        return Err("runtime add-on downloads currently support Windows x64/ARM64".into());
    }
    let devices = if matches!(accel, Accelerator::Cuda | Accelerator::TensorRt) {
        lw_ort::nvidia::devices()?
    } else {
        Vec::new()
    };
    plan_on(platform, accel, &devices)
}

/// The pinned package IDs selected for a target, without downloading or probing native APIs.
pub fn package_ids_on(
    platform: Platform,
    accel: Accelerator,
    devices: &[lw_ort::nvidia::NvidiaDevice],
) -> Result<Vec<String>, String> {
    plan_on(platform, accel, devices).map(|packages| packages.into_iter().map(|p| p.id).collect())
}

fn plan_on(
    platform: Platform,
    accel: Accelerator,
    devices: &[lw_ort::nvidia::NvidiaDevice],
) -> Result<Vec<Package>, String> {
    if !downloadable_platform(platform) {
        return Err("runtime add-on downloads currently support Windows x64/ARM64".into());
    }
    if platform.arch == Architecture::Arm64 {
        let key = match accel {
            Accelerator::DirectMl => "directml-arm64",
            Accelerator::WebGpu => "webgpu-arm64",
            _ => return Err(
                "this accelerator has no pinned Windows ARM64 runtime; use DirectML or WebGPU on RTX Spark"
                    .into(),
            ),
        };
        return packages()
            .into_iter()
            .find(|p| p.id == key)
            .map(|package| vec![package])
            .ok_or_else(|| format!("missing pinned package {key}"));
    }
    let gpu = if matches!(accel, Accelerator::Cuda | Accelerator::TensorRt) {
        let gpu = devices.first().ok_or("no NVIDIA GPU detected")?;
        if gpu.driver_cuda_version < 13000 {
            return Err("update the NVIDIA driver before installing CUDA 13 libraries".into());
        }
        if gpu.major * 10 + gpu.minor < 75 {
            return Err(
                "this CUDA 13 runtime requires compute capability 7.5 or newer; use DirectML or WebGPU"
                    .into(),
            );
        }
        Some(gpu)
    } else {
        None
    };
    let resource = gpu.and_then(|g| g.tensorrt_resource());
    selected_packages(accel, resource.as_deref())
}

fn directory(accel: Accelerator, selected: &[Package]) -> PathBuf {
    let mut hash = Sha256::new();
    for p in selected {
        hash.update(&p.id);
        hash.update(&p.sha256);
        hash.update(&p.sha512);
    }
    crate::paths::app_data_dir()
        .join("runtimes")
        .join(Platform::current().runtime_dir().unwrap_or("unsupported"))
        .join(format!("{}-{}", accel.id(), &hex::encode(hash.finalize())[..16]))
}

/// Installed immutable runtime for the current hardware and pinned package versions.
pub fn installed_runtime(accel: Accelerator) -> Option<PathBuf> {
    let selected = plan(accel).ok()?;
    let base = directory(accel, &selected);
    let dir = active_directory(&base, accel);
    (dir.join("installed.json").is_file()
        && dir.join(lw_ort::onnxruntime_lib_name()).is_file()
        && selected
            .iter()
            .all(|p| p.files.values().all(|name| dir.join(name).is_file())))
    .then_some(dir)
}

fn active_directory(base: &Path, accel: Accelerator) -> PathBuf {
    let Some(root) = base.parent() else {
        return base.to_path_buf();
    };
    let prefix = base.file_name().unwrap_or_default().to_string_lossy();
    fs::read(root.join(format!("{}-active.json", accel.id())))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<String>(&bytes).ok())
        .filter(|name| {
            Path::new(name).components().count() == 1
                && (name == prefix.as_ref() || name.starts_with(&format!("{prefix}-")))
        })
        .map(|name| root.join(name))
        .unwrap_or_else(|| base.to_path_buf())
}

/// A snapshot for the download button; it is updated only by the install thread.
#[derive(Clone, Debug)]
pub struct Progress {
    pub label: String,
    pub fraction: Option<f32>,
}

/// Download operation with cancellation and a terminal result.
pub struct Handle {
    progress: Arc<Mutex<Progress>>,
    cancelled: Arc<AtomicBool>,
    done: crossbeam_channel::Receiver<Result<PathBuf, String>>,
}
impl Handle {
    pub fn progress(&self) -> Progress {
        self.progress.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
    pub fn poll(&self) -> Option<Result<PathBuf, String>> {
        self.done.try_recv().ok()
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Start installation without blocking the window. No system directories are changed.
pub fn start(accel: Accelerator) -> Handle {
    let progress = Arc::new(Mutex::new(Progress {
        label: "Preparing download...".into(),
        fraction: None,
    }));
    let cancelled = Arc::new(AtomicBool::new(false));
    let (tx, done) = crossbeam_channel::bounded(1);
    let (state, stop) = (progress.clone(), cancelled.clone());
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(|| install(accel, &state, &stop))
            .unwrap_or_else(|_| Err("runtime installer stopped unexpectedly".into()));
        let _ = tx.send(result);
    });
    Handle {
        progress,
        cancelled,
        done,
    }
}

fn update(state: &Mutex<Progress>, label: impl Into<String>, fraction: Option<f32>) {
    *state.lock().unwrap_or_else(|e| e.into_inner()) = Progress {
        label: label.into(),
        fraction,
    };
}
fn check_cancel(stop: &AtomicBool) -> Result<(), String> {
    if stop.load(Ordering::Relaxed) {
        Err("Download cancelled; click Download runtime to resume.".into())
    } else {
        Ok(())
    }
}
fn digest_file(path: &Path, sha512: bool) -> Result<String, String> {
    let mut input = File::open(path).map_err(|e| e.to_string())?;
    let mut buffer = [0u8; 128 * 1024];
    let (mut h256, mut h512) = (Sha256::new(), Sha512::new());
    loop {
        let n = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        if sha512 {
            h512.update(&buffer[..n]);
        } else {
            h256.update(&buffer[..n]);
        }
    }
    Ok(if sha512 {
        hex::encode(h512.finalize())
    } else {
        hex::encode(h256.finalize())
    })
}
fn verified_archive(path: &Path, package: &Package) -> bool {
    fs::metadata(path).is_ok_and(|m| m.len() == package.size)
        && digest_file(path, !package.sha512.is_empty()).is_ok_and(|h| {
            &h == if package.sha512.is_empty() {
                &package.sha256
            } else {
                &package.sha512
            }
        })
}

fn extract(archive: &Path, package: &Package, dest: &Path) -> Result<(), String> {
    let mut zip =
        zip::ZipArchive::new(File::open(archive).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    for (member, name) in &package.files {
        if Path::new(name).components().count() != 1 || name == ".." {
            return Err("invalid runtime file name".into());
        }
        let mut input = zip
            .by_name(member)
            .map_err(|e| format!("{} missing {member}: {e}", package.id))?;
        if input.size() > 4 * 1024 * 1024 * 1024 {
            return Err("runtime member exceeds size limit".into());
        }
        let mut output = File::create(dest.join(name)).map_err(|e| e.to_string())?;
        std::io::copy(&mut input, &mut output).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn install(accel: Accelerator, state: &Mutex<Progress>, stop: &AtomicBool) -> Result<PathBuf, String> {
    let selected = plan(accel)?;
    let dest = directory(accel, &selected);
    if let Some(existing) = installed_runtime(accel) {
        update(state, "Checking accelerator...", None);
        if crate::provider_worker::probe_runtime(&existing, accel).is_ok() {
            return Ok(existing);
        }
    }
    let root = dest.parent().ok_or("missing runtime root")?;
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let stage = root.join(format!(
        ".{}-{}",
        dest.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    let cache = crate::paths::app_data_dir().join("downloads/runtimes");
    fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    let base = lw_ort::locate_runtime_dir().ok_or("bundled ONNX Runtime not found")?;
    if accel == Accelerator::WebGpu {
        // WebGPU is a plugin; use the same bundled core, never a different core in this process.
        fs::copy(
            base.join(lw_ort::onnxruntime_lib_name()),
            stage.join(lw_ort::onnxruntime_lib_name()),
        )
        .map_err(|e| e.to_string())?;
    }
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(std::time::Duration::from_secs(1800))
        .build()
        .map_err(|e| e.to_string())?;
    let total_bytes: u64 = selected.iter().map(|p| p.size).sum();
    let shared_cuda = (accel == Accelerator::TensorRt)
        .then(|| installed_runtime(Accelerator::Cuda))
        .flatten();
    for (index, package) in selected.iter().enumerate() {
        check_cancel(stop)?;
        update(
            state,
            format!("Checking files {}/{}", index + 1, selected.len()),
            None,
        );
        let bundle = if accel == Accelerator::DirectMl {
            crate::provider_worker::directml_runtime_dir(&base)
        } else {
            base.clone()
        };
        let sources: Vec<_> = std::iter::once(&bundle).chain(shared_cuda.as_ref()).collect();
        let reusable = sources.into_iter().find(|source| {
            package.files.values().all(|name| {
                package.bundled_sha256.get(name).is_some_and(|expected| {
                    digest_file(&source.join(name), false).is_ok_and(|actual| &actual == expected)
                })
            })
        });
        if let Some(source) = reusable {
            for name in package.files.values() {
                check_cancel(stop)?;
                let target = stage.join(name);
                if target.exists() {
                    fs::remove_file(&target).map_err(|e| e.to_string())?;
                }
                // Only private immutable add-ons share storage. Bundled/system files are copied.
                if !source.starts_with(crate::paths::app_data_dir().join("runtimes"))
                    || fs::hard_link(source.join(name), &target).is_err()
                {
                    fs::copy(source.join(name), &target).map_err(|e| e.to_string())?;
                }
            }
            continue;
        }
        let hash = if package.sha512.is_empty() {
            &package.sha256
        } else {
            &package.sha512
        };
        let archive = cache.join(format!("{}-{}.zip", package.id, &hash[..16]));
        if !verified_archive(&archive, package) {
            let partial = archive.with_extension("partial");
            let offset = fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);
            let mut request = client.get(&package.url);
            if offset > 0 && offset < package.size {
                request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
            }
            let mut response = request
                .send()
                .and_then(|r| r.error_for_status())
                .map_err(|e| format!("download {}: {e}", package.id))?;
            let resume = offset > 0 && response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
            if resume
                && response
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|h| h.to_str().ok())
                    .is_none_or(|v| !v.starts_with(&format!("bytes {offset}-")))
            {
                return Err("invalid download resume response".into());
            }
            let mut output = fs::OpenOptions::new()
                .create(true)
                .write(true)
                .append(resume)
                .truncate(!resume)
                .open(&partial)
                .map_err(|e| e.to_string())?;
            let mut received = if resume { offset } else { 0 };
            let mut buffer = [0u8; 128 * 1024];
            loop {
                check_cancel(stop)?;
                let n = response.read(&mut buffer).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                output.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
                received += n as u64;
                if received > package.size {
                    return Err("download exceeds pinned package size".into());
                }
                update(
                    state,
                    format!("Downloading {} ({}/{})", package.id, index + 1, selected.len()),
                    Some(
                        (selected[..index].iter().map(|p| p.size).sum::<u64>() + received) as f32
                            / total_bytes as f32,
                    ),
                );
            }
            drop(output);
            update(state, "Verifying package...", None);
            if !verified_archive(&partial, package) {
                let _ = fs::remove_file(&partial);
                return Err(format!(
                    "checksum mismatch for {}; click Download runtime to retry",
                    package.id
                ));
            }
            // Only an archive verified against the embedded lock can be extracted.
            if archive.exists() {
                fs::remove_file(&archive).map_err(|e| e.to_string())?;
            }
            fs::rename(partial, &archive).map_err(|e| e.to_string())?;
        }
        update(state, format!("Installing {}...", package.id), None);
        extract(&archive, package, &stage)?;
    }
    check_cancel(stop)?;
    update(state, "Checking accelerator...", None);
    crate::provider_worker::probe_runtime(&stage, accel)?;
    check_cancel(stop)?;
    fs::write(stage.join("installed.json"), serde_json::to_vec_pretty(&serde_json::json!({"accelerator":accel,"packages":selected.iter().map(|p| &p.id).collect::<Vec<_>>()})).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    // Never overwrite DLLs already loaded by a running worker.
    let published = if dest.exists() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        root.join(format!(
            "{}-{suffix}",
            dest.file_name().unwrap().to_string_lossy()
        ))
    } else {
        dest.clone()
    };
    fs::rename(&stage, &published).map_err(|e| e.to_string())?;
    let pending_index = root.join(format!(".{}-active-{}.json", accel.id(), std::process::id()));
    fs::write(
        &pending_index,
        serde_json::to_vec(&published.file_name().unwrap().to_string_lossy()).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::rename(pending_index, root.join(format!("{}-active.json", accel.id()))).map_err(|e| e.to_string())?;
    Ok(published)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_the_matching_tensorrt_partition_is_selected() {
        for sm in [75, 80, 86, 89, 90, 120] {
            let resource = format!("nvinfer_builder_resource_sm{sm}_10");
            let chosen = selected_packages(Accelerator::TensorRt, Some(&resource)).unwrap();
            assert_eq!(
                chosen
                    .iter()
                    .filter(|p| p.id.starts_with("nvinfer_builder_resource"))
                    .count(),
                1
            );
            assert!(chosen.iter().any(|p| p.id == resource));
        }
        assert!(selected_packages(Accelerator::TensorRt, None).is_err());
        assert!(selected_packages(Accelerator::TensorRt, Some("unknown")).is_err());
        assert!(
            selected_packages(Accelerator::Cuda, None)
                .unwrap()
                .iter()
                .all(|p| !p.id.starts_with("nvinfer"))
        );
        assert_eq!(selected_packages(Accelerator::DirectMl, None).unwrap().len(), 1);
    }
    #[test]
    fn every_executable_package_is_pinned_and_has_flat_output_paths() {
        for p in packages() {
            assert!(
                p.url.starts_with("https://api.nuget.org/")
                    || p.url.starts_with("https://developer.download.nvidia.com/")
            );
            assert!(p.sha256.len() == 64 || p.sha512.len() == 128);
            assert!(p.size > 0);
            assert!(p.files.values().all(|n| Path::new(n).components().count() == 1));
        }
    }
    #[test]
    fn active_index_cannot_escape_root_or_select_an_old_package_set() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().join("cuda-current-lock");
        let index = root.path().join("cuda-active.json");
        for invalid in [
            "../outside",
            "cuda-old-lock",
            "C:/Windows",
            "cuda-current-lock/../escape",
        ] {
            fs::write(&index, serde_json::to_vec(invalid).unwrap()).unwrap();
            assert_eq!(active_directory(&base, Accelerator::Cuda), base);
        }
        fs::write(&index, serde_json::to_vec("cuda-current-lock-repair").unwrap()).unwrap();
        assert_eq!(
            active_directory(&base, Accelerator::Cuda),
            root.path().join("cuda-current-lock-repair")
        );
    }
    #[test]
    fn corrupted_archives_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.zip");
        fs::write(&path, b"bad").unwrap();
        let mut p = packages().remove(0);
        p.size = 3;
        assert!(!verified_archive(&path, &p));
    }
}
