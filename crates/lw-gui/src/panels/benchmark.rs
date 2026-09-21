//! The Benchmark tab.
//!
//! A port of `app/frontend/src/panels/Benchmark.tsx`. Its job is to turn the model currently
//! selected into real numbers for *this* machine, and then to say exactly what those numbers do
//! and do not cover -- which is most of the panel's text, and deliberately so.
//!
//! The sweep takes tens of seconds and loads an ONNX Runtime session per accelerator, so it runs
//! on a blocking task and the window stays live throughout. Progress lines are collected in a
//! shared buffer the worker writes and the view reads; nothing else is shared.

use std::sync::{Arc, Mutex};

use iced::widget::{button, column, container, pick_list, row, Space};
use iced::{Element, Padding, Task};
use lw_app::bench::{BenchReport, BenchSuite};
use lw_core::engine::BackendPreference;

use crate::{theme, widgets};

/// A pickable wrapper, so a list can show a label while carrying the value.
macro_rules! pickable {
    ($name:ident, $inner:ty) => {
        #[derive(Clone, PartialEq, Eq)]
        pub struct $name {
            value: $inner,
            label: String,
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.label)
            }
        }
    };
}

pickable!(ModelChoice, Option<String>);
pickable!(BackendChoice, Option<BackendPreference>);

/// What the worker thread reports while a sweep is running.
#[derive(Default)]
struct Progress {
    lines: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// Measure one backend -- the chosen one, or the one in Settings.
    RunOne,
    /// Measure every accelerator this machine can use, over one shared clip set.
    RunAll,
    Tick,
    ModelSelected(Option<String>),
    BackendSelected(Option<BackendPreference>),
    MethodToggled,
    FinishedOne(Arc<Result<BenchReport, String>>),
    Finished(Arc<Result<BenchSuite, String>>),
}

pub struct State {
    running: bool,
    progress: Arc<Mutex<Progress>>,
    result: Option<Result<BenchSuite, String>>,
    /// The single-backend run, kept apart from the sweep: they answer different questions and
    /// showing one where the other was expected is how a reader ends up quoting the wrong number.
    single: Option<Result<BenchReport, String>>,
    /// What to measure. `None` in either means "whatever Settings says".
    models: Vec<ModelChoice>,
    backends: Vec<BackendChoice>,
    model_choice: Option<String>,
    backend_choice: Option<BackendPreference>,
    /// When the run in flight started, so the button can count.
    started: Option<std::time::Instant>,
    /// Whether the "how this is measured" block is open.
    method: bool,
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

impl State {
    pub fn new() -> Self {
        let diag = lw_app::diagnostics::collect(env!("CARGO_PKG_VERSION"));
        let usable: std::collections::BTreeMap<String, bool> = diag
            .accelerators
            .iter()
            .map(|a| (a.id.to_string(), a.usable))
            .collect();

        // Only models that are installed and that this build can run. Offering one that is
        // neither is offering a run that fails after the user has waited for it.
        let mut models = vec![ModelChoice {
            value: None,
            label: "From Settings".into(),
        }];
        if let Ok(view) = lw_app::catalog::build(&lw_app::paths::models_root()) {
            for e in &view.entries {
                if e.runnable && e.install_state == lw_core::model::InstallState::Installed {
                    models.push(ModelChoice {
                        value: Some(e.id.clone()),
                        label: e.name.clone(),
                    });
                }
            }
        }

        let mut backends = vec![BackendChoice {
            value: None,
            label: "From Settings".into(),
        }];
        for p in BackendPreference::all() {
            // The reason travels with the name, so the list says *why* a choice is pointless
            // rather than only listing it. A coarse choice ("any NPU") stays offered even when
            // nothing matches: measuring the failure of one is a legitimate thing to do.
            let note = match p.accelerator() {
                Some(a) if !usable.get(a.id()).copied().unwrap_or(false) => {
                    " - not usable on this machine"
                }
                _ => "",
            };
            backends.push(BackendChoice {
                value: Some(p),
                label: format!("{}{note}", p.label()),
            });
        }

        Self {
            running: false,
            progress: Arc::new(Mutex::new(Progress::default())),
            result: None,
            single: None,
            models,
            backends,
            model_choice: None,
            backend_choice: None,
            started: None,
            method: false,
        }
    }

    /// Seconds since the run in flight began, for the button to count.
    fn elapsed(&self) -> u64 {
        self.started.map(|t| t.elapsed().as_secs()).unwrap_or(0)
    }

    /// Redraw while a sweep is in flight, so the progress lines appear as they are written.
    pub fn subscription(&self) -> iced::Subscription<Message> {
        if self.running {
            iced::time::every(std::time::Duration::from_millis(250)).map(|_| Message::Tick)
        } else {
            iced::Subscription::none()
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ModelSelected(v) => self.model_choice = v,
            Message::BackendSelected(v) => self.backend_choice = v,
            Message::MethodToggled => self.method = !self.method,

            Message::RunOne => {
                if self.running {
                    return Task::none();
                }
                self.running = true;
                self.started = Some(std::time::Instant::now());
                self.single = None;
                self.result = None;
                let settings_path = lw_app::paths::settings_path();
                let model = self.model_choice.clone();
                let backend = self.backend_choice;

                // Same reasoning as the sweep: this loads an ONNX Runtime session and transcribes
                // a dozen clips, so it must not run on the thread that draws.
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            lw_app::bench::run_benchmark_job(&settings_path, model, backend, None)
                        })
                        .await
                        .unwrap_or_else(|e| Err(format!("the benchmark task failed: {e}")))
                    },
                    |r| Message::FinishedOne(Arc::new(r)),
                );
            }
            Message::FinishedOne(r) => {
                self.running = false;
                self.started = None;
                self.single = Arc::try_unwrap(r).ok();
                return Task::none();
            }

            Message::RunAll => {
                if self.running {
                    return Task::none();
                }
                self.running = true;
                self.started = Some(std::time::Instant::now());
                self.result = None;
                self.single = None;
                let progress = Arc::clone(&self.progress);
                progress.lock().map(|mut p| p.lines.clear()).ok();
                let settings_path = lw_app::paths::settings_path();

                // `spawn_blocking`, not an ordinary future: this loads an ONNX Runtime session per
                // accelerator and transcribes fifteen clips through each. On the executor thread
                // it would stop the window redrawing for the whole sweep.
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            lw_app::bench::run_benchmark_suite(&settings_path, None, |value| {
                                if let Ok(mut p) = progress.lock() {
                                    p.lines.push(describe_progress(&value));
                                }
                            })
                        })
                        .await
                        .unwrap_or_else(|e| Err(format!("the benchmark task failed: {e}")))
                    },
                    |r| Message::Finished(Arc::new(r)),
                );
            }
            Message::Tick => {}
            Message::Finished(r) => {
                self.running = false;
                self.started = None;
                // The Arc exists only because iced messages must be Clone; nothing else holds
                // a reference by the time it arrives, so unwrapping it is the normal path.
                self.result = Arc::try_unwrap(r).ok();
            }
        }
        Task::none()
    }

    /// The model and backend to measure. Both default to what Settings says.
    fn controls(&self) -> Element<'_, Message> {
        let model = self
            .models
            .iter()
            .find(|m| m.value == self.model_choice)
            .cloned();
        let backend = self
            .backends
            .iter()
            .find(|b| b.value == self.backend_choice)
            .cloned();

        let installed = self.models.len() - 1;

        column![
            column![
                widgets::field_label("Model"),
                pick_list(self.models.clone(), model, |m: ModelChoice| {
                    Message::ModelSelected(m.value)
                })
                .text_size(14)
                .width(320),
                widgets::sub(if installed == 0 {
                    "No installed, runnable model found - a run falls back to the one in Settings."
                        .to_string()
                } else {
                    format!(
                        "{installed} installed model{} this build can run. Both actions below \
                         measure the one chosen here.",
                        if installed == 1 { "" } else { "s" }
                    )
                }),
            ]
            .spacing(4),
            column![
                widgets::field_label("Backend"),
                pick_list(self.backends.clone(), backend, |b: BackendChoice| {
                    Message::BackendSelected(b.value)
                })
                .text_size(14)
                .width(320),
                widgets::prose(
                    "Forcing a backend is how you confirm or refute an estimate: run each one and \
                     compare the measured RTF. A choice this machine cannot honour is marked as \
                     such - a strict choice fails rather than quietly producing a slower number \
                     from somewhere else.",
                ),
                widgets::sub(
                    "This applies to \u{201c}Run benchmark\u{201d} only. \u{201c}Compare all \
                     accelerators\u{201d} measures every usable one, so it ignores this choice.",
                ),
            ]
            .spacing(4),
        ]
        .spacing(10)
        .into()
    }

    /// The two buttons, and what each of them costs in time.
    fn actions(&self) -> Element<'_, Message> {
        let secs = self.elapsed();
        column![
            row![
                button(widgets::body(if self.running && self.single.is_none() && self.result.is_none() {
                    format!("Running... {secs}s")
                } else {
                    "Run benchmark".to_string()
                }))
                .padding(Padding::from([8, 18]))
                .on_press_maybe((!self.running).then_some(Message::RunOne)),
                button(widgets::body("Compare all accelerators"))
                    .padding(Padding::from([8, 18]))
                    .on_press_maybe((!self.running).then_some(Message::RunAll)),
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center),
            widgets::prose(
                "Run benchmark measures one backend and takes tens of seconds. Compare all \
                 accelerators measures CPU, GPU and NPU on one shared clip set in a single action \
                 and takes several minutes: each loads its own engine, and a first NPU run also \
                 prepares and caches a context binary before it can time anything. Both drop the \
                 dictation engine; the next dictation loads it again.",
            ),
        ]
        .spacing(6)
        .into()
    }

    /// What a run actually does, collapsed by default.
    ///
    /// Reference material rather than something to read before every run -- but without it,
    /// "RTF 0.04, WER 7.9%" is a pair of numbers with no method behind them, and the reader has no
    /// way to tell that the cold figure is deliberately the worst one, or that some clips were
    /// left out of the accuracy total on purpose.
    fn methodology(&self) -> Element<'_, Message> {
        let head = button(
            row![
                widgets::sub(if self.method { "v" } else { ">" }),
                widgets::body("How this is measured"),
            ]
            .spacing(8),
        )
        .padding(Padding::from([4, 8]))
        .on_press(Message::MethodToggled)
        .style(|_t, _s| iced::widget::button::Style {
            background: None,
            text_color: theme::TEXT,
            ..Default::default()
        });

        if !self.method {
            return head.into();
        }

        let body = column![
            widgets::prose(
                "A run transcribes the fixture clips committed with this repository - twelve \
                 clips, three each in English, Russian, Spanish and Ukrainian - and times every \
                 one. They are real speech with reference transcripts, not a synthesised tone.",
            ),
            method_item(
                "RTF",
                "Wall-clock seconds per second of audio; lower is faster, and 1.0 means \
                 transcribing takes as long as the recording did. The first clip is reported \
                 separately as cold, because it carries the one-time warm-up. Warm is the mean of \
                 the rest, and warm is what steady-state dictation feels like.",
            ),
            method_item(
                "WER",
                "The share of words that came out wrong - substituted, dropped or invented - \
                 against the reference. Lower is better: 5% is about one word in twenty. \
                 Levenshtein distance over words, lowercased and with punctuation stripped, so \
                 casing and commas never count as errors. Word-weighted across the clips, so long \
                 clips carry more of the total; not a mean of per-clip rates.",
            ),
            method_item(
                "Which clips count",
                "Accuracy is scored only on the languages a model claims. An English-only model \
                 is judged on the English clips; the rest are not run at all, because a rate \
                 against a language a model never advertised measures the question rather than \
                 the model. Moonshine tiny en scores 0.092 on English and 0.850 if you score it \
                 on all four.",
            ),
            method_item(
                "Comparing backends",
                "Compare all accelerators runs every usable one over a single clip set, loaded \
                 once before the sweep starts. That shared set is what makes the rows comparable \
                 rather than three unrelated benchmarks.",
            ),
        ]
        .spacing(8);

        column![head, widgets::inset(body)].spacing(6).into()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let intro: Element<'_, Message> = widgets::card(
            column![
                widgets::heading("Measure this machine"),
                widgets::prose(
                    "Every number on this page is measured here, by the run you start. The catalog \
                     figures in Models are estimates; these are not.",
                ),
                self.methodology(),
                self.controls(),
                self.actions(),
            ]
            .spacing(10),
        )
        .into();

        let mut body = column![intro]
            .spacing(12)
            .padding(Padding::from([0, 8]));

        if self.running {
            let lines = self
                .progress
                .lock()
                .map(|p| p.lines.clone())
                .unwrap_or_default();
            let mut list = column![widgets::heading("In progress")].spacing(4);
            for l in lines {
                list = list.push(widgets::sub(l));
            }
            body = body.push(widgets::card(list));
        }

        if let Some(single) = &self.single {
            match single {
                Err(e) => {
                    body = body.push(widgets::card(
                        column![
                            widgets::heading("The benchmark could not run"),
                            widgets::prose(e.clone()),
                        ]
                        .spacing(6),
                    ));
                }
                Ok(report) => {
                    body = body.push(widgets::card(
                        column![
                            widgets::heading("One backend, measured"),
                            fact("Machine", report.machine.clone()),
                            fact("Model", report.model_id.clone()),
                            fact("Clips", report.clip_source.clone()),
                            widgets::prose(
                                "The backend named here is the one that actually executed, read \
                                 back from the engine rather than from what was asked for.",
                            ),
                        ]
                        .spacing(4),
                    ));
                    body = body.push(run_card(report, None));
                }
            }
        }

        match &self.result {
            None => {}
            Some(Err(e)) => {
                body = body.push(widgets::card(
                    column![
                        widgets::heading("The benchmark could not run"),
                        widgets::prose(e.clone()),
                    ]
                    .spacing(6),
                ));
            }
            Some(Ok(suite)) => {
                body = body.push(suite_card(suite));
                for run in &suite.runs {
                    body = body.push(run_card(run, Some(suite)));
                }
                if !suite.skipped.is_empty() {
                    let mut list = column![
                        widgets::heading("Not measured"),
                        widgets::prose(
                            "Skipped before loading anything, so nothing below was slowed down by \
                             an accelerator that was never going to work.",
                        ),
                    ]
                    .spacing(6);
                    for s in &suite.skipped {
                        list = list.push(
                            row![
                                widgets::badge_no("skipped"),
                                widgets::body(s.label.clone()),
                                widgets::sub(s.reason.clone()),
                            ]
                            .spacing(8)
                            .align_y(iced::Alignment::Center),
                        );
                    }
                    body = body.push(widgets::card(list));
                }
            }
        }

        body.push(Space::new(0, 8)).into()
    }
}

fn suite_card(suite: &BenchSuite) -> Element<'_, Message> {
    let mut c = column![
        widgets::heading("Every backend, side by side"),
        fact("Machine", suite.machine.clone()),
        fact("Model", suite.model_id.clone()),
        fact("Clips", suite.clip_source.clone()),
        widgets::prose(
            "Every row below ran those same clips, loaded once before the sweep started. That is \
             what makes these numbers a comparison rather than three unrelated benchmarks.",
        ),
    ]
    .spacing(4);

    if suite.runs.is_empty() {
        c = c.push(
            iced::widget::text(
                "No accelerator could be measured. Anything that was offered is listed below with \
                 the reason it did not run.",
            )
            .size(13)
            .color(theme::BAD),
        );
        return widgets::card(c).into();
    }

    let mut table = column![row![
        cell(widgets::field_label("backend"), 240),
        cell(widgets::field_label("cold RTF"), 110),
        cell(widgets::field_label("warm RTF"), 110),
        cell(widgets::field_label("error"), 110),
        widgets::field_label("marks"),
    ]
    .spacing(8)]
    .spacing(2);

    for run in &suite.runs {
        let id = run.accelerator.as_deref();
        let fastest = id.is_some() && id == suite.fastest.as_deref();
        let accurate = id.is_some() && id == suite.most_accurate.as_deref();

        let mut marks = row![].spacing(6);
        if fastest {
            marks = marks.push(widgets::badge_yes("fastest"));
        }
        if accurate {
            marks = marks.push(widgets::badge_yes("most accurate"));
        }
        if !fastest && !accurate {
            marks = marks.push(widgets::sub("-"));
        }

        table = table.push(
            row![
                cell(widgets::body(run.backend.clone()), 240),
                cell(widgets::mono(format!("{:.4}", run.cold_rtf)), 110),
                cell(
                    match run.warm_rtf {
                        Some(r) => widgets::mono(format!("{r:.4}")),
                        None => widgets::sub("no warm run"),
                    },
                    110,
                ),
                cell(
                    match run.wer {
                        Some(w) => widgets::mono(format!(
                            "{} {:.1}%",
                            run.unit.unwrap_or("WER"),
                            w * 100.0
                        )),
                        None => widgets::sub("no reference"),
                    },
                    110,
                ),
                marks,
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        );
    }

    widgets::card(c.push(widgets::inset(table))).into()
}

/// A fixed-width cell, so the columns line up without a table widget.
fn cell<'a>(content: impl Into<Element<'a, Message>>, width: u16) -> Element<'a, Message> {
    container(content).width(width).into()
}

/// Is this note the engine saying it did not get what was asked for?
///
/// The distinction earns its own badge because it is the one note a reader must not skim past: a
/// run labelled "NPU" that quietly fell back to the CPU is the exact failure this project refuses
/// to ship, and the note is where it shows.
fn is_fallback(note: &str) -> bool {
    let text = note.to_lowercase();
    text.contains("unavailable") || text.contains("using cpu")
}

fn run_card<'a>(run: &'a BenchReport, _suite: Option<&'a BenchSuite>) -> Element<'a, Message> {
    let warm = run
        .warm_rtf
        .map(|r| format!("{r:.4}"))
        .unwrap_or_else(|| "no warm run".into());

    // RTF is two different quantities and the difference decides which number a reader should
    // quote, so each carries the sentence that says which it is.
    let mut stats = column![
        row![
            widgets::heading(run.backend.clone()),
            widgets::sub(format!("engine load {:.0} ms", run.engine_load_ms)),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center),
        fact("Cold RTF", format!("{:.4}", run.cold_rtf)),
        widgets::sub("First run, including one-time warm-up."),
        fact("Warm RTF", warm),
        widgets::sub(match run.warm_rtf {
            None =>
                "Only one clip, so nothing ran after the first - there is no warm figure to \
                 average."
                    .to_string(),
            Some(_) => format!(
                "Mean of {} run{} after the first.",
                run.warm_count,
                if run.warm_count == 1 { "" } else { "s" }
            ),
        }),
        fact("Audio", format!("{:.1} s", run.audio_secs)),
    ]
    .spacing(4);

    // Two units is not "no result". The blend is withheld because a word rate and a character rate
    // do not average, but each unit's own total is an ordinary number and both are shown.
    if run.by_unit.len() > 1 {
        for u in &run.by_unit {
            stats = stats.push(
                row![
                    widgets::measured(
                        if u.unit == lw_core::bench::ErrorUnit::Character {
                            "CER"
                        } else {
                            "WER"
                        },
                        u.rate,
                    ),
                    widgets::sub(format!(
                        "over {} clip{}",
                        u.clips,
                        if u.clips == 1 { "" } else { "s" }
                    )),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );
        }
        stats = stats.push(widgets::prose(
            "Two figures, not one: Chinese is scored by character and the rest by word, and the \
             two cannot be averaged into a single number.",
        ));
    } else {
        match run.wer {
            Some(w) => {
                stats = stats.push(
                    row![
                        widgets::measured(run.unit.unwrap_or("WER"), w),
                        widgets::sub(format!("over {} clips", run.clips.len())),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center),
                );
            }
            None => {
                stats = stats.push(widgets::sub(
                    "No clip contributed a reference this model could be scored against.",
                ));
            }
        }
    }

    if run.skipped_clips > 0 {
        stats = stats.push(widgets::prose(format!(
            "{} clip{} in {} {} not run: this model does not claim {}. Nothing above includes \
             them, neither the accuracy nor the timings, which is the point -- a model asked to \
             transcribe a language it was never built for still takes time to produce something \
             wrong.",
            run.skipped_clips,
            if run.skipped_clips == 1 { "" } else { "s" },
            run.skipped_languages.join(", "),
            if run.skipped_clips == 1 { "was" } else { "were" },
            if run.skipped_clips == 1 {
                "that language"
            } else {
                "those languages"
            },
        )));
    }

    if !run.notes.is_empty() {
        let mut notes = column![
            widgets::field_label("Backend selection"),
            widgets::prose(
                "\u{201c}backend that ran\u{201d} above is what actually executed; these are the \
                 engine's notes on why, in the order it decided them.",
            ),
        ]
        .spacing(4);
        for note in &run.notes {
            let fallback = is_fallback(note);
            notes = notes.push(
                row![
                    if fallback {
                        widgets::badge("fallback", theme::ESTIMATE)
                    } else {
                        widgets::badge("note", theme::TEXT_DIM)
                    },
                    widgets::sub(note.clone()),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );
        }
        stats = stats.push(notes);
    }

    // Every clip, so a reader can see which one a figure came from rather than trusting the mean.
    let mut clips = column![
        widgets::field_label("Per clip"),
        widgets::sub("clip · duration · time · RTF · error"),
    ]
    .spacing(2);
    for c in &run.clips {
        clips = clips.push(widgets::sub(format!(
            "{}  {:.2}s  {:.0}ms  {:.3}  {}",
            c.name,
            c.duration_s,
            c.ms,
            c.rtf,
            match c.wer {
                Some(w) => format!("{:.2}", w),
                None => "no reference".to_string(),
            }
        )));
    }
    let clip_table: Element<'_, Message> = widgets::inset(clips).into();
    stats = stats.push(clip_table);

    widgets::card(stats).into()
}

/// One term and its definition, in the shape the web version used.
fn method_item<'a>(term: &'a str, body: &'a str) -> Element<'a, Message> {
    column![widgets::body(term), widgets::prose(body)]
        .spacing(2)
        .into()
}

fn fact(label: &str, value: String) -> Element<'_, Message> {
    row![
        container(iced::widget::text(label).size(11).color(theme::TEXT_DIM)).width(110),
        widgets::body(value),
    ]
    .spacing(8)
    .into()
}

/// Turn one progress event from the sweep into a line a reader can follow.
fn describe_progress(v: &serde_json::Value) -> String {
    let accel = v
        .get("accelerator")
        .and_then(|x| x.as_str())
        .or_else(|| v.get("label").and_then(|x| x.as_str()))
        .unwrap_or("accelerator");
    match v.get("phase").and_then(|x| x.as_str()) {
        Some("start") => format!("measuring {accel}..."),
        Some("done") => format!("{accel} finished"),
        Some("skipped") => format!(
            "{accel} skipped: {}",
            v.get("reason").and_then(|x| x.as_str()).unwrap_or("unusable")
        ),
        _ => v.to_string(),
    }
}
