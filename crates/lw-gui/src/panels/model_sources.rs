//! Local model import and opt-in online speech connections, within the Models tab.

use std::path::PathBuf;

use iced::widget::{button, column, pick_list, row, text, text_input};
use iced::{Element, Length, Padding};
use lw_app::custom_models::{self, CustomModel, LocalModelKind, ModelSource};

use crate::{theme, widgets};

#[derive(Clone)]
pub enum Message {
    ShowLocal,
    ShowOnline,
    CancelLocal,
    CancelOnline,
    LocalNameChanged(String),
    OnlineNameChanged(String),
    DirectoryChanged(String),
    KindSelected(KindChoice),
    BrowseFolder,
    FolderPicked(Option<PathBuf>),
    CheckLocal,
    Tick,
    Relocate(String),
    Remove(String),
    BaseUrlChanged(String),
    ModelChanged(String),
    KeyChanged(String),
    SaveOnline,
    TestOnline,
    EditOnline(String),
}

// GUI messages can be traced by the toolkit. Never format an API key as part of an event.
impl std::fmt::Debug for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ModelSources({:?})", std::mem::discriminant(self))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KindChoice(Option<LocalModelKind>);

impl std::fmt::Display for KindChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(kind) => write!(f, "{kind}"),
            None => f.write_str("Detect automatically"),
        }
    }
}

struct Row {
    model: CustomModel,
    error: Option<String>,
}

enum JobResult {
    Saved(String),
    Tested(String),
}

pub struct State {
    entries: Vec<Row>,
    registry_error: Option<String>,
    local_open: bool,
    online_open: bool,
    local_name: String,
    online_name: String,
    directory: String,
    kind: KindChoice,
    kinds: Vec<KindChoice>,
    replacing: Option<String>,
    picking: bool,
    validation: Option<custom_models::ValidationHandle>,
    online_job: Option<std::sync::mpsc::Receiver<Result<JobResult, String>>>,
    base_url: String,
    model: String,
    key: String,
    notice: Option<String>,
    error: Option<String>,
}

impl State {
    pub fn new() -> Self {
        let mut state = Self {
            entries: Vec::new(),
            registry_error: None,
            local_open: false,
            online_open: false,
            local_name: String::new(),
            online_name: String::new(),
            directory: String::new(),
            kind: KindChoice(None),
            kinds: std::iter::once(KindChoice(None))
                .chain(LocalModelKind::ALL.iter().map(|k| KindChoice(Some(*k))))
                .collect(),
            replacing: None,
            picking: false,
            validation: None,
            online_job: None,
            base_url: "https://api.openai.com/v1".into(),
            model: "whisper-1".into(),
            key: String::new(),
            notice: None,
            error: None,
        };
        state.refresh();
        state
    }

    pub fn refresh(&mut self) {
        match custom_models::list(&lw_app::paths::settings_path()) {
            Ok(models) => {
                self.registry_error = None;
                self.entries = models
                    .into_iter()
                    .map(|model| {
                        let error = match &model.source {
                            ModelSource::Local { .. } => {
                                custom_models::local_files(&model).err().or_else(|| {
                                    (!lw_app::catalog::engine_features().contains(&"sherpa"))
                                        .then(|| "this build does not include the sherpa ONNX engine".into())
                                })
                            }
                            ModelSource::Online { .. } => None,
                        };
                        Row { model, error }
                    })
                    .collect();
            }
            Err(error) => {
                self.registry_error = Some(error);
                self.entries.clear();
            }
        }
    }

    pub fn busy(&self) -> bool {
        self.picking || self.validation.is_some() || self.online_job.is_some()
    }

    pub fn subscription(&self) -> iced::Subscription<Message> {
        if self.validation.is_some() || self.online_job.is_some() {
            iced::time::every(std::time::Duration::from_millis(200)).map(|_| Message::Tick)
        } else {
            iced::Subscription::none()
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn all_expanded(&self) -> bool {
        self.local_open && self.online_open
    }

    pub fn set_expanded(&mut self, expanded: bool) {
        if self.busy() {
            return;
        }
        if !expanded {
            self.clear_local_form();
            self.clear_online_form();
        }
        self.local_open = expanded;
        self.online_open = expanded;
    }

    fn clear_local_form(&mut self) {
        self.local_name.clear();
        self.directory.clear();
        self.kind = KindChoice(None);
        self.replacing = None;
        self.notice = None;
        self.error = None;
    }

    fn clear_online_form(&mut self) {
        self.online_name.clear();
        self.key.clear();
        self.notice = None;
        self.error = None;
    }

    /// True when the selected model's registration changed and dictation needs to reload it.
    pub fn update(&mut self, message: Message) -> bool {
        let settings_path = lw_app::paths::settings_path();
        if self.busy() && !matches!(message, Message::Tick | Message::FolderPicked(_)) {
            return false;
        }
        match message {
            Message::ShowLocal => {
                if self.local_open {
                    self.clear_local_form();
                }
                self.local_open = !self.local_open;
            }
            Message::ShowOnline => {
                if self.online_open {
                    self.clear_online_form();
                }
                self.online_open = !self.online_open;
            }
            Message::CancelLocal => {
                self.clear_local_form();
                self.local_open = false;
            }
            Message::CancelOnline => {
                self.clear_online_form();
                self.online_open = false;
            }
            Message::LocalNameChanged(value) => self.local_name = value,
            Message::OnlineNameChanged(value) => self.online_name = value,
            Message::DirectoryChanged(value) => self.directory = value,
            Message::KindSelected(value) => self.kind = value,
            Message::BrowseFolder => self.picking = true,
            Message::FolderPicked(path) => {
                self.picking = false;
                if let Some(path) = path {
                    if self.local_name.trim().is_empty() {
                        self.local_name = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();
                    }
                    self.directory = path.display().to_string();
                }
            }
            Message::CheckLocal => {
                self.notice = None;
                self.error = None;
                self.validation = Some(custom_models::start_validation(
                    PathBuf::from(self.directory.trim()),
                    self.kind.0,
                ));
            }
            Message::Tick => {
                if let Some(result) = self.validation.as_ref().and_then(|h| h.poll()) {
                    self.validation = None;
                    match result.and_then(|validated| {
                        custom_models::register_local(
                            &settings_path,
                            &self.local_name,
                            validated,
                            self.replacing.as_deref(),
                        )
                    }) {
                        Ok(id) => {
                            self.clear_local_form();
                            self.local_open = false;
                            self.notice = Some(
                                "Model checked and added. Its files stay in the selected folder.".into(),
                            );
                            self.refresh();
                            return lw_core::settings::Settings::load(&settings_path)
                                .is_ok_and(|s| s.model_id == id);
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                let result = self.online_job.as_ref().and_then(|rx| match rx.try_recv() {
                    Ok(result) => Some(result),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(_) => Some(Err("the connection task stopped unexpectedly".into())),
                });
                if let Some(result) = result {
                    self.online_job = None;
                    match result {
                        Ok(JobResult::Saved(id)) => {
                            self.clear_online_form();
                            self.online_open = false;
                            self.notice = Some("Online connection saved. Choose Use online to send dictation to this service.".into());
                            self.refresh();
                            return lw_core::settings::Settings::load(&settings_path)
                                .is_ok_and(|s| s.model_id == id);
                        }
                        Ok(JobResult::Tested(note)) => self.notice = Some(note),
                        Err(error) => self.error = Some(error),
                    }
                }
            }
            Message::Relocate(id) => {
                let model = self
                    .entries
                    .iter()
                    .find(|r| r.model.id == id)
                    .map(|r| r.model.clone());
                if let Some(CustomModel {
                    name,
                    source: ModelSource::Local { directory, kind },
                    ..
                }) = model
                {
                    self.clear_local_form();
                    self.local_name = name;
                    self.directory = directory.display().to_string();
                    self.kind = KindChoice(Some(kind));
                    self.replacing = Some(id);
                    self.local_open = true;
                }
            }
            Message::Remove(id) => match custom_models::remove(&settings_path, &id) {
                Ok(()) => {
                    self.notice = Some("Removed from the list. Local model files are kept.".into());
                    self.error = None;
                    self.refresh();
                }
                Err(error) => self.error = Some(error),
            },
            Message::BaseUrlChanged(value) => self.base_url = value,
            Message::ModelChanged(value) => self.model = value,
            Message::KeyChanged(value) => self.key = value,
            Message::SaveOnline | Message::TestOnline => {
                let testing = matches!(message, Message::TestOnline);
                let name = self.online_name.clone();
                let base_url = self.base_url.clone();
                let model = self.model.clone();
                let mut key = self.key.clone();
                let existing = self
                    .entries
                    .iter()
                    .find(|r| match &r.model.source {
                        ModelSource::Online {
                            base_url: base,
                            model: m,
                        } => {
                            base.trim_end_matches('/') == base_url.trim().trim_end_matches('/')
                                && m == model.trim()
                        }
                        _ => false,
                    })
                    .map(|r| r.model.id.clone());
                self.notice = None;
                self.error = None;
                let (tx, rx) = std::sync::mpsc::channel();
                self.online_job = Some(rx);
                std::thread::spawn(move || {
                    let result = if testing {
                        (|| {
                            if key.is_empty()
                                && let Some(id) = existing
                            {
                                key = lw_platform::SecureStore::new("OwlWhisp")
                                    .get(&id)
                                    .map_err(|e| e.to_string())?
                                    .unwrap_or_default();
                            }
                            lw_app::online::test_connection(&base_url, &model, key).map(JobResult::Tested)
                        })()
                    } else {
                        custom_models::register_online(&settings_path, &name, &base_url, &model, &key)
                            .map(JobResult::Saved)
                    };
                    let _ = tx.send(result);
                });
            }
            Message::EditOnline(id) => {
                let model = self
                    .entries
                    .iter()
                    .find(|r| r.model.id == id)
                    .map(|r| r.model.clone());
                if let Some(CustomModel {
                    name,
                    source: ModelSource::Online { base_url, model },
                    ..
                }) = model
                {
                    self.clear_online_form();
                    self.online_name = name;
                    self.base_url = base_url;
                    self.model = model;
                    self.online_open = true;
                }
            }
        }
        false
    }

    pub fn view<'a>(&'a self, selected: &'a str) -> Element<'a, super::models::Message> {
        let enabled = !self.busy() && self.registry_error.is_none();
        let mut content = column![
            row![
                text("CUSTOM MODELS").size(11).color(theme::TEXT),
                widgets::sub(format!(
                    "{} model{}",
                    self.entries.len(),
                    if self.entries.len() == 1 { "" } else { "s" }
                )),
            ]
            .spacing(8),
        ]
        .spacing(0);
        if let Some(error) = &self.registry_error {
            content = content.push(widgets::body(error).color(theme::BAD));
        }
        content = content.push(disclosure(
            "Add local model",
            self.local_open,
            enabled.then_some(Message::ShowLocal),
        ));
        if self.local_open {
            let editing = !self.busy();
            content = content.push(widgets::inset(column![
                widgets::body(if self.replacing.is_some() { "Locate local model" } else { "Add a compatible ONNX model folder" }),
                text_input("Model name", &self.local_name).on_input_maybe(editing.then_some(Message::LocalNameChanged)),
                row![
                    text_input("Folder containing the model files", &self.directory).on_input_maybe(editing.then_some(Message::DirectoryChanged)).width(Length::Fill),
                    button("Choose folder").on_press_maybe(editing.then_some(Message::BrowseFolder)).style(theme::action(false)),
                ].spacing(8),
                pick_list(self.kinds.as_slice(), Some(self.kind.clone()), Message::KindSelected),
                widgets::prose("Whisper, Moonshine, NeMo transducer, SenseVoice, Paraformer, Qwen3-ASR and Omnilingual sherpa-onnx exports. If detection is ambiguous, select the family. Custom ONNX models run on CPU."),
                row![
                    button(if self.validation.is_some() { "Checking model…" } else { "Check and add" })
                        .on_press_maybe((editing && !self.local_name.trim().is_empty() && !self.directory.trim().is_empty()).then_some(Message::CheckLocal)).style(theme::action(false)),
                    button("Cancel").on_press_maybe(editing.then_some(Message::CancelLocal)).style(theme::action(false)),
                ].spacing(8),
                widgets::sub("Checks loading and a short inference. Files stay in your folder; the active model changes only when you choose Use."),
            ].spacing(8)));
        }
        content = content.push(iced::widget::rule::horizontal(1).style(theme::rule));
        content = content.push(disclosure(
            "Connect online model",
            self.online_open,
            enabled.then_some(Message::ShowOnline),
        ));
        if self.online_open {
            let editing = !self.busy();
            content = content.push(widgets::inset(column![
                widgets::body("Online speech recognition (compatible transcription API)"),
                text_input("Connection name", &self.online_name).on_input_maybe(editing.then_some(Message::OnlineNameChanged)),
                text_input("API base URL, for example https://api.openai.com/v1", &self.base_url).on_input_maybe(editing.then_some(Message::BaseUrlChanged)),
                text_input("Transcription model ID, for example whisper-1", &self.model).on_input_maybe(editing.then_some(Message::ModelChanged)),
                text_input("API key — blank keeps a saved key, or uses no authentication", &self.key).secure(true).on_input_maybe(editing.then_some(Message::KeyChanged)),
                widgets::prose("When you select this model, recorded speech is sent to the API address above. The provider may charge for API usage. A chat subscription does not necessarily include API access. Keys are stored in your operating system's credential store."),
                widgets::sub("Test connection uploads one second of generated silence and may use API quota. Saving does not contact the provider. Changing the address or model creates a separate connection; saved keys stay with their original endpoint."),
                row![
                    button("Test connection").on_press_maybe(editing.then_some(Message::TestOnline)).style(theme::action(false)),
                    button(if self.online_job.is_some() { "Working…" } else { "Save connection" }).on_press_maybe((editing && !self.online_name.trim().is_empty()).then_some(Message::SaveOnline)).style(theme::action(false)),
                    button("Cancel").on_press_maybe(editing.then_some(Message::CancelOnline)).style(theme::action(false)),
                ].spacing(8),
            ].spacing(8)));
        }
        content = content.push(iced::widget::rule::horizontal(1).style(theme::rule));
        if let Some(note) = &self.notice {
            content = content.push(widgets::body(note).color(theme::GOOD));
        }
        if let Some(error) = &self.error {
            content = content.push(widgets::body(error).color(theme::BAD));
        }
        // Rows map their actions to the enclosing Models message, including the common selection path.
        let mut rows = column![].spacing(12);
        for entry in &self.entries {
            let model = &entry.model;
            let active = selected == model.id;
            let (detail, action, change) = match &model.source {
                ModelSource::Local { directory, kind } => (
                    format!("{kind} · CPU · {}", directory.display()),
                    "Use on CPU",
                    Message::Relocate(model.id.clone()),
                ),
                ModelSource::Online {
                    base_url,
                    model: model_id,
                } => (
                    format!(
                        "Online · {model_id} · {base_url}\nRecorded speech is sent to this service when selected."
                    ),
                    "Use online",
                    Message::EditOnline(model.id.clone()),
                ),
            };
            let change_label = if matches!(model.source, ModelSource::Local { .. }) {
                "Choose folder"
            } else {
                "Edit connection"
            };
            let mut item = column![widgets::body(&model.name), widgets::sub(detail)].spacing(6);
            if let Some(error) = &entry.error {
                item = item.push(widgets::body(error).color(theme::BAD));
            }
            let item: Element<'_, Message> = item.into();
            rows =
                rows.push(
                    column![
                        item.map(super::models::Message::Sources),
                        row![
                            button(if active { "Selected" } else { action })
                                .on_press_maybe(
                                    (enabled && !active && entry.error.is_none())
                                        .then(|| super::models::Message::Select(model.id.clone()))
                                )
                                .style(theme::action(false)),
                            button(change_label)
                                .on_press_maybe(enabled.then_some(super::models::Message::Sources(change)))
                                .style(theme::action(false)),
                            button("Remove from list")
                                .on_press_maybe((enabled && !active).then(
                                    || super::models::Message::Sources(Message::Remove(model.id.clone()))
                                ))
                                .style(theme::action(true)),
                        ]
                        .spacing(8),
                    ]
                    .spacing(6),
                );
        }
        let form: Element<'_, Message> = content.into();
        column![form.map(super::models::Message::Sources), rows]
            .spacing(12)
            .into()
    }
}

fn disclosure(label: &str, open: bool, message: Option<Message>) -> Element<'_, Message> {
    button(
        row![
            text(if open { "v" } else { ">" }).size(11).color(theme::TEXT_DIM),
            text(label).size(14).color(theme::TEXT),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center),
    )
    .padding(Padding::from([8, 4]))
    .width(Length::Fill)
    .on_press_maybe(message)
    .style(theme::disclosure)
    .into()
}

#[cfg(test)]
mod tests {
    use super::Message;

    #[test]
    fn debug_events_do_not_reveal_api_keys() {
        let event = Message::KeyChanged("private-api-key".into());
        assert!(!format!("{event:?}").contains("private-api-key"));
    }
}
