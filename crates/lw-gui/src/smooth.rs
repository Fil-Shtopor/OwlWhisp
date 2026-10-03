//! Scrolling that moves rather than jumps.
//!
//! iced's `scrollable` applies a wheel notch to its offset immediately: one event, one 60-point
//! step, no motion in between. A browser spends 100-200 ms travelling that distance, and the
//! difference is the whole of why this window felt worse to scroll than the one it replaced --
//! measurably it was not slow (a frame costs about 11 ms on a maximised window, and a burst of
//! events coalesces to under 2 ms each), it simply arrived in visible jumps.
//!
//! So the wheel is taken away from the scrollable and given to a spring. A `mouse_area` wrapped
//! around the content captures the wheel event -- `scrollable` checks whether its content captured
//! before acting, so it never sees it -- and the offset is driven here instead, one step per
//! rendered frame, easing toward wherever the wheel has asked for.
//!
//! The cost is frames: while a scroll is in flight the window redraws at its refresh rate rather
//! than once per notch. That is the trade, and it is bounded -- when nothing is moving this costs
//! exactly nothing, and the animation stops as soon as it arrives.

use std::time::Instant;

use iced::widget::{mouse_area, scrollable};
use iced::{Element, Length};

/// How quickly the offset closes the distance to the target.
///
/// Exponential easing with this time constant: roughly two thirds of the remaining distance every
/// 70 ms. Fast enough that the window does not feel like it is lagging behind the wheel, slow
/// enough to read as movement rather than as a jump.
const TAU_SECS: f32 = 0.06;

/// How far one wheel notch travels, in logical points. The same distance iced itself uses, so the
/// feel of a single notch is unchanged -- only the way it gets there.
const NOTCH: f32 = 60.0;

/// Below this the animation has arrived.
///
/// Sub-pixel: chasing the last fraction of a point costs frames for a distance nobody can see, and
/// the exponential tail is long.
pub const SETTLED: f32 = 0.6;

/// The fastest the animation will step, however quick the panel claims to be.
///
/// Two hundred and forty a second. There is no need for a lower cap tied to what the rasteriser
/// can manage: a tick that arrives while the previous frame is still being drawn does not produce
/// an extra frame, because redraw requests coalesce -- it only advances the easing, which is a
/// function of elapsed time and therefore already correct at any rate. The cap is here so that an
/// absurd reported rate cannot turn into a busy loop.
const FASTEST_STEP: std::time::Duration = std::time::Duration::from_micros(4_000);

/// The slowest, for a display that reports something implausible or nothing at all.
const SLOWEST_STEP: std::time::Duration = std::time::Duration::from_micros(16_667);

/// How many display frames one step may span before the motion is worse than slowing down.
///
/// Three is 40 a second on this panel, which still reads as movement. Past that the wait is long
/// enough to read as a series of jumps whatever its regularity, and the real answer becomes a
/// cheaper frame rather than a longer slot.
const MAX_DIVISOR: u32 = 3;

/// How many steps to judge before changing the rate.
///
/// Long enough that one slow frame -- a tooltip appearing, another process waking -- does not
/// change the rate, short enough that the adjustment happens within the first scroll rather than
/// the third.
const WINDOW: u32 = 12;

/// How far past its slot a step must land to count as missed. A little slack, because a step that
/// arrives a fraction late still draws on time.
const LATE: f32 = 1.4;

/// How often the animation steps, matched to the display.
///
/// Not `window::frames()`, which self-sustains: each step asks for a redraw, every redraw fires
/// the subscription again, and the loop free-runs at whatever the rasteriser can manage -- about
/// 120 a second here, and the frames past the panel's own rate are work nobody sees.
///
/// Not a fixed sixty either. This panel runs at 120 Hz, and stepping at 60 shows every frame
/// twice, which the eye reads as exactly the stepping a smooth scroll exists to remove.
///
/// And not whole milliseconds, which is the trap this fell into first. A 120 Hz panel refreshes
/// every 8.333 ms; `1000 / 120` is 8, four percent fast, so every twenty-fifth step landed in a
/// refresh that already had one and was never shown -- a hitch, at a beat of about five a second,
/// in motion that was otherwise perfectly regular. The multiples were wrong for the same reason:
/// three refreshes are 25 ms, not 24. Measured, the same tab scrolled three times settled at 16 ms
/// twice and 24 ms once, and the 24 ms run was half again as uneven as the 16 ms ones -- the rate
/// was not the problem, the rounding was.
fn step_for_display() -> std::time::Duration {
    lw_platform::screen::refresh_hz()
        .map(period_for)
        .unwrap_or(SLOWEST_STEP)
}

/// One refresh at `hz`, within the bounds worth stepping at.
fn period_for(hz: u32) -> std::time::Duration {
    std::time::Duration::from_secs_f64(1.0 / f64::from(hz.max(1))).clamp(FASTEST_STEP, SLOWEST_STEP)
}

/// The scroll position of one scrollable, and where it is heading.
pub struct Scroll {
    id: iced::widget::Id,
    /// Where the wheel has asked to be.
    target: f32,
    /// Where the view actually is.
    current: f32,
    /// The furthest the content can be scrolled, learnt from the scrollable itself. Without it a
    /// user who keeps scrolling past the end builds up a target metres beyond the content, and
    /// scrolling back does nothing until the debt is paid off.
    max: f32,
    /// When the last frame was, so the easing is a function of time rather than of frame rate.
    last_frame: Option<Instant>,
    /// How often to step, read from the display once.
    step: std::time::Duration,
    /// How many display frames each step currently spans.
    ///
    /// One is every frame. It rises when the window cannot draw that fast -- see `judge`.
    divisor: u32,
    /// Steps judged since the rate last changed, and how many of them arrived late.
    judged: u32,
    missed: u32,
}

impl Default for Scroll {
    fn default() -> Self {
        Self::new()
    }
}

impl Scroll {
    pub fn new() -> Self {
        Self {
            id: iced::widget::Id::unique(),
            target: 0.0,
            current: 0.0,
            max: f32::INFINITY,
            last_frame: None,
            step: step_for_display(),
            divisor: 1,
            judged: 0,
            missed: 0,
        }
    }

    /// How often this wants to be ticked while a scroll is in flight.
    ///
    /// The display's frame time, or a multiple of it when the window has shown it cannot draw that
    /// fast. Asking for more frames than can be delivered does not produce more frames -- it
    /// produces uneven ones, and unevenness is what the eye reads as stutter. A steady forty is
    /// smoother than a sixty that is really a hundred and twenty every third frame.
    pub fn step(&self) -> std::time::Duration {
        self.step * self.divisor
    }

    /// Wrap `content` in a scrollable whose wheel this type owns.
    ///
    /// `wheel` and `viewport` are the messages the caller wants back. The `mouse_area` sits
    /// *inside* the scrollable on purpose: iced dispatches an event to the content first and
    /// `scrollable` returns early when the content captured it, which is what takes the wheel out
    /// of its hands without forking it.
    pub fn view<'a, M: Clone + 'a>(
        &self,
        content: impl Into<Element<'a, M>>,
        wheel: impl Fn(iced::mouse::ScrollDelta) -> M + 'a,
        viewport: impl Fn(scrollable::Viewport) -> M + 'a,
    ) -> Element<'a, M> {
        // The container makes the wheel area the whole viewport rather than just the content.
        //
        // `mouse_area` captures the wheel only while the pointer is within its own bounds, and its
        // bounds are the content's. Every panel here happens to fill the width today, so this
        // changes nothing for them -- but a panel that did not would leave a strip down the side
        // where a notch reached the scrollable directly and jumped, and that is a trap to close
        // rather than to remember.
        scrollable(mouse_area(iced::widget::container(content).width(Length::Fill)).on_scroll(wheel))
            .id(self.id.clone())
            .on_scroll(viewport)
            .height(Length::Fill)
            .into()
    }

    /// A wheel notch, or a trackpad's pixel delta.
    pub fn wheel(&mut self, delta: iced::mouse::ScrollDelta) {
        let by = match delta {
            // A notch is a distance, not a pixel count: the OS reports lines and the toolkit
            // decides what a line is worth.
            iced::mouse::ScrollDelta::Lines { y, .. } => -y * NOTCH,
            // A precision trackpad already speaks in pixels, and second-guessing it is how
            // trackpad scrolling ends up feeling wrong.
            iced::mouse::ScrollDelta::Pixels { y, .. } => -y,
        };
        self.target = (self.target + by).clamp(0.0, self.max);
    }

    /// What the scrollable reports about itself after every move.
    ///
    /// Two things are learnt here: how far it can go, and -- when nothing of ours is in flight --
    /// where it actually is, because the scrollbar can be dragged and the keyboard can move it.
    pub fn observed(&mut self, viewport: scrollable::Viewport) {
        let content = viewport.content_bounds().height;
        let window = viewport.bounds().height;
        self.max = (content - window).max(0.0);
        if !self.animating() {
            self.current = viewport.absolute_offset().y;
            self.target = self.current.clamp(0.0, self.max);
        }
    }

    /// Is there still distance to cover?
    pub fn animating(&self) -> bool {
        (self.target - self.current).abs() > SETTLED
    }

    /// Advance by one frame. `None` once it has arrived.
    pub fn tick(&mut self, now: Instant) -> Option<scrollable::AbsoluteOffset> {
        let elapsed = self
            .last_frame
            .map(|t| (now - t).as_secs_f32())
            // The first frame of a scroll has no previous one to measure against. A nominal 60 Hz
            // step is closer than zero, which would move nothing.
            .unwrap_or(1.0 / 60.0)
            // A frame that took a quarter of a second -- a benchmark starting, a model loading --
            // must not teleport the view. Cap it and let the next frames catch up.
            .min(0.1);
        self.last_frame = Some(now);
        // The pacing below is decided by measurement, so the measurement is worth being able to
        // see. At `debug` this is the only honest answer to "is it actually smooth?" -- the gaps
        // the frames really landed in, rather than the ones that were asked for.
        tracing::debug!(
            gap_ms = (elapsed * 1000.0) as u32,
            slot_ms = self.step().as_millis() as u32,
            "scroll frame"
        );
        self.judge(elapsed);

        if !self.animating() {
            self.current = self.target;
            self.last_frame = None;
            return None;
        }

        // Exponential approach: frame-rate independent, and it decelerates into the target, which
        // is what makes it read as movement rather than as a slide.
        let fraction = 1.0 - (-elapsed / TAU_SECS).exp();
        self.current += (self.target - self.current) * fraction;
        Some(scrollable::AbsoluteOffset {
            x: 0.0,
            y: self.current,
        })
    }

    /// Watch whether the steps are landing in their slots, and slow down when they are not.
    ///
    /// The rate only ever falls, and is reset by `remeasure` when the cost of a frame has actually
    /// changed -- a different tab, a different window size. It is tempting to let it climb back on
    /// its own, and wrong: once slowed, every window looks clean, because it is being judged at
    /// the slower rate. That reads as permission to speed up, the frames are missed again, and the
    /// rate ends up alternating between the two -- which is precisely the unevenness this exists
    /// to remove. Better a rate that is occasionally more conservative than the machine requires
    /// than one that is never steady.
    ///
    /// Measured rather than assumed, because the cost of a frame is a property of this window on
    /// this machine at this size -- a wide window drawn on the CPU costs several times a narrow
    /// one, and which tab is showing changes it again. Nothing here can be decided ahead of time.
    ///
    /// A quarter late is already visible: the eye finds one frame in four arriving at double the
    /// interval far more objectionable than every frame arriving at that interval. So the bar for
    /// backing off is a quarter, well below the half that would mean the rate is merely nominal.
    fn judge(&mut self, elapsed: f32) {
        let slot = self.step().as_secs_f32();
        self.judged += 1;
        if elapsed > slot * LATE {
            self.missed += 1;
        }
        if self.judged < WINDOW {
            return;
        }
        if self.missed * 4 >= self.judged && self.divisor < MAX_DIVISOR {
            self.divisor += 1;
            tracing::debug!(
                late = self.missed,
                of = self.judged,
                now_ms = self.step().as_millis() as u32,
                "the window cannot draw that fast; stepping the scroll less often"
            );
        }
        self.judged = 0;
        self.missed = 0;
    }

    /// Jump to the top with no animation, for when the content underneath has been replaced.
    ///
    /// Sliding through a page the user has never seen, because they switched tabs, is motion that
    /// means nothing.
    pub fn reset(&mut self) -> scrollable::AbsoluteOffset {
        self.target = 0.0;
        self.current = 0.0;
        self.max = f32::INFINITY;
        self.last_frame = None;
        // A different tab costs a different amount to draw, so what was learnt about the last one
        // says nothing about this one.
        self.remeasure();
        scrollable::AbsoluteOffset { x: 0.0, y: 0.0 }
    }

    /// Forget what the last frames cost and start optimistic again.
    ///
    /// For when the drawing itself has changed -- another tab, another window size -- so that a
    /// window made small, or a cheap tab, is not left stepping at the rate a large expensive one
    /// needed.
    pub fn remeasure(&mut self) {
        self.divisor = 1;
        self.judged = 0;
        self.missed = 0;
    }

    /// The scrollable this drives, for `scroll_to`.
    pub fn id(&self) -> iced::widget::Id {
        self.id.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn lines(y: f32) -> iced::mouse::ScrollDelta {
        iced::mouse::ScrollDelta::Lines { x: 0.0, y }
    }

    #[test]
    fn a_notch_asks_to_travel_but_the_view_has_not_moved_yet() {
        // The whole point: the wheel sets a destination, it does not set the position.
        let mut s = Scroll::new();
        s.wheel(lines(-1.0));
        assert_eq!(s.target, NOTCH);
        assert_eq!(s.current, 0.0);
        assert!(s.animating());
    }

    #[test]
    fn it_arrives_and_then_stops_asking_for_frames() {
        let mut s = Scroll::new();
        s.wheel(lines(-1.0));

        let mut now = Instant::now();
        let mut frames = 0;
        while s.animating() && frames < 1000 {
            now += Duration::from_millis(16);
            s.tick(now);
            frames += 1;
        }
        assert!(frames < 100, "took {frames} frames to travel one notch");
        assert!((s.current - NOTCH).abs() <= SETTLED, "{}", s.current);

        // And once there, it hands the frames back.
        assert!(!s.animating());
        assert!(s.tick(now + Duration::from_millis(16)).is_none());
    }

    #[test]
    fn the_distance_covered_does_not_depend_on_the_frame_rate() {
        // Easing by a fixed fraction per frame would travel further on a fast display than a slow
        // one, which is the classic way animation goes wrong.
        let travel = |step_ms: u64| {
            let mut s = Scroll::new();
            s.wheel(lines(-10.0));
            let mut now = Instant::now();
            for _ in 0..(300 / step_ms) {
                now += Duration::from_millis(step_ms);
                s.tick(now);
            }
            s.current
        };
        let at_60 = travel(16);
        let at_30 = travel(33);
        assert!(
            (at_60 - at_30).abs() < NOTCH * 0.4,
            "60 Hz reached {at_60}, 30 Hz reached {at_30}"
        );
    }

    #[test]
    fn scrolling_past_the_end_does_not_build_up_a_debt() {
        // Otherwise the wheel keeps adding to a target far below the content, and scrolling back
        // does nothing at all until all of it has been paid off.
        let mut s = Scroll::new();
        s.max = 100.0;
        for _ in 0..20 {
            s.wheel(lines(-1.0));
        }
        assert_eq!(s.target, 100.0);
        s.wheel(lines(1.0));
        assert_eq!(s.target, 40.0, "one notch back must move one notch");
    }

    #[test]
    fn a_stalled_frame_does_not_teleport_the_view() {
        // A model loading or a benchmark starting can stall the loop for a moment. Easing by the
        // real elapsed time would cover the whole distance in one step and look like a jump --
        // which is exactly what this exists to avoid.
        let mut s = Scroll::new();
        s.wheel(lines(-100.0));
        let now = Instant::now();
        s.tick(now);

        let before = s.current;
        let remaining = s.target - before;
        s.tick(now + Duration::from_secs(3));
        let moved = s.current - before;

        // Treated as the capped tenth of a second, not as three seconds: most of the distance but
        // not all of it, and there is still somewhere to travel on the frames that follow.
        assert!(
            moved < remaining * 0.9,
            "one stalled frame covered {moved} of {remaining}"
        );
        assert!(s.animating(), "one stalled frame swallowed the whole scroll");
    }

    #[test]
    fn a_refresh_is_not_a_whole_number_of_milliseconds() {
        // The bug this exists to prevent: `1000 / 120` is 8, and a step of 8 ms against a panel
        // that refreshes every 8.333 ms gains a whole refresh every twenty-fifth frame, where two
        // steps share one refresh and the first is never shown. Perfectly regular arithmetic, a
        // visible hitch about five times a second.
        let at_120 = period_for(120);
        assert!(
            at_120 > Duration::from_micros(8_300) && at_120 < Duration::from_micros(8_400),
            "120 Hz gave {at_120:?}, not a refresh"
        );
        // And the multiples have to be multiples of the real thing: three refreshes are 25 ms.
        assert!(
            at_120 * 3 > Duration::from_millis(24) && at_120 * 3 < Duration::from_millis(26),
            "three refreshes came to {:?}",
            at_120 * 3
        );
        let at_60 = period_for(60);
        assert!(
            at_60 > Duration::from_micros(16_600) && at_60 < Duration::from_micros(16_700),
            "60 Hz gave {at_60:?}"
        );
    }

    #[test]
    fn an_implausible_refresh_rate_cannot_make_a_busy_loop_or_a_slideshow() {
        assert_eq!(period_for(10_000), FASTEST_STEP);
        assert_eq!(period_for(1), SLOWEST_STEP);
        assert_eq!(period_for(0), SLOWEST_STEP);
    }

    #[test]
    fn a_trackpad_is_taken_at_its_word() {
        let mut s = Scroll::new();
        s.wheel(iced::mouse::ScrollDelta::Pixels { x: 0.0, y: -17.0 });
        assert_eq!(s.target, 17.0);
    }
}

#[cfg(test)]
mod pace_tests {
    use super::*;
    use std::time::Duration;

    /// Drive a scroll for `frames` steps, each one taking `cost_ms` to draw, and report the rate
    /// it settled on.
    fn settle(cost_ms: u64, frames: u32) -> Duration {
        let mut s = Scroll::new();
        s.step = Duration::from_millis(8);
        s.max = f32::INFINITY;
        let mut now = Instant::now();
        for _ in 0..frames {
            s.wheel(lines(-1.0));
            // A frame lands when the step is due or when the drawing finishes, whichever is later.
            now += s.step().max(Duration::from_millis(cost_ms));
            s.tick(now);
        }
        s.step()
    }

    fn lines(y: f32) -> iced::mouse::ScrollDelta {
        iced::mouse::ScrollDelta::Lines { x: 0.0, y }
    }

    #[test]
    fn a_window_that_keeps_up_is_left_at_the_display_rate() {
        assert_eq!(settle(4, 60), Duration::from_millis(8));
    }

    #[test]
    fn a_window_that_cannot_draw_in_time_is_asked_for_fewer_frames_instead() {
        // The Benchmark tab, measured: sixteen milliseconds a frame against an eight millisecond
        // slot. Asking every eight produces 8/16/24 jitter; asking every sixteen produces sixteen.
        assert_eq!(settle(16, 60), Duration::from_millis(16));
    }

    #[test]
    fn it_never_backs_off_so_far_that_the_motion_stops_reading_as_motion() {
        assert_eq!(
            settle(500, 120),
            Duration::from_millis(8 * u64::from(MAX_DIVISOR))
        );
    }

    #[test]
    fn a_quarter_of_the_frames_arriving_late_is_enough_to_slow_down() {
        // Models and Diagnostics, measured: not late often enough to be a majority, late often
        // enough to be seen.
        let mut s = Scroll::new();
        s.step = Duration::from_millis(8);
        let mut now = Instant::now();
        for i in 0..WINDOW {
            s.wheel(lines(-1.0));
            now += Duration::from_millis(if i % 4 == 0 { 16 } else { 8 });
            s.tick(now);
        }
        assert_eq!(s.step(), Duration::from_millis(16));
    }

    #[test]
    fn one_slow_frame_does_not_change_the_rate() {
        let mut s = Scroll::new();
        s.step = Duration::from_millis(8);
        let mut now = Instant::now();
        for i in 0..WINDOW {
            s.wheel(lines(-1.0));
            now += Duration::from_millis(if i == 3 { 40 } else { 8 });
            s.tick(now);
        }
        assert_eq!(s.step(), Duration::from_millis(8));
    }

    #[test]
    fn the_rate_does_not_flap_between_two_values() {
        // Once slowed, a clean window at the slower rate must not be read as proof that the faster
        // one would work -- that is how an adaptive rate ends up oscillating, which looks worse
        // than either rate on its own.
        let mut s = Scroll::new();
        s.step = Duration::from_millis(8);
        let mut now = Instant::now();
        let mut rates = Vec::new();
        for _ in 0..(WINDOW * 4) {
            s.wheel(lines(-1.0));
            // Twelve milliseconds: too slow for an eight millisecond slot, comfortable in sixteen.
            now += s.step().max(Duration::from_millis(12));
            s.tick(now);
            rates.push(s.step());
        }
        assert_eq!(*rates.last().unwrap(), Duration::from_millis(16));
        let changes = rates.windows(2).filter(|w| w[0] != w[1]).count();
        assert_eq!(changes, 1, "the rate changed {changes} times: {rates:?}");
    }

    #[test]
    fn switching_tabs_forgets_what_the_last_one_cost() {
        let mut s = Scroll::new();
        s.step = Duration::from_millis(8);
        s.divisor = 3;
        s.reset();
        assert_eq!(s.step(), Duration::from_millis(8));
    }
}
