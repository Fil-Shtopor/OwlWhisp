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

use iced::widget::{button, column, container, row, scrollable, Space};
use iced::{Element, Length, Padding, Task};
use lw_app::bench::{BenchReport, BenchSuite};

use crate::{theme, widgets};

/// What the worker thread reports while a sweep is running.
#[derive(Default)]
struct Progress {
    lines: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    RunAll,
    Tick,
    Finished(Arc<Result<BenchSuite, String>>),
}

pub struct State {
    running: bool,
    progress: Arc<Mutex<Progress>>,
    result: Option<Result<BenchSuite, String>>,
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

impl State {
    pub fn new() -> Self {
        Self {
            running: false,
            progress: Arc::new(Mutex::new(Progress::default())),
            result: None,
        }
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
            Message::RunAll => {
                if self.running {
                    return Task::none();
                }
                self.running = true;
                self.result = None;
                let progress = Arc::clone(&self.progress);
                progress.lock().map(|mut p| p.lines.clear()).ok();
                let settings_path = lw_app::paths::settings_path();

                // `spawn_blocking`, not an ordinary future: this loads an ONNX Runtime session per
                // accelerator and transcribes fifteen clips through each. On the executor thread
                // it would stop the window redrawing for the whole sweep.
                Task::perform(
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
                )
            }
            Message::Tick => Task::none(),
            Message::Finished(r) => {
                self.running = false;
                // The Arc exists only because iced messages must be Clone; nothing else holds
                // a reference by the time it arrives, so unwrapping it is the normal path.
                self.result = Arc::try_unwrap(r).ok();
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let intro: Element<'_, Message> = widgets::card(
                column![
                    widgets::heading("Measure this machine"),
                    widgets::prose(
                        "Runs the model currently selected in Settings on every accelerator this \
                         machine can actually use, over the same committed clip set, and reports \
                         what each one did. Everything here is measured now, on this computer; \
                         nothing is an estimate.",
                    ),
                    row![
                        button(widgets::body(if self.running {
                            "Measuring..."
                        } else {
                            "Compare all accelerators"
                        }))
                        .padding(Padding::from([8, 18]))
                        .on_press_maybe((!self.running).then_some(Message::RunAll)),
                        widgets::sub(
                            "A sweep loads an engine per accelerator, so it takes tens of seconds.",
                        ),
                    ]
                    .spacing(12)
                    .align_y(iced::Alignment::Center),
            ]
            .spacing(8),
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
                    body = body.push(run_card(run, suite));
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

        body = body.push(Space::new(0, 8));
        scrollable(body).height(Length::Fill).into()
    }
}

fn suite_card(suite: &BenchSuite) -> Element<'_, Message> {
    let mut c = column![
        widgets::heading("Result"),
        fact("Machine", suite.machine.clone()),
        fact("Model", suite.model_id.clone()),
        fact("Clips", suite.clip_source.clone()),
    ]
    .spacing(4);
    if let Some(f) = &suite.fastest {
        c = c.push(
            row![widgets::badge_yes("fastest"), widgets::body(f.clone())]
                .spacing(8)
                .align_y(iced::Alignment::Center),
        );
    }
    if let Some(a) = &suite.most_accurate {
        c = c.push(
            row![widgets::badge_yes("most accurate"), widgets::body(a.clone())]
                .spacing(8)
                .align_y(iced::Alignment::Center),
        );
    }
    widgets::card(c).into()
}

fn run_card<'a>(run: &'a BenchReport, _suite: &'a BenchSuite) -> Element<'a, Message> {
    let warm = run
        .warm_rtf
        .map(|r| format!("{r:.4}"))
        .unwrap_or_else(|| "— (only one clip ran)".into());

    let mut stats = column![
        row![
            widgets::heading(run.backend.clone()),
            widgets::sub(format!("engine load {:.0} ms", run.engine_load_ms)),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center),
        fact("Warm RTF", warm),
        fact("Cold RTF", format!("{:.4}", run.cold_rtf)),
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

    for note in &run.notes {
        stats = stats.push(widgets::sub(note.clone()));
    }

    // Every clip, so a reader can see which one a figure came from rather than trusting the mean.
    let mut clips = column![widgets::sub("clip · duration · time · RTF · error")].spacing(2);
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
