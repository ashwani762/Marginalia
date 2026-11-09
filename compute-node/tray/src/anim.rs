//! Motion for the menu: springs. A spring is set by its damping ratio and the time it takes to come to its target, and it
//! always moves on from where it is and how fast it is going, so a new target in mid-motion (the pointer moving to another row
//! before the highlight arrived) bends the path instead of jumping. No operating system calls and no drawing: the window
//! says how much time has passed and reads the value, so everything here is tested without a screen.

/// The longest step taken at once. A frame later than this (the machine was busy) moves on this far and no further, so the
/// motion carries on from where it was rather than leaping to its end.
const MAX_STEP: f32 = 1.0 / 20.0;

/// Smaller steps inside one frame keep the spring accurate at any frame rate.
const SUB_STEP: f32 = 1.0 / 240.0;

/// How close counts as arrived, in the spring's own units, and how slow.
const REST: f32 = 0.0005;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    value: f32,
    velocity: f32,
    target: f32,
    /// 1.0 settles without overshoot, less than 1.0 swings past its target and back.
    damping: f32,
    /// Seconds for one swing: shorter is quicker.
    response: f32,
}

impl Spring {
    pub fn new(value: f32, damping: f32, response: f32) -> Spring {
        Spring { value, velocity: 0.0, target: value, damping, response }
    }

    pub fn value(&self) -> f32 {
        self.value
    }

    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Jumps to a value with no motion.
    pub fn snap(&mut self, value: f32) {
        self.value = value;
        self.target = value;
        self.velocity = 0.0;
    }

    /// True while it still moves, so the window keeps asking for frames and draws nothing at rest.
    pub fn moving(&self) -> bool {
        (self.value - self.target).abs() > REST || self.velocity.abs() > REST
    }

    /// Moves it on by `dt` seconds. With Reduce Motion on it lands on its target at once.
    pub fn step(&mut self, dt: f32, reduced: bool) {
        if reduced {
            self.snap(self.target);
            return;
        }
        let mut left = dt.clamp(0.0, MAX_STEP);
        let omega = std::f32::consts::TAU / self.response.max(0.02);
        while left > 0.0 {
            let h = left.min(SUB_STEP);
            let accel = omega * omega * (self.target - self.value) - 2.0 * self.damping * omega * self.velocity;
            self.velocity += accel * h;
            self.value += self.velocity * h;
            left -= h;
        }
        if !self.moving() {
            self.snap(self.target);
        }
    }
}

/// A fade over a set time that reaches exactly 0 or 1 at its end, never a faint remainder to be cut off: a spring only comes
/// near. It leaves quickly and eases into its end. For what goes away (the menu closing).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fade {
    from: f32,
    to: f32,
    seconds: f32,
    elapsed: f32,
}

impl Fade {
    pub fn new(from: f32, to: f32, seconds: f32) -> Fade {
        Fade { from, to, seconds: seconds.max(0.001), elapsed: 0.0 }
    }

    pub fn step(&mut self, dt: f32, reduced: bool) {
        self.elapsed = if reduced { self.seconds } else { (self.elapsed + dt.clamp(0.0, MAX_STEP)).min(self.seconds) };
    }

    pub fn value(&self) -> f32 {
        let t = (self.elapsed / self.seconds).clamp(0.0, 1.0);
        // An ease out: fast at first, slowing into the end.
        let eased = 1.0 - (1.0 - t) * (1.0 - t);
        self.from + (self.to - self.from) * eased
    }

    pub fn done(&self) -> bool {
        self.elapsed >= self.seconds
    }
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn clamp01(t: f32) -> f32 {
    t.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &mut Spring, seconds: f32) {
        let mut t = 0.0;
        while t < seconds {
            s.step(1.0 / 60.0, false);
            t += 1.0 / 60.0;
        }
    }

    #[test]
    fn it_arrives_and_then_stops_asking_for_frames() {
        let mut s = Spring::new(0.0, 0.9, 0.25);
        s.set_target(1.0);
        assert!(s.moving());
        run(&mut s, 2.0);
        assert_eq!(s.value(), 1.0, "it lands exactly");
        assert!(!s.moving());
    }

    #[test]
    fn a_springy_one_swings_past_and_a_critical_one_does_not() {
        let peak = |damping: f32| {
            let mut s = Spring::new(0.0, damping, 0.3);
            s.set_target(1.0);
            let mut high: f32 = 0.0;
            for _ in 0..180 {
                s.step(1.0 / 60.0, false);
                high = high.max(s.value());
            }
            high
        };
        assert!(peak(0.6) > 1.02, "an underdamped spring overshoots");
        assert!(peak(1.0) <= 1.001, "a critically damped one comes up to its target and stays");
    }

    #[test]
    fn a_new_target_in_mid_flight_goes_on_from_where_it_is() {
        let mut s = Spring::new(0.0, 0.9, 0.3);
        s.set_target(1.0);
        run(&mut s, 0.12);
        let (v, speed) = (s.value(), s.velocity);
        assert!(v > 0.05 && v < 0.95 && speed > 0.0, "it is moving: {v} {speed}");
        s.set_target(0.0);
        s.step(1.0 / 60.0, false);
        assert!((s.value() - v).abs() < 0.1, "no jump when the target changes");
        run(&mut s, 3.0);
        assert_eq!(s.value(), 0.0);
    }

    #[test]
    fn the_result_does_not_depend_on_the_frame_rate() {
        let at = |fps: f32| {
            let mut s = Spring::new(0.0, 0.8, 0.3);
            s.set_target(1.0);
            let mut t = 0.0;
            while t < 0.3 {
                s.step(1.0 / fps, false);
                t += 1.0 / fps;
            }
            s.value()
        };
        assert!((at(60.0) - at(144.0)).abs() < 0.02, "{} {}", at(60.0), at(144.0));
    }

    #[test]
    fn a_late_frame_moves_on_a_little_and_not_to_the_end() {
        let mut s = Spring::new(0.0, 1.0, 0.4);
        s.set_target(1.0);
        s.step(5.0, false);
        assert!(s.value() < 0.5, "a five second frame is only taken as a twentieth of a second: {}", s.value());
    }

    #[test]
    fn reduced_motion_lands_at_once() {
        let mut s = Spring::new(0.0, 0.8, 0.3);
        s.set_target(1.0);
        s.step(1.0 / 60.0, true);
        assert_eq!(s.value(), 1.0);
        assert!(!s.moving());
    }

    #[test]
    fn a_fade_ends_exactly_and_leaves_fast() {
        let mut f = Fade::new(1.0, 0.0, 0.12);
        f.step(0.03, false);
        f.step(0.03, false);
        assert!(f.value() < 0.3, "most of it is gone in half the time: {}", f.value());
        for _ in 0..30 {
            f.step(0.02, false);
        }
        assert!(f.done());
        assert_eq!(f.value(), 0.0);
    }
}
