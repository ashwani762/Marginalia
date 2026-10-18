//! The tray icon, drawn in code: a ring with a dot in the middle. Green when the node is running,
//! blue while it works on a job, amber when it is paused, violet when new phones must be allowed first, red when it is not running.

use crate::model::State;

pub fn colour(state: State) -> [u8; 3] {
    match state {
        State::Running => [46, 160, 67],
        State::Busy => [40, 120, 220],
        State::Paused => [230, 150, 10],
        State::Locked => [140, 90, 200],
        State::Down => [215, 60, 55],
    }
}

/// RGBA pixels, `size` by `size`, with soft (anti-aliased) edges.
pub fn rgba(state: State, size: u32) -> Vec<u8> {
    let [r, g, b] = colour(state);
    let s = size as f32;
    let (c, outer, inner, dot) = (s / 2.0, s * 0.46, s * 0.34, s * 0.19);
    let cover = |d: f32, edge: f32| (edge + 0.5 - d).clamp(0.0, 1.0);
    let mut px = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let d = (((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)) as f32).sqrt();
            // Ring: inside the outer edge and outside the inner edge. Dot: inside its own edge.
            let ring = cover(d, outer) * (1.0 - cover(d, inner));
            let a = ring.max(cover(d, dot));
            px.extend_from_slice(&[r, g, b, (a * 255.0).round() as u8]);
        }
    }
    px
}

/// The same picture as ARGB32, which is what the StatusNotifierItem protocol wants.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn argb(state: State, size: u32) -> Vec<u8> {
    let mut px = rgba(state, size);
    for p in px.chunks_exact_mut(4) {
        p.rotate_right(1);
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(px: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    }

    #[test]
    fn dot_in_a_ring_with_clear_corners() {
        for state in ALL {
            let px = rgba(state, 32);
            assert_eq!(px.len(), 32 * 32 * 4);
            let [r, g, b] = colour(state);
            assert_eq!(at(&px, 32, 16, 16), [r, g, b, 255], "dot in the middle");
            assert_eq!(at(&px, 32, 0, 0)[3], 0, "corner is clear");
            // Between dot and ring there is a clear gap.
            assert_eq!(at(&px, 32, 16 + 8, 16)[3], 0, "gap between dot and ring");
            // The ring itself is drawn.
            assert_eq!(at(&px, 32, 31 - 3, 16)[3], 255, "ring");
        }
    }

    const ALL: [State; 5] = [State::Running, State::Busy, State::Paused, State::Locked, State::Down];

    #[test]
    fn every_state_has_its_own_colour() {
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert_ne!(colour(*a), colour(*b), "{a:?} and {b:?}");
            }
        }
    }

    #[test]
    fn argb_moves_alpha_first() {
        let px = argb(State::Running, 16);
        let i = (8 * 16 + 8) * 4;
        assert_eq!(&px[i..i + 4], &[255, 46, 160, 67]);
    }
}
