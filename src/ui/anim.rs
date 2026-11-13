// anim.rs: frame-rate independent exponential approach, for the UI only.
//
// Pure Rust, no GL, no winit. The app steps every animation once per frame and
// keeps `animating` true while any of them is still moving; only then does it
// ask for another frame. When they settle, the loop goes back to `Wait` and idle
// CPU returns to 0% (v1.2 sections 8 and 0).
//
// The step is exponential rather than linear so the motion does not depend on
// frame rate: at 60 fps and at 144 fps the same animation takes the same wall
// time. `dt` is capped by the caller (50 ms) so a stalled frame cannot jump.

/// One value easing toward a target.
#[derive(Clone, Copy, Debug)]
pub struct Anim {
    value: f32,
    target: f32,
    /// Below this the value snaps to the target and the animation stops.
    snap: f32,
}

/// Position-space animations settle at a tenth of a pixel: anything finer is
/// invisible after rounding the draw position to whole pixels.
pub const SNAP_PX: f32 = 0.1;
/// Unit-space animations (0..1 fades) settle a little finer.
pub const SNAP_UNIT: f32 = 0.003;

impl Anim {
    pub fn new(value: f32) -> Self {
        Self {
            value,
            target: value,
            snap: SNAP_PX,
        }
    }

    /// A 0..1 animation, e.g. a hover fade.
    pub fn unit(value: f32) -> Self {
        Self {
            value,
            target: value,
            snap: SNAP_UNIT,
        }
    }

    pub fn value(&self) -> f32 {
        self.value
    }

    /// Set where to head. Snapping is handled by `step`, so the very next step
    /// reports the motion.
    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Jump straight to the target: what `animations = false` does.
    pub fn snap_to_target(&mut self) {
        self.value = self.target;
    }

    /// Advance by `dt` seconds. Returns true while still moving.
    ///
    /// `rate` is how fast the value chases the target: higher is snappier. The
    /// exponential form is `value += (target - value) * (1 - e^(-rate*dt))`,
    /// which is stable for any `dt` and frame-rate independent.
    pub fn step(&mut self, dt: f32, rate: f32) -> bool {
        if !dt.is_finite() || dt <= 0.0 {
            return self.is_moving();
        }
        let diff = self.target - self.value;
        if diff.abs() <= self.snap {
            self.value = self.target;
            return false;
        }
        // `rate * dt` is clamped so a pathological dt cannot overshoot.
        let k = 1.0 - (-(rate * dt).min(20.0)).exp();
        self.value += diff * k;
        if (self.target - self.value).abs() <= self.snap {
            self.value = self.target;
            return false;
        }
        true
    }

    /// True when the value has not reached its target yet.
    pub fn is_moving(&self) -> bool {
        (self.target - self.value).abs() > self.snap
    }
}

/// `dt` for one frame, in seconds, capped so a stalled frame (debugger, HDD
/// stall, window drag) does not teleport an animation.
pub fn dt_seconds(elapsed: std::time::Duration) -> f32 {
    (elapsed.as_secs_f32()).min(0.05)
}

/// Animations the shell actually uses, in one place so `animating` is a single
/// question and `animations = false` is a single switch.
#[derive(Clone, Debug)]
pub struct UiAnims {
    /// Sidebar width in points, easing between expanded, rail and hidden.
    pub sidebar_w: Anim,
    /// Scroll offset in points; the editor and the tree both draw with a
    /// fractional vertical offset from this.
    pub scroll: Anim,
    /// The hovered row's fade (0..1).
    pub hover: Anim,
    /// True when animation is enabled at all. Off means every `set_target`
    /// snaps, and the loop never asks for an extra frame.
    pub enabled: bool,
}

impl Default for UiAnims {
    fn default() -> Self {
        Self {
            sidebar_w: Anim::new(0.0),
            scroll: Anim::new(0.0),
            hover: Anim::unit(0.0),
            enabled: true,
        }
    }
}

/// Which animation to retarget. A selector instead of `&mut Anim` keeps the
/// borrow checker happy at call sites (`anims.set(Sidebar, w)`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Which {
    Sidebar,
    Scroll,
    Hover,
}

impl UiAnims {
    fn pick(&mut self, which: Which) -> &mut Anim {
        match which {
            Which::Sidebar => &mut self.sidebar_w,
            Which::Scroll => &mut self.scroll,
            Which::Hover => &mut self.hover,
        }
    }

    /// Point an animation at a new target. With `animations = false` the value
    /// jumps there, so nothing is ever mid-flight.
    pub fn set(&mut self, which: Which, target: f32) {
        let enabled = self.enabled;
        let anim = self.pick(which);
        anim.set_target(target);
        if !enabled {
            anim.snap_to_target();
        }
    }

    pub fn value(&self, which: Which) -> f32 {
        match which {
            Which::Sidebar => self.sidebar_w.value(),
            Which::Scroll => self.scroll.value(),
            Which::Hover => self.hover.value(),
        }
    }

    pub fn is_moving(&self, which: Which) -> bool {
        match which {
            Which::Sidebar => self.sidebar_w.is_moving(),
            Which::Scroll => self.scroll.is_moving(),
            Which::Hover => self.hover.is_moving(),
        }
    }

    /// Step everything once. Returns true while anything is still moving.
    pub fn step(&mut self, dt: f32) -> bool {
        if !self.enabled {
            // Disabled: nothing is ever mid-flight, so the caller never gets an
            // extra frame from here.
            self.sidebar_w.snap_to_target();
            self.scroll.snap_to_target();
            self.hover.snap_to_target();
            return false;
        }
        // Rates: the sidebar is a quick 150 ms-ish move, scrolling a little
        // slower so a wheel flick reads as motion, hover quickest.
        let a = self.sidebar_w.step(dt, 22.0);
        let b = self.scroll.step(dt, 18.0);
        let c = self.hover.step(dt, 28.0);
        a || b || c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_animation_reaches_its_target_and_stops() {
        let mut a = Anim::new(0.0);
        a.set_target(100.0);
        assert!(a.is_moving());
        let mut frames = 0;
        while a.step(1.0 / 60.0, 22.0) {
            frames += 1;
            assert!(frames < 600, "must settle");
        }
        assert_eq!(a.value(), 100.0);
        assert!(!a.is_moving());
    }

    #[test]
    fn it_is_frame_rate_independent() {
        // Same wall time at 60 and 144 fps must land in the same place.
        let mut slow = Anim::new(0.0);
        let mut fast = Anim::new(0.0);
        slow.set_target(100.0);
        fast.set_target(100.0);
        for _ in 0..30 {
            slow.step(1.0 / 60.0, 20.0);
        }
        for _ in 0..72 {
            fast.step(1.0 / 144.0, 20.0);
        }
        assert!(
            (slow.value() - fast.value()).abs() < 1.0,
            "{} vs {}",
            slow.value(),
            fast.value()
        );
    }

    #[test]
    fn it_is_most_of_the_way_there_in_150_ms() {
        // rate 22 over 150 ms: 1 - e^(-3.3) = 96.3% of the distance. The spec's
        // "about 150 ms" for the sidebar means this, not that it is pixel-exact.
        let mut a = Anim::new(0.0);
        a.set_target(100.0);
        for _ in 0..9 {
            a.step(1.0 / 60.0, 22.0);
        }
        assert!(a.value() > 95.0, "got {}", a.value());
    }

    #[test]
    fn a_huge_dt_does_not_overshoot() {
        let mut a = Anim::new(0.0);
        a.set_target(10.0);
        a.step(10.0, 22.0);
        assert!(a.value() <= 10.0);
    }

    #[test]
    fn a_degenerate_dt_does_not_move_the_value() {
        let mut a = Anim::new(0.0);
        a.set_target(10.0);
        assert!(a.step(0.0, 22.0));
        assert_eq!(a.value(), 0.0);
        assert!(a.step(f32::NAN, 22.0));
        assert_eq!(a.value(), 0.0);
        assert!(a.step(-1.0, 22.0));
        assert_eq!(a.value(), 0.0);
    }

    #[test]
    fn snapping_stops_the_motion_immediately() {
        let mut a = Anim::new(0.0);
        a.set_target(50.0);
        a.step(1.0 / 60.0, 22.0);
        assert!(a.is_moving());
        a.snap_to_target();
        assert_eq!(a.value(), 50.0);
        assert!(!a.step(1.0 / 60.0, 22.0));
    }

    #[test]
    fn unit_animations_use_a_finer_snap() {
        let mut a = Anim::unit(0.0);
        a.set_target(1.0);
        // A single tiny step must not immediately count as finished.
        a.step(0.001, 28.0);
        assert!(a.value() > 0.0);
        let mut guard = 0;
        while a.step(1.0 / 60.0, 28.0) {
            guard += 1;
            assert!(guard < 600);
        }
        assert_eq!(a.value(), 1.0);
    }

    #[test]
    fn dt_is_capped_for_a_stalled_frame() {
        assert_eq!(dt_seconds(std::time::Duration::from_millis(500)), 0.05);
        assert!((dt_seconds(std::time::Duration::from_millis(16)) - 0.016).abs() < 1e-6);
    }

    #[test]
    fn disabled_animations_never_report_motion() {
        let mut anims = UiAnims {
            enabled: false,
            ..UiAnims::default()
        };
        anims.set(Which::Sidebar, 220.0);
        assert_eq!(anims.value(Which::Sidebar), 220.0, "snapped");
        assert!(!anims.is_moving(Which::Sidebar));
        assert!(!anims.step(1.0 / 60.0), "never asks for another frame");
    }

    #[test]
    fn enabled_animations_report_motion_until_they_settle() {
        let mut anims = UiAnims::default();
        anims.set(Which::Sidebar, 220.0);
        assert!(anims.is_moving(Which::Sidebar));
        assert!(anims.step(1.0 / 60.0));
        let mut guard = 0;
        while anims.step(1.0 / 60.0) {
            guard += 1;
            assert!(guard < 600, "animations must settle");
        }
        assert_eq!(anims.value(Which::Sidebar), 220.0);
        assert!(!anims.is_moving(Which::Sidebar));
    }

    #[test]
    fn every_selector_addresses_its_own_animation() {
        let mut anims = UiAnims::default();
        anims.set(Which::Sidebar, 10.0);
        anims.set(Which::Scroll, 20.0);
        anims.set(Which::Hover, 0.5);
        assert_eq!(anims.value(Which::Sidebar), 0.0, "not stepped yet");
        // Each selector drives its own field: stepping moves sidebar and scroll,
        // and leaves the hover alone because its target never changed.
        anims.step(1.0 / 60.0);
        assert!(anims.value(Which::Sidebar) > 0.0);
        assert!(anims.value(Which::Scroll) > 0.0);
        assert_ne!(anims.value(Which::Sidebar), anims.value(Which::Scroll));
        assert!(anims.is_moving(Which::Scroll));
        // The hover was retargeted to 0.5 from 0.0, so it is moving too.
        assert!(anims.is_moving(Which::Hover));
        // Setting one back to where it already is clears only that one.
        anims.set(Which::Hover, anims.value(Which::Hover));
        assert!(!anims.is_moving(Which::Hover));
        assert!(anims.is_moving(Which::Scroll), "the others are untouched");
    }
}
