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

/// How often the animation steps. Sixty a second, not as fast as the renderer will go.
///
/// The obvious driver is `window::frames()`, but that self-sustains: each step asks for a redraw,
/// every redraw fires the subscription again, and the loop free-runs at whatever the rasteriser
/// can manage -- measured at about 120 a second here, which is twice the work for no visible
/// difference on a 60 Hz panel.
pub const STEP: std::time::Duration = std::time::Duration::from_millis(16);

/// The scroll position of one scrollable, and where it is heading.
pub struct Scroll {
    id: scrollable::Id,
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
}

impl Default for Scroll {
    fn default() -> Self {
        Self::new()
    }
}

impl Scroll {
    pub fn new() -> Self {
        Self {
            id: scrollable::Id::unique(),
            target: 0.0,
            current: 0.0,
            max: f32::INFINITY,
            last_frame: None,
        }
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
        scrollable(mouse_area(content).on_scroll(wheel))
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

    /// Jump to the top with no animation, for when the content underneath has been replaced.
    ///
    /// Sliding through a page the user has never seen, because they switched tabs, is motion that
    /// means nothing.
    pub fn reset(&mut self) -> scrollable::AbsoluteOffset {
        self.target = 0.0;
        self.current = 0.0;
        self.max = f32::INFINITY;
        self.last_frame = None;
        scrollable::AbsoluteOffset { x: 0.0, y: 0.0 }
    }

    /// The scrollable this drives, for `scroll_to`.
    pub fn id(&self) -> scrollable::Id {
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
    fn a_trackpad_is_taken_at_its_word() {
        let mut s = Scroll::new();
        s.wheel(iced::mouse::ScrollDelta::Pixels { x: 0.0, y: -17.0 });
        assert_eq!(s.target, 17.0);
    }
}
