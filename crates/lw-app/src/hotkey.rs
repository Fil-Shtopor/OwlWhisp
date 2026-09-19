//! The global hotkey: what a key edge *means*, and the registration that produces the edges.
//!
//! Two things live here, deliberately apart.
//!
//! [`decide`] is the whole behaviour of the three hotkey modes, as a pure function of the edge, the
//! mode, and what the worker is doing right now. It is the part that can be wrong in a way a user
//! feels -- a push-to-talk binding that stops on key auto-repeat, a toggle that starts a second
//! capture on top of the first -- and it is therefore the part that is tested, without a keyboard,
//! a window, or an OS hook anywhere near it.
//!
//! [`Binding`] is the registration: it owns the platform listener, remembers the mode the settings
//! asked for, and can be re-pointed at a new combination when the settings change. It is thin on
//! purpose, because it cannot be tested on a build machine with no input queue.
//!
//! The current state comes from the worker, not from a copy kept here. A hotkey handler that
//! remembers what it *asked* for drifts the moment anything else stops a capture -- the panel's own
//! button, a silence timeout in hands-free mode, an engine that failed to load -- and then the next
//! key press does the opposite of what the user expects. Asking is cheap; guessing is not.

use std::sync::{Arc, Mutex};

use crossbeam_channel::{Receiver, Sender, select, unbounded};
use lw_core::settings::{HotkeyConfig, HotkeyMode};
use lw_platform::{GlobalHotkey, HotkeyEvent, HotkeySpec};

use crate::dictation::{Command, RecordingState, Remote};

/// What one hotkey edge should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Begin capturing. `hands_free` lets the worker's silence detector end the utterance.
    Start { hands_free: bool },
    /// End the utterance and transcribe it.
    Stop,
    /// The edge is not meaningful here: key auto-repeat under push-to-talk, a release in a tap
    /// mode, or a press that arrived while the previous utterance is still being transcribed.
    Nothing,
}

impl Action {
    /// The worker command this action corresponds to, if any.
    pub fn command(self) -> Option<Command> {
        match self {
            Action::Start { hands_free } => Some(Command::Start { hands_free }),
            Action::Stop => Some(Command::Stop),
            Action::Nothing => None,
        }
    }
}

/// Decide what a hotkey edge means.
///
/// - **PushToTalk** — press starts, release stops. A press while already listening is key auto-
///   repeat, which the OS delivers for as long as the keys are held, and is ignored: treating it
///   as "stop" would cut the user off mid-sentence.
/// - **Toggle** — press starts, the next press stops, releases are ignored, so the keys need not be
///   held down.
/// - **HandsFree** — press starts and the worker ends the utterance on silence. A second press
///   still stops it early, for a user who has finished talking and does not want to wait out the
///   silence window.
///
/// In every mode a press during `Processing` does nothing: the previous utterance is still being
/// transcribed, and starting a second capture on top of it would interleave two transcripts.
pub fn decide(event: HotkeyEvent, mode: HotkeyMode, state: RecordingState) -> Action {
    match event {
        HotkeyEvent::Pressed => match state {
            RecordingState::Listening => {
                if mode == HotkeyMode::PushToTalk {
                    Action::Nothing
                } else {
                    Action::Stop
                }
            }
            RecordingState::Processing => Action::Nothing,
            // Idle, and the two transient states the worker passes through on its way back to it.
            RecordingState::Idle | RecordingState::Done | RecordingState::Error => Action::Start {
                hands_free: mode == HotkeyMode::HandsFree,
            },
        },
        HotkeyEvent::Released => {
            if mode == HotkeyMode::PushToTalk && state == RecordingState::Listening {
                Action::Stop
            } else {
                Action::Nothing
            }
        }
    }
}

/// The platform-level combination a settings binding asks for.
///
/// `HotkeyConfig` speaks the settings file's vocabulary and `HotkeySpec` the listener's; they
/// happen to line up, and this is the one place that assumption is written down.
pub fn spec_from(cfg: &HotkeyConfig) -> HotkeySpec {
    HotkeySpec {
        modifiers: cfg.modifiers.clone(),
        trigger: cfg.trigger.clone(),
    }
}

/// A live global-hotkey registration.
pub struct Binding {
    listener: Box<dyn GlobalHotkey>,
    spec: HotkeySpec,
    mode: HotkeyMode,
}

impl Binding {
    /// Register `cfg` with the OS.
    ///
    /// Fails if this OS has no listener (only Windows does today) or if the combination cannot be
    /// parsed. Either way the caller gets a sentence to show, not a silent fallback to some other
    /// binding: a hotkey that quietly became a different hotkey is worse than one that is plainly
    /// reported as not working, because the user cannot tell the first case from a broken machine.
    pub fn new(cfg: &HotkeyConfig) -> Result<Self, String> {
        let mut listener = lw_platform::platform()
            .hotkeys()
            .map_err(|e| e.to_string())?;
        let spec = spec_from(cfg);
        listener.register(&spec).map_err(|e| e.to_string())?;
        Ok(Self {
            listener,
            spec,
            mode: cfg.mode,
        })
    }

    /// The channel the listener publishes edges on.
    pub fn events(&self) -> Receiver<HotkeyEvent> {
        self.listener.events()
    }

    /// The mode the current binding was registered with.
    pub fn mode(&self) -> HotkeyMode {
        self.mode
    }

    /// The combination currently registered.
    pub fn spec(&self) -> &HotkeySpec {
        &self.spec
    }

    /// Point the registration at a new binding, if it is actually a different one.
    ///
    /// Re-registering the same keys is not free: between unregister and register there is a window
    /// in which the hotkey does nothing, and settings are saved on every keystroke in the editor.
    /// A mode change alone needs no OS call at all -- the keys have not moved, only their meaning.
    pub fn rebind(&mut self, cfg: &HotkeyConfig) -> Result<(), String> {
        self.mode = cfg.mode;
        let spec = spec_from(cfg);
        if spec == self.spec {
            return Ok(());
        }
        self.listener.register(&spec).map_err(|e| e.to_string())?;
        self.spec = spec;
        Ok(())
    }
}

impl std::fmt::Debug for Binding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Binding")
            .field("spec", &self.spec)
            .field("mode", &self.mode)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// How a binding is spelled on screen
// ---------------------------------------------------------------------------

/// A modifier as the settings editor offers it.
pub struct ModifierOption {
    /// The canonical name written back to the settings file.
    pub id: &'static str,
    pub label: &'static str,
    /// Extra wording for a checkbox, where the name alone is not enough.
    pub detail: Option<&'static str>,
    /// Spellings `lw-core` also accepts in a settings file written elsewhere. A binding holding
    /// one of them must still tick the right box and render with the right name.
    pub aliases: &'static [&'static str],
}

/// The four modifiers, in the order the settings editor shows them.
pub const MODIFIERS: [ModifierOption; 4] = [
    ModifierOption {
        id: "ctrl",
        label: "Ctrl",
        detail: None,
        aliases: &["ctrl", "control"],
    },
    ModifierOption {
        id: "alt",
        label: "Alt",
        detail: None,
        aliases: &["alt", "option"],
    },
    ModifierOption {
        id: "shift",
        label: "Shift",
        detail: None,
        aliases: &["shift"],
    },
    ModifierOption {
        id: "meta",
        label: "Meta",
        detail: Some("Win/Cmd"),
        aliases: &["meta", "win", "super", "cmd"],
    },
];

/// Does this binding hold `id`, under any spelling the core accepts?
pub fn has_modifier(cfg: &HotkeyConfig, id: &str) -> bool {
    let Some(opt) = MODIFIERS.iter().find(|m| m.id == id) else {
        return false;
    };
    cfg.modifiers
        .iter()
        .any(|m| opt.aliases.contains(&m.trim().to_ascii_lowercase().as_str()))
}

/// A modifier's display name. An unrecognised one is shown capitalised rather than dropped: a
/// settings file written by hand should read back as what it says, not as nothing.
pub fn modifier_label(name: &str) -> String {
    let trimmed = name.trim();
    let lower = trimmed.to_ascii_lowercase();
    if let Some(m) = MODIFIERS
        .iter()
        .find(|m| m.aliases.contains(&lower.as_str()))
    {
        return m.label.to_string();
    }
    let mut chars = trimmed.chars();
    match chars.next() {
        None => name.to_string(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// A trigger key's display name.
pub fn trigger_label(key: &str) -> String {
    match key.trim().to_ascii_lowercase().as_str() {
        "space" => "Space".into(),
        "tab" => "Tab".into(),
        "enter" | "return" => "Enter".into(),
        "esc" | "escape" => "Esc".into(),
        "capslock" | "caps_lock" => "Caps Lock".into(),
        "insert" => "Insert".into(),
        "backquote" | "grave" => "`".into(),
        "none" | "" => "no key".into(),
        _ => key.to_uppercase(),
    }
}

/// The trigger key, normalised. An empty string means the binding has no real key.
pub fn trigger_of(cfg: &HotkeyConfig) -> String {
    let t = cfg.trigger.trim().to_ascii_lowercase();
    if t == "none" { String::new() } else { t }
}

/// The key names in press order, e.g. `["Ctrl", "Alt", "Space"]`.
pub fn parts(cfg: &HotkeyConfig) -> Vec<String> {
    let mut parts: Vec<String> = cfg.modifiers.iter().map(|m| modifier_label(m)).collect();
    let trigger = trigger_of(cfg);
    if !trigger.is_empty() {
        parts.push(trigger_label(&trigger));
    }
    parts
}

/// One-line spelling of a binding, e.g. `"Ctrl + Alt + Space"`.
pub fn format_hotkey(cfg: &HotkeyConfig) -> String {
    let parts = parts(cfg);
    if parts.is_empty() {
        return "nothing bound".into();
    }
    if trigger_of(cfg).is_empty() {
        return format!("{} + (no key)", parts.join(" + "));
    }
    parts.join(" + ")
}

/// The sentence that tells a user how to dictate with this binding.
///
/// Read from the real binding rather than hardcoded, because both the keys and the mode are
/// editable, and a hint that says "press again to stop" beside a push-to-talk binding teaches the
/// user the wrong thing about their own machine.
pub fn hint(cfg: &HotkeyConfig) -> String {
    let parts = parts(cfg);
    if trigger_of(cfg).is_empty() || parts.is_empty() {
        return "No hotkey is set - pick one in Settings to dictate.".into();
    }
    let keys = parts.join(" + ");
    match cfg.mode {
        HotkeyMode::Toggle => format!("Press {keys} to start, press again to stop"),
        HotkeyMode::HandsFree => format!("Press {keys} and speak; it stops when you do"),
        HotkeyMode::PushToTalk => format!("Hold {keys} to dictate"),
    }
}

// ---------------------------------------------------------------------------
// The pump
// ---------------------------------------------------------------------------

/// Whether the hotkey is actually live, in terms a user interface can show without lying.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// The combination currently registered with the OS. `None` means nothing is.
    pub bound: Option<HotkeySpec>,
    /// Why nothing is registered, in a sentence fit to put on screen.
    pub error: Option<String>,
}

impl Status {
    /// Is a press of the configured keys going to reach the worker?
    pub fn is_live(&self) -> bool {
        self.bound.is_some() && self.error.is_none()
    }
}

/// What the owner can ask the pump thread to do.
enum Control {
    Rebind(Box<HotkeyConfig>),
    Stop,
}

/// Turns hotkey edges into worker commands, on a thread of its own.
///
/// This does not live in the front end, and the reason is not tidiness. The window can be closed
/// to the tray, minimised, or never opened at all -- this application's whole purpose is to type
/// into *other* programs -- and in every one of those cases the hotkey still has to work. A pump
/// driven by a redraw loop would also add that loop's period to the time between pressing the keys
/// and the microphone opening, which for push-to-talk is the difference between catching the first
/// word and losing it.
pub struct Pump {
    control: Sender<Control>,
    status: Arc<Mutex<Status>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Pump {
    /// Start pumping `cfg` into `remote`.
    ///
    /// Always succeeds, even where there is no listener to register with: the pump exists so that
    /// [`Pump::status`] can say *why* the hotkey is dead and [`Pump::rebind`] can try again after
    /// the user changes the binding. A constructor that failed here would leave the front end with
    /// nothing to ask.
    pub fn spawn(cfg: &HotkeyConfig, remote: Remote) -> Self {
        let (control, control_rx) = unbounded::<Control>();
        let status = Arc::new(Mutex::new(Status::default()));
        let thread_status = Arc::clone(&status);
        let cfg = cfg.clone();

        let thread = std::thread::Builder::new()
            .name("lw-hotkey-pump".into())
            .spawn(move || {
                let mut binding: Option<Binding> = None;
                let status = Arc::clone(&thread_status);
                let disconnected = Arc::clone(&thread_status);
                pump_loop(
                    cfg,
                    remote,
                    control_rx,
                    disconnected,
                    move |cfg| bind(cfg, &mut binding, &status),
                )
            })
            .ok();

        Self {
            control,
            status,
            thread,
        }
    }

    /// Whether the hotkey is live, and why not if it is not.
    pub fn status(&self) -> Status {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Re-point the hotkey at a new binding. Read [`Pump::status`] afterwards for the outcome.
    pub fn rebind(&self, cfg: &HotkeyConfig) {
        let _ = self.control.send(Control::Rebind(Box::new(cfg.clone())));
    }
}

impl Drop for Pump {
    fn drop(&mut self) {
        let _ = self.control.send(Control::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl std::fmt::Debug for Pump {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pump").field("status", &self.status()).finish()
    }
}

/// Register `cfg`, recording the outcome in `status`, and return the edge channel to listen on.
///
/// A failed registration yields [`crossbeam_channel::never`] rather than a disconnected channel:
/// `select!` on a disconnected receiver returns immediately and forever, which would turn a
/// missing hotkey into a thread spinning at the speed of the scheduler.
fn bind(
    cfg: &HotkeyConfig,
    current: &mut Option<Binding>,
    status: &Arc<Mutex<Status>>,
) -> Receiver<HotkeyEvent> {
    let outcome = match current.as_mut() {
        Some(b) => b.rebind(cfg).map(|()| b.events()),
        None => match Binding::new(cfg) {
            Ok(b) => {
                let events = b.events();
                *current = Some(b);
                Ok(events)
            }
            Err(e) => Err(e),
        },
    };

    match outcome {
        Ok(events) => {
            set_status(
                status,
                Status {
                    bound: Some(spec_from(cfg)),
                    error: None,
                },
            );
            events
        }
        Err(e) => {
            // Drop the old registration too. Leaving it in place would mean the previous keys keep
            // working while the settings screen shows the new ones -- a hotkey that is not the one
            // the user is looking at is the one bug that is impossible to diagnose from the UI.
            *current = None;
            set_status(
                status,
                Status {
                    bound: None,
                    error: Some(e),
                },
            );
            crossbeam_channel::never()
        }
    }
}

fn set_status(status: &Arc<Mutex<Status>>, next: Status) {
    if let Ok(mut s) = status.lock() {
        *s = next;
    }
}

/// The pump, with where the edges come from left open.
///
/// `attach` is the only part that touches the operating system, and taking it as an argument is
/// what lets the rest -- rebinding, the closed-channel case, and the translation from edge to
/// command -- be tested against a channel a test can write to. Production passes [`bind`].
fn pump_loop(
    cfg: HotkeyConfig,
    remote: Remote,
    control: Receiver<Control>,
    status: Arc<Mutex<Status>>,
    mut attach: impl FnMut(&HotkeyConfig) -> Receiver<HotkeyEvent>,
) {
    let mut mode = cfg.mode;
    let mut events = attach(&cfg);

    loop {
        select! {
            recv(control) -> msg => match msg {
                Ok(Control::Rebind(cfg)) => {
                    mode = cfg.mode;
                    events = attach(&cfg);
                }
                // A closed control channel means the owner is gone, which is the same instruction.
                Ok(Control::Stop) | Err(_) => break,
            },
            recv(events) -> edge => match edge {
                Ok(edge) => {
                    // The state is read now, from the worker, rather than remembered here: the
                    // capture can end without the hotkey being involved at all.
                    if let Some(cmd) = decide(edge, mode, remote.state()).command() {
                        remote.send(cmd);
                    }
                }
                // A listener that hangs up must not become a spin: `select!` on a disconnected
                // receiver is ready immediately, and forever. It must also not go unmentioned --
                // the hotkey is dead from here on and the panel has to be able to say so.
                Err(_) => {
                    set_status(&status, Status {
                        bound: None,
                        error: Some("the hotkey listener stopped sending events".into()),
                    });
                    events = crossbeam_channel::never();
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use HotkeyEvent::{Pressed, Released};
    use HotkeyMode::{HandsFree, PushToTalk, Toggle};
    use RecordingState::{Done, Error, Idle, Listening, Processing};

    #[test]
    fn push_to_talk_starts_on_press_and_stops_on_release() {
        assert_eq!(
            decide(Pressed, PushToTalk, Idle),
            Action::Start { hands_free: false }
        );
        assert_eq!(decide(Released, PushToTalk, Listening), Action::Stop);
    }

    #[test]
    fn push_to_talk_ignores_key_auto_repeat() {
        // Holding the keys down makes Windows deliver Pressed over and over. Reading the second one
        // as "stop" would end the utterance a fraction of a second after it began, every time.
        assert_eq!(decide(Pressed, PushToTalk, Listening), Action::Nothing);
    }

    #[test]
    fn toggle_starts_on_the_first_press_and_stops_on_the_second() {
        assert_eq!(
            decide(Pressed, Toggle, Idle),
            Action::Start { hands_free: false }
        );
        assert_eq!(decide(Pressed, Toggle, Listening), Action::Stop);
    }

    #[test]
    fn toggle_ignores_the_release_so_the_keys_need_not_be_held() {
        assert_eq!(decide(Released, Toggle, Listening), Action::Nothing);
    }

    #[test]
    fn hands_free_asks_the_worker_to_end_the_utterance_itself() {
        assert_eq!(
            decide(Pressed, HandsFree, Idle),
            Action::Start { hands_free: true }
        );
        // ...but a second press still stops it early, rather than making the user wait out silence.
        assert_eq!(decide(Pressed, HandsFree, Listening), Action::Stop);
        assert_eq!(decide(Released, HandsFree, Listening), Action::Nothing);
    }

    #[test]
    fn no_mode_starts_a_second_capture_while_the_first_is_still_transcribing() {
        for mode in [PushToTalk, Toggle, HandsFree] {
            assert_eq!(decide(Pressed, mode, Processing), Action::Nothing, "{mode:?}");
            assert_eq!(decide(Released, mode, Processing), Action::Nothing, "{mode:?}");
        }
    }

    #[test]
    fn the_transient_states_are_as_good_as_idle_for_starting_again() {
        // Done and Error sit on screen for a moment after the worker is already free. A press then
        // is a user starting their next sentence, not a user pressing during a busy period.
        for state in [Done, Error] {
            assert_eq!(
                decide(Pressed, PushToTalk, state),
                Action::Start { hands_free: false },
                "{state:?}"
            );
        }
    }

    #[test]
    fn a_release_that_arrives_when_nothing_is_being_captured_does_nothing() {
        // The hook can deliver a release with no matching press: the combo can be completed while
        // another window has focus, or the app can start with the keys already held.
        for state in [Idle, Done, Error, Processing] {
            assert_eq!(decide(Released, PushToTalk, state), Action::Nothing, "{state:?}");
        }
    }

    fn cfg(modifiers: &[&str], trigger: &str, mode: HotkeyMode) -> HotkeyConfig {
        HotkeyConfig {
            modifiers: modifiers.iter().map(|s| s.to_string()).collect(),
            trigger: trigger.into(),
            mode,
        }
    }

    #[test]
    fn the_hint_describes_the_mode_the_binding_is_actually_in() {
        assert_eq!(
            hint(&cfg(&["ctrl", "alt"], "space", PushToTalk)),
            "Hold Ctrl + Alt + Space to dictate"
        );
        assert_eq!(
            hint(&cfg(&["ctrl", "alt"], "space", Toggle)),
            "Press Ctrl + Alt + Space to start, press again to stop"
        );
        assert_eq!(
            hint(&cfg(&["ctrl", "alt"], "space", HandsFree)),
            "Press Ctrl + Alt + Space and speak; it stops when you do"
        );
    }

    #[test]
    fn a_binding_with_no_trigger_key_says_so_rather_than_naming_half_a_combination() {
        assert_eq!(
            hint(&cfg(&["ctrl", "win"], "none", PushToTalk)),
            "No hotkey is set - pick one in Settings to dictate."
        );
        assert_eq!(format_hotkey(&cfg(&["ctrl", "win"], "none", Toggle)), "Ctrl + Meta + (no key)");
    }

    #[test]
    fn every_spelling_the_core_accepts_ticks_the_same_box_and_prints_the_same_name() {
        // A settings file written by hand, or by an older build, can say "control" where the editor
        // writes "ctrl". Both are the same key and must look like it.
        for spelling in ["ctrl", "control", "CTRL", " Control "] {
            assert!(has_modifier(&cfg(&[spelling], "d", Toggle), "ctrl"), "{spelling}");
            assert_eq!(modifier_label(spelling), "Ctrl", "{spelling}");
        }
        for spelling in ["meta", "win", "super", "cmd"] {
            assert!(has_modifier(&cfg(&[spelling], "d", Toggle), "meta"), "{spelling}");
        }
    }

    #[test]
    fn an_unknown_modifier_is_shown_rather_than_swallowed() {
        assert_eq!(modifier_label("hyper"), "Hyper");
        assert!(!has_modifier(&cfg(&["hyper"], "d", Toggle), "ctrl"));
    }

    // -----------------------------------------------------------------------
    // The pump, against a channel instead of a keyboard
    // -----------------------------------------------------------------------

    /// A pump wired to channels instead of a keyboard and a worker.
    struct Harness {
        /// Edges, as if the hook had produced them.
        edges: Sender<HotkeyEvent>,
        /// Rebind and stop.
        control: Sender<Control>,
        /// What the pump asked the worker to do.
        commands: Receiver<Command>,
        /// What the worker would be doing, as the pump reads it back.
        state: Arc<Mutex<RecordingState>>,
        thread: std::thread::JoinHandle<()>,
    }

    /// Run a pump over channels. Nothing here touches the OS or the microphone.
    fn harness(cfg: &HotkeyConfig) -> Harness {
        let (edges_tx, edges_rx) = unbounded::<HotkeyEvent>();
        let (control_tx, control_rx) = unbounded::<Control>();
        let (remote, commands, state) = crate::dictation::Remote::detached();
        let status = Arc::new(Mutex::new(Status::default()));
        let cfg = cfg.clone();
        let thread = std::thread::spawn(move || {
            pump_loop(cfg, remote, control_rx, status, move |_| edges_rx.clone())
        });
        Harness {
            edges: edges_tx,
            control: control_tx,
            commands,
            state,
            thread,
        }
    }

    fn next(commands: &Receiver<Command>) -> Option<Command> {
        commands
            .recv_timeout(std::time::Duration::from_secs(2))
            .ok()
    }

    #[test]
    fn a_press_and_release_reach_the_worker_as_start_and_stop() {
        let h = harness(&cfg(&["ctrl", "alt"], "space", PushToTalk));

        h.edges.send(Pressed).unwrap();
        assert!(matches!(
            next(&h.commands),
            Some(Command::Start { hands_free: false })
        ));

        // The worker would have moved itself to Listening by now; say so, because that is what the
        // release edge is judged against.
        *h.state.lock().unwrap() = Listening;
        h.edges.send(Released).unwrap();
        assert!(matches!(next(&h.commands), Some(Command::Stop)));

        h.control.send(Control::Stop).unwrap();
        h.thread.join().unwrap();
    }

    #[test]
    fn rebinding_to_a_new_mode_changes_what_the_next_press_means() {
        let h = harness(&cfg(&["ctrl", "alt"], "space", PushToTalk));

        // Push-to-talk: a press while listening is auto-repeat and must produce nothing at all.
        *h.state.lock().unwrap() = Listening;
        h.edges.send(Pressed).unwrap();
        assert!(
            h.commands
                .recv_timeout(std::time::Duration::from_millis(250))
                .is_err(),
            "push-to-talk sent a command on key auto-repeat"
        );

        h.control
            .send(Control::Rebind(Box::new(cfg(
                &["ctrl", "alt"],
                "space",
                Toggle,
            ))))
            .unwrap();
        // The same press, in the new mode, is now "stop".
        h.edges.send(Pressed).unwrap();
        assert!(matches!(next(&h.commands), Some(Command::Stop)));

        h.control.send(Control::Stop).unwrap();
        h.thread.join().unwrap();
    }

    #[test]
    fn dropping_the_owner_stops_the_thread() {
        // `Pump::drop` sends Stop and joins, but a pump that outlived its owner some other way --
        // a panic on the owning thread -- must still not survive as a thread pressing keys.
        let h = harness(&cfg(&["ctrl"], "f13", Toggle));
        drop(h.control);
        h.thread.join().unwrap();
    }

    #[test]
    fn the_spec_carries_the_settings_binding_across_unchanged() {
        let cfg = HotkeyConfig {
            modifiers: vec!["ctrl".into(), "win".into()],
            trigger: "none".into(),
            mode: PushToTalk,
        };
        let spec = spec_from(&cfg);
        assert_eq!(spec.modifiers, cfg.modifiers);
        assert_eq!(spec.trigger, cfg.trigger);
    }
}
