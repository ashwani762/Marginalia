//! The tray icon's picture: the Marginalia mark (an indigo tile with a white note card, its margin rule, the notes
//! and a cyan spark, as on the phone) and a small badge in the corner that tells the node's state by colour and,
//! from a size that has room for it, by shape too (two bars when paused, a dash when not running).
//!
//! Drawn in code with no libraries, the same on every system, at any size: the shapes are tested on a 4 by 4 grid of
//! points in each pixel, so edges are smooth at 16 pixels and at 256. The plain ring and dot in `icon.rs` stays as it was.
//! The PNG and ICO writers at the end are for packaging (`--make-icons`): the same picture becomes the program's icon.

use crate::icon;
use crate::model::State;

/// The mark's gradient, top left to bottom right, as on the phone.
const STOPS: [(f32, [f32; 3]); 3] = [(0.0, [0x8C as f32, 0x7C as f32, 0xF9 as f32]), (0.55, [0x6A as f32, 0x58 as f32, 0xF0 as f32]), (1.0, [0x3B as f32, 0x2C as f32, 0xB8 as f32])];
const SPARK: [f32; 3] = [0x22 as f32, 0xD3 as f32, 0xEE as f32];
const INK: [f32; 3] = [0x3B as f32, 0x2C as f32, 0xB8 as f32];
const WHITE: [f32; 3] = [255.0, 255.0, 255.0];
/// Around the badge, so it keeps its edge on a tile of any colour and on a light or dark taskbar.
const BADGE_RING: [f32; 3] = [16.0, 16.0, 24.0];

/// A pixel being built, premultiplied: red, green, blue (0 to 255 times alpha) and alpha (0 to 1).
type Px = [f32; 4];

fn over(dst: &mut Px, colour: [f32; 3], alpha: f32) {
    let keep = 1.0 - alpha;
    for i in 0..3 {
        dst[i] = colour[i] * alpha + dst[i] * keep;
    }
    dst[3] = alpha + dst[3] * keep;
}

fn gradient(t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    for w in STOPS.windows(2) {
        let ((a, ca), (b, cb)) = (w[0], w[1]);
        if t <= b {
            let k = (t - a) / (b - a);
            return [ca[0] + (cb[0] - ca[0]) * k, ca[1] + (cb[1] - ca[1]) * k, ca[2] + (cb[2] - ca[2]) * k];
        }
    }
    STOPS[2].1
}

/// Distance from a point to a rounded rectangle (negative inside), in the units of its arguments.
fn rrect(x: f32, y: f32, cx: f32, cy: f32, hx: f32, hy: f32, r: f32) -> f32 {
    let (qx, qy) = ((x - cx).abs() - hx + r, (y - cy).abs() - hy + r);
    (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r
}

/// Distance from a point to a line with round ends, minus its half width (negative inside).
fn capsule(x: f32, y: f32, ax: f32, ay: f32, bx: f32, by: f32, r: f32) -> f32 {
    let (px, py, dx, dy) = (x - ax, y - ay, bx - ax, by - ay);
    let k = ((px * dx + py * dy) / (dx * dx + dy * dy).max(1e-6)).clamp(0.0, 1.0);
    ((px - dx * k).powi(2) + (py - dy * k).powi(2)).sqrt() - r
}

/// What the picture is made of at `size` pixels: the finest detail only where it can be seen.
#[derive(Clone, Copy)]
struct Look {
    /// Room for the three note lines and the spark (from about 40 pixels). Smaller shows a plain white card.
    fine: bool,
    /// Room for a shape inside the badge.
    badge_shape: bool,
}

impl Look {
    fn at(size: u32) -> Look {
        Look { fine: size >= 40, badge_shape: size >= 24 }
    }
}

/// The picture at one point (`u`, `v` from 0 to 1 across the icon).
fn point(u: f32, v: f32, state: Option<State>, look: Look) -> Px {
    let mut p: Px = [0.0; 4];
    // Everything below is placed on the phone icon's 108 unit grid.
    let (x, y) = (u * 108.0, v * 108.0);

    // The tile, with the soft light from the top left corner.
    if rrect(x, y, 54.0, 54.0, 54.0, 54.0, 24.0) > 0.0 {
        return p;
    }
    over(&mut p, gradient((u + v) / 2.0), 1.0);
    let glow = (1.0 - (((x - 26.0).powi(2) + (y - 20.0).powi(2)).sqrt() / 78.0)).clamp(0.0, 1.0);
    over(&mut p, WHITE, glow * 0.33);

    // The mark is drawn a quarter larger than on the phone, so it reads in a corner of the screen.
    let s = 1.28;
    let (mx, my) = (54.0 + (x - 54.0) / s, 54.0 + (y - 54.0) / s);
    if look.fine {
        // The note card in glass, the margin rule, three notes, the spark.
        let card = rrect(mx, my, 54.0, 54.0, 23.0, 25.0, 10.0);
        if card <= 0.0 {
            over(&mut p, WHITE, 0.18);
        }
        if capsule(mx, my, 41.5, 38.0, 41.5, 70.0, 2.0) <= 0.0 {
            over(&mut p, WHITE, 0.7);
        }
        for (yy, end) in [(42.0, 67.0), (54.0, 62.0), (66.0, 65.0)] {
            if capsule(mx, my, 50.5, yy, end, yy, 2.2) <= 0.0 {
                over(&mut p, WHITE, 1.0);
            }
        }
        let (sx, sy) = ((mx - 72.0).abs() / 7.4, (my - 30.2).abs() / 7.4);
        if sx.powf(0.62) + sy.powf(0.62) <= 1.0 {
            over(&mut p, SPARK, 1.0);
        }
    } else {
        // Too small for lines to read: a white card with the margin rule and two notes in the tile's dark blue.
        if rrect(mx, my, 54.0, 54.0, 24.0, 27.0, 9.0) <= 0.0 {
            over(&mut p, WHITE, 0.96);
            if capsule(mx, my, 40.5, 38.0, 40.5, 70.0, 2.6) <= 0.0 {
                over(&mut p, INK, 0.55);
            }
            for (yy, end) in [(44.0, 68.0), (60.0, 64.0)] {
                if capsule(mx, my, 50.0, yy, end, yy, 3.4) <= 0.0 {
                    over(&mut p, INK, 1.0);
                }
            }
        }
    }

    // The badge: the state, in the corner.
    if let Some(state) = state {
        let (bx, by, br) = (81.0, 81.0, 21.0);
        let d = ((x - bx).powi(2) + (y - by).powi(2)).sqrt();
        if d <= br + 5.0 {
            over(&mut p, BADGE_RING, 0.62);
        }
        if d <= br {
            let c = icon::colour(state);
            over(&mut p, [c[0] as f32, c[1] as f32, c[2] as f32], 1.0);
            if look.badge_shape {
                let on = match state {
                    // Two bars.
                    State::Paused => capsule(x, y, bx - 6.0, by - 7.0, bx - 6.0, by + 7.0, 3.2) <= 0.0 || capsule(x, y, bx + 6.0, by - 7.0, bx + 6.0, by + 7.0, 3.2) <= 0.0,
                    // A dash.
                    State::Down => capsule(x, y, bx - 8.0, by, bx + 8.0, by, 3.4) <= 0.0,
                    // A small dot, like a keyhole.
                    State::Locked => d <= 7.0,
                    State::Running | State::Busy => false,
                };
                if on {
                    over(&mut p, WHITE, 1.0);
                }
            }
        }
    }
    p
}

fn draw(size: u32, state: Option<State>) -> Vec<u8> {
    const N: u32 = 4;
    let look = Look::at(size);
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let mut sum: Px = [0.0; 4];
            for sy in 0..N {
                for sx in 0..N {
                    let u = (px as f32 + (sx as f32 + 0.5) / N as f32) / size as f32;
                    let v = (py as f32 + (sy as f32 + 0.5) / N as f32) / size as f32;
                    let q = point(u, v, state, look);
                    for i in 0..4 {
                        sum[i] += q[i];
                    }
                }
            }
            let n = (N * N) as f32;
            let a = sum[3] / n;
            if a <= 0.0 {
                out.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                // Straight colour for the output: the premultiplied average divided by its coverage.
                for c in &sum[..3] {
                    out.push((c / n / a).round().clamp(0.0, 255.0) as u8);
                }
                out.push((a * 255.0).round() as u8);
            }
        }
    }
    out
}

/// The tray picture for `state`: RGBA, `size` by `size`.
pub fn rgba(state: State, size: u32) -> Vec<u8> {
    draw(size, Some(state))
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

/// The mark alone, with no badge, for the program's own icon.
pub fn mark_rgba(size: u32) -> Vec<u8> {
    draw(size, None)
}

// -------------------------------------------------------------------------------------------------
// Packaging: PNG and ICO files. A PNG is stored without compression, which every reader accepts and which
// keeps this free of libraries. An ICO of PNG images is read by Windows 10 and 11.
// -------------------------------------------------------------------------------------------------

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in bytes {
        a = (a + x as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut body = kind.to_vec();
    body.extend_from_slice(data);
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
}

/// A PNG file for RGBA pixels, `width` by `height`.
pub fn png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(((width * 4 + 1) * height) as usize);
    for row in rgba.chunks_exact((width * 4) as usize) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    // zlib: a header, the data in stored blocks of at most 65535 bytes, and the checksum.
    let mut z = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65_535).peekable();
    while let Some(b) = blocks.next() {
        z.push(if blocks.peek().is_none() { 1 } else { 0 });
        z.extend_from_slice(&(b.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
        z.extend_from_slice(b);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut head = Vec::new();
    head.extend_from_slice(&width.to_be_bytes());
    head.extend_from_slice(&height.to_be_bytes());
    head.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &head);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// An ICO file holding the mark at each of `sizes` (256 at most).
pub fn ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<(u32, Vec<u8>)> = sizes.iter().map(|&s| (s, png(s, s, &mark_rgba(s)))).collect();
    let mut out = vec![0, 0, 1, 0];
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (s, data) in &images {
        out.push(if *s >= 256 { 0 } else { *s as u8 });
        out.push(if *s >= 256 { 0 } else { *s as u8 });
        out.extend_from_slice(&[0, 0, 1, 0, 32, 0]);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in images {
        out.extend_from_slice(&data);
    }
    out
}

/// A picture of every state at the sizes the tray uses, each enlarged so single pixels can be seen. For looking at the icon.
pub fn sheet() -> (u32, u32, Vec<u8>) {
    const STATES: [State; 5] = [State::Running, State::Busy, State::Paused, State::Locked, State::Down];
    const SIZES: [u32; 5] = [16, 20, 24, 32, 48];
    const ZOOM: u32 = 4;
    let cell = 48 * ZOOM + 12;
    let (w, h) = (cell * SIZES.len() as u32 + 12, (cell * STATES.len() as u32) + 12);
    // A mid grey, so a light or a dark taskbar is easy to imagine on it.
    let mut out = vec![0u8; (w * h * 4) as usize];
    for p in out.chunks_exact_mut(4) {
        p.copy_from_slice(&[96, 96, 104, 255]);
    }
    for (row, st) in STATES.iter().enumerate() {
        for (col, &size) in SIZES.iter().enumerate() {
            let px = rgba(*st, size);
            let (ox, oy) = (12 + col as u32 * cell, 12 + row as u32 * cell);
            for y in 0..size * ZOOM {
                for x in 0..size * ZOOM {
                    let s = &px[(((y / ZOOM) * size + x / ZOOM) * 4) as usize..][..4];
                    let d = (((oy + y) * w + ox + x) * 4) as usize;
                    let a = s[3] as f32 / 255.0;
                    for i in 0..3 {
                        out[d + i] = (s[i] as f32 * a + out[d + i] as f32 * (1.0 - a)).round() as u8;
                    }
                }
            }
        }
    }
    (w, h, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [State; 5] = [State::Running, State::Busy, State::Paused, State::Locked, State::Down];

    fn at(px: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    }

    #[test]
    fn every_size_and_state_has_the_right_number_of_pixels_and_clear_corners() {
        for size in [16, 20, 24, 32, 48, 64] {
            for st in ALL {
                let px = rgba(st, size);
                assert_eq!(px.len() as u32, size * size * 4);
                assert_eq!(at(&px, size, 0, 0)[3], 0, "the tile's corner is clear at {size}");
            }
        }
    }

    #[test]
    fn the_badge_carries_the_state_colour() {
        for st in ALL {
            let px = rgba(st, 48);
            // The badge is in the lower right corner, its middle at about 74 percent across and down.
            let c = icon::colour(st);
            let m = at(&px, 48, 36, 36);
            if matches!(st, State::Running | State::Busy) {
                assert_eq!([m[0], m[1], m[2]], c, "{st:?}");
            }
            assert_eq!(m[3], 255);
        }
    }

    #[test]
    fn the_states_differ_in_the_badge_and_nowhere_else() {
        let a = rgba(State::Running, 48);
        let b = rgba(State::Down, 48);
        // The top left quarter is the tile and the card only.
        for y in 0..20 {
            for x in 0..20 {
                assert_eq!(at(&a, 48, x, y), at(&b, 48, x, y));
            }
        }
        assert_ne!(at(&a, 48, 36, 36), at(&b, 48, 36, 36));
    }

    #[test]
    fn paused_and_down_show_a_shape_where_there_is_room() {
        // White inside the badge at 48 pixels, none at 16 where it would only blur.
        let white = |px: &[u8], size: u32| px.chunks_exact(4).skip(((size * 3 / 4 - 3) * size) as usize).any(|p| p == [255, 255, 255, 255]);
        assert!(white(&rgba(State::Paused, 48), 48));
        assert!(white(&rgba(State::Down, 48), 48));
        let small = rgba(State::Paused, 16);
        let tile_only_white = small.chunks_exact(4).filter(|p| p[..3] == [255, 255, 255] && p[3] == 255).count();
        let big_only_white = rgba(State::Running, 16).chunks_exact(4).filter(|p| p[..3] == [255, 255, 255] && p[3] == 255).count();
        assert_eq!(tile_only_white, big_only_white, "a paused badge at 16 pixels has no shape in it");
    }

    #[test]
    fn argb_moves_alpha_first() {
        let r = rgba(State::Running, 24);
        let a = argb(State::Running, 24);
        let i = (12 * 24 + 12) * 4;
        assert_eq!(&a[i..i + 4], &[r[i + 3], r[i], r[i + 1], r[i + 2]]);
    }

    #[test]
    fn the_mark_alone_has_no_badge() {
        let px = mark_rgba(48);
        let m = at(&px, 48, 40, 40);
        assert_ne!([m[0], m[1], m[2]], icon::colour(State::Running));
    }

    #[test]
    fn png_has_its_signature_and_checksums() {
        let p = png(2, 2, &[255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 0]);
        assert_eq!(&p[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(&p[12..16], b"IHDR");
        // The checksum of "IEND" is a known value.
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn ico_lists_every_size_and_points_at_real_data() {
        let f = ico(&[16, 32, 256]);
        assert_eq!(&f[..6], &[0, 0, 1, 0, 3, 0]);
        let entry = |i: usize| &f[6 + 16 * i..6 + 16 * (i + 1)];
        assert_eq!((entry(0)[0], entry(1)[0], entry(2)[0]), (16, 32, 0), "256 is written as 0");
        for i in 0..3 {
            let len = u32::from_le_bytes(entry(i)[8..12].try_into().unwrap()) as usize;
            let off = u32::from_le_bytes(entry(i)[12..16].try_into().unwrap()) as usize;
            assert_eq!(&f[off..off + 4], &[0x89, b'P', b'N', b'G']);
            assert!(off + len <= f.len());
        }
    }
}
