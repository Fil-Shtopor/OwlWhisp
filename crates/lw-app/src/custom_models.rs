//! User-owned models and endpoint registrations. Model files are never copied or deleted here.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, TryRecvError, bounded};
#[cfg(feature = "sherpa")]
use lw_core::engine::EngineInitContext;
use lw_core::engine::{BackendPreference, SpeechEngine};
use lw_core::settings::Settings;
use lw_engine_sherpa::{ModelKind, detect_in_dir};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use lw_engine_sherpa::ModelKind as LocalModelKind;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelSource {
    Local { directory: PathBuf, kind: ModelKind },
    Online { base_url: String, model: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CustomModel {
    pub id: String,
    pub name: String,
    pub source: ModelSource,
}

#[derive(Serialize, Deserialize)]
struct Registry {
    version: u32,
    models: Vec<CustomModel>,
}

pub fn registry_path(settings_path: &Path) -> PathBuf {
    settings_path.with_file_name("custom-models.json")
}

pub fn is_custom(id: &str) -> bool {
    id.starts_with("custom-")
}

fn valid_id(id: &str) -> bool {
    is_custom(id) && id.len() == 71 && id[7..].bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn list(settings_path: &Path) -> Result<Vec<CustomModel>, String> {
    let path = registry_path(settings_path);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let registry: Registry =
        serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if registry.version != 1 {
        return Err("unsupported custom model registry version".into());
    }
    let mut ids = std::collections::BTreeSet::new();
    for model in &registry.models {
        if !valid_id(&model.id) || !ids.insert(&model.id) || model.name.trim().is_empty() {
            return Err("invalid or duplicate custom model registration".into());
        }
        match &model.source {
            ModelSource::Local { directory, .. } if !directory.is_absolute() => {
                return Err("custom model directory must be absolute".into());
            }
            ModelSource::Online {
                base_url,
                model: model_id,
            } => {
                let normalized = crate::online::validate_endpoint(base_url, model_id)?;
                if id_for(&format!("online\0{normalized}\0{}", model_id.trim())) != model.id {
                    return Err("online connection address changed; add it again to bind the API key to the new endpoint".into());
                }
            }
            _ => {}
        }
    }
    Ok(registry.models)
}

pub fn find(settings_path: &Path, id: &str) -> Result<Option<CustomModel>, String> {
    if !is_custom(id) {
        return Ok(None);
    }
    list(settings_path)?
        .into_iter()
        .find(|m| m.id == id)
        .map(Some)
        .ok_or_else(|| format!("custom model '{id}' is no longer registered; choose a model in Models"))
}

fn save(settings_path: &Path, models: Vec<CustomModel>) -> Result<(), String> {
    let path = registry_path(settings_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(&Registry { version: 1, models }).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

fn id_for(source: &str) -> String {
    format!("custom-{}", hex::encode(Sha256::digest(source.as_bytes())))
}

fn upsert(settings_path: &Path, entry: CustomModel, replacing: Option<&str>) -> Result<String, String> {
    let mut models = list(settings_path)?;
    if let Some(id) = replacing {
        let old = models
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or_else(|| "the custom model being edited is no longer registered".to_string())?;
        if !matches!(old.source, ModelSource::Local { .. }) {
            return Err("only a local model can be given a new folder".into());
        }
        *old = entry.clone();
    } else if let Some(old) = models.iter_mut().find(|m| m.id == entry.id) {
        *old = entry.clone();
    } else {
        models.push(entry.clone());
    }
    save(settings_path, models)?;
    Ok(entry.id)
}

fn model_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 120 {
        return Err("enter a model name of 1–120 characters".into());
    }
    Ok(name.to_string())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ValidatedLocal {
    pub directory: PathBuf,
    pub kind: ModelKind,
}

/// The validation result can only be constructed by a real load and inference in production.
pub fn register_local(
    settings_path: &Path,
    name: &str,
    validated: ValidatedLocal,
    replacing: Option<&str>,
) -> Result<String, String> {
    let name = model_name(name)?;
    let id = replacing.map(str::to_string).unwrap_or_else(|| {
        id_for(&format!(
            "local:{}:{}",
            validated.directory.display(),
            validated.kind
        ))
    });
    upsert(
        settings_path,
        CustomModel {
            id,
            name,
            source: ModelSource::Local {
                directory: validated.directory,
                kind: validated.kind,
            },
        },
        replacing,
    )
}

pub fn register_online(
    settings_path: &Path,
    name: &str,
    base_url: &str,
    model: &str,
    key: &str,
) -> Result<String, String> {
    let name = model_name(name)?;
    let base_url = crate::online::validate_endpoint(base_url, model)?;
    let model = model.trim().to_string();
    let key = key.trim();
    if key.contains(['\r', '\n']) {
        return Err("API key must not contain line breaks".into());
    }
    let id = id_for(&format!("online\0{base_url}\0{model}"));
    let store = lw_platform::SecureStore::new("OwlWhisp");
    let previous = store.get(&id).map_err(|e| e.to_string())?;
    if !key.is_empty() {
        store
            .set(&id, key)
            .map_err(|e| format!("could not store the API key: {e}"))?;
    }
    let entry = CustomModel {
        id: id.clone(),
        name,
        source: ModelSource::Online { base_url, model },
    };
    match upsert(settings_path, entry, None) {
        Ok(id) => Ok(id),
        Err(error) => {
            if !key.is_empty() {
                match previous {
                    Some(old) => {
                        let _ = store.set(&id, &old);
                    }
                    None => {
                        let _ = store.delete(&id);
                    }
                }
            }
            Err(error)
        }
    }
}

/// Unregister only. An external model directory is never passed to a deletion function.
pub fn remove(settings_path: &Path, id: &str) -> Result<(), String> {
    let settings = Settings::load(settings_path).map_err(|e| e.to_string())?;
    if settings.model_id == id {
        return Err("choose another model before removing the active model".into());
    }
    let mut models = list(settings_path)?;
    let position = models
        .iter()
        .position(|m| m.id == id)
        .ok_or_else(|| "custom model is no longer registered".to_string())?;
    if matches!(models[position].source, ModelSource::Online { .. }) {
        lw_platform::SecureStore::new("OwlWhisp")
            .delete(id)
            .map_err(|e| e.to_string())?;
    }
    models.remove(position);
    save(settings_path, models)
}

pub fn local_files(model: &CustomModel) -> Result<lw_engine_sherpa::ModelFiles, String> {
    let ModelSource::Local { directory, kind } = &model.source else {
        return Err("this is an online model".into());
    };
    let files = detect_in_dir(directory, true, Some(*kind)).map_err(|e| {
        format!(
            "{}: {e}. Use Choose folder to locate the model again.",
            model.name
        )
    })?;
    files.verify_exists().map_err(|e| e.to_string())?;
    Ok(files)
}

/// Resolve both sources in one place; IDs never become arbitrary file paths.
pub fn resolve_directory(settings_path: &Path, id: &str) -> Result<PathBuf, String> {
    if let Some(model) = find(settings_path, id)? {
        return match model.source {
            ModelSource::Local { ref directory, .. } => {
                local_files(&model)?;
                Ok(directory.clone())
            }
            ModelSource::Online { .. } => Ok(PathBuf::new()),
        };
    }
    let catalog = lw_core::model::Catalog::builtin().map_err(|e| e.to_string())?;
    if catalog.get(id).is_none() {
        return Err(format!("unknown model '{id}'; choose a model in Models"));
    }
    let dir = settings_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("models")
        .join(id);
    if !dir.is_dir() {
        return Err(format!("model '{id}' is not installed ({})", dir.display()));
    }
    Ok(dir)
}

pub fn build_engine(model: &CustomModel, settings: &Settings) -> Result<Box<dyn SpeechEngine>, String> {
    match &model.source {
        ModelSource::Online {
            base_url,
            model: model_id,
        } => {
            let key = lw_platform::SecureStore::new("OwlWhisp")
                .get(&model.id)
                .map_err(|e| e.to_string())?
                .unwrap_or_default();
            Ok(Box::new(crate::online::OnlineEngine::new(
                base_url, model_id, key,
            )?))
        }
        ModelSource::Local { directory, kind } => {
            if !matches!(
                settings.backend,
                BackendPreference::Automatic | BackendPreference::ForceCpu
            ) {
                return Err("this custom ONNX model uses CPU; select Automatic or CPU in Settings".into());
            }
            local_files(model)?;
            #[cfg(feature = "sherpa")]
            {
                Ok(Box::new(lw_engine_sherpa::SherpaEngine::new(
                    lw_engine_sherpa::SherpaConfig::new(directory).with_model_kind(*kind),
                )))
            }
            #[cfg(not(feature = "sherpa"))]
            {
                let _ = (directory, kind);
                Err("this build does not include the sherpa ONNX engine".into())
            }
        }
    }
}

pub struct ValidationHandle {
    receiver: Receiver<Result<ValidatedLocal, String>>,
}

impl ValidationHandle {
    pub fn poll(&self) -> Option<Result<ValidatedLocal, String>> {
        match self.receiver.try_recv() {
            Ok(answer) => Some(answer),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("model validation stopped unexpectedly".into())),
        }
    }
}

pub fn start_validation(directory: PathBuf, kind: Option<ModelKind>) -> ValidationHandle {
    let (sender, receiver) = bounded(1);
    std::thread::spawn(move || {
        let _ = sender.send(validate_isolated(&directory, kind));
    });
    ValidationHandle { receiver }
}

fn inspect(directory: &Path, kind: Option<ModelKind>) -> Result<ValidatedLocal, String> {
    let directory = directory
        .canonicalize()
        .map_err(|e| format!("{}: {e}", directory.display()))?;
    let files = detect_in_dir(&directory, true, kind).map_err(|e| e.to_string())?;
    files.verify_exists().map_err(|e| e.to_string())?;
    Ok(ValidatedLocal {
        directory,
        kind: files.kind(),
    })
}

fn validate_isolated(directory: &Path, kind: Option<ModelKind>) -> Result<ValidatedLocal, String> {
    let inspected = inspect(directory, kind)?;
    if !lw_engine_sherpa::is_available() {
        return Err("this build does not include the sherpa ONNX engine".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut command = Command::new(exe);
    command
        .arg("--validate-custom-model")
        .arg(&inspected.directory)
        .arg(inspected.kind.as_str())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("could not start model validation: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(match result {
                    Err(e) => format!("model validation failed: {e}"),
                    _ => "model validation timed out after 120 seconds".into(),
                });
            }
        }
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    serde_json::from_slice::<Result<ValidatedLocal, String>>(&output.stdout)
        .map_err(|_| "the model could not be loaded; the validation process stopped without a result. Check that it is a compatible sherpa-onnx export.".to_string())?
}

/// Entry point of the short-lived validation child. Never changes the active model or registry.
pub fn run_validation_worker(directory: &Path, kind: ModelKind) -> Result<ValidatedLocal, String> {
    let inspected = inspect(directory, Some(kind))?;
    #[cfg(feature = "sherpa")]
    {
        let mut engine = lw_engine_sherpa::SherpaEngine::new(
            lw_engine_sherpa::SherpaConfig::new(&inspected.directory).with_model_kind(kind),
        );
        engine
            .initialize(&EngineInitContext {
                model_dir: inspected.directory.clone(),
                ..Default::default()
            })
            .map_err(|e| e.to_string())?;
        let health = engine.health_check();
        if !health.ok {
            return Err(health.message);
        }
        Ok(inspected)
    }
    #[cfg(not(feature = "sherpa"))]
    {
        let _ = inspected;
        Err("this build does not include the sherpa ONNX engine".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf, ValidatedLocal) {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("external model");
        std::fs::create_dir(&dir).unwrap();
        for name in [
            "tiny.en-encoder.onnx",
            "tiny.en-decoder.onnx",
            "tiny.en-tokens.txt",
        ] {
            std::fs::write(dir.join(name), b"fixture").unwrap();
        }
        let validated = inspect(&dir, None).unwrap();
        let settings = temp.path().join("app/settings.json");
        (temp, settings, validated)
    }

    #[test]
    fn registration_resolves_external_files_and_removal_preserves_them() {
        let (_temp, settings, model) = fixture();
        let id = register_local(&settings, "My Whisper", model.clone(), None).unwrap();
        assert_eq!(resolve_directory(&settings, &id).unwrap(), model.directory);
        remove(&settings, &id).unwrap();
        assert!(model.directory.join("tiny.en-encoder.onnx").exists());
        assert!(list(&settings).unwrap().is_empty());
    }

    #[test]
    fn active_model_cannot_be_removed_and_relocation_keeps_its_id() {
        let (temp, settings, model) = fixture();
        let id = register_local(&settings, "Mine", model.clone(), None).unwrap();
        let s = Settings {
            model_id: id.clone(),
            ..Default::default()
        };
        s.save(&settings).unwrap();
        assert!(remove(&settings, &id).is_err());
        let moved = temp.path().join("moved");
        std::fs::rename(&model.directory, &moved).unwrap();
        assert!(resolve_directory(&settings, &id).is_err());
        let updated = inspect(&moved, Some(model.kind)).unwrap();
        assert_eq!(
            register_local(&settings, "Mine", updated.clone(), Some(&id)).unwrap(),
            id
        );
        assert_eq!(resolve_directory(&settings, &id).unwrap(), updated.directory);
        assert_eq!(list(&settings).unwrap().len(), 1);
    }

    #[test]
    fn rejects_unknown_ids_corrupt_registry_and_ambiguous_layout() {
        let (temp, settings, _) = fixture();
        assert!(resolve_directory(&settings, "../../other").is_err());
        assert!(find(&settings, "custom-missing").is_err());
        std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
        std::fs::write(registry_path(&settings), "broken json").unwrap();
        assert!(list(&settings).is_err());
        let dir = temp.path().join("ambiguous");
        std::fs::create_dir(&dir).unwrap();
        for name in ["model.onnx", "tokens.txt"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        assert!(inspect(&dir, None).is_err());
        assert_eq!(
            inspect(&dir, Some(ModelKind::SenseVoice)).unwrap().kind,
            ModelKind::SenseVoice
        );
    }

    #[test]
    fn online_selection_is_explicit_and_benchmarks_do_not_upload_fixtures() {
        let (_temp, settings, _) = fixture();
        let base = "https://127.0.0.1:9/v1";
        let id = id_for(&format!("online\0{base}\0whisper-1"));
        let model = CustomModel {
            id: id.clone(),
            name: "Online test".into(),
            source: ModelSource::Online {
                base_url: base.into(),
                model: "whisper-1".into(),
            },
        };
        save(&settings, vec![model]).unwrap();
        assert_ne!(Settings::load(&settings).unwrap().model_id, id);
        crate::install::select(&settings, &id).unwrap();
        assert_eq!(Settings::load(&settings).unwrap().model_id, id);
        assert!(
            crate::bench::run_benchmark_job(&settings, None, None, None)
                .unwrap_err()
                .contains("excluded")
        );
        assert!(
            crate::bench::run_benchmark_suite(&settings, None, |_| panic!("must not start a benchmark"))
                .unwrap_err()
                .contains("excluded")
        );
        let mut models = list(&settings).unwrap();
        if let ModelSource::Online { base_url, .. } = &mut models[0].source {
            *base_url = "https://different.example/v1".into();
        }
        save(&settings, models).unwrap();
        assert!(list(&settings).unwrap_err().contains("address changed"));
    }
}
