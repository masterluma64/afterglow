//! The app icon, drawn in code: a play symbol in front of a sun setting into
//! a violet-to-orange sky. Used for the window at runtime, and by
//! `cargo run --example make_icon` to write `assets/icon.ico` (built into the
//! executable) and `assets/icon.png`.

/// Draws the icon at `size` × `size` pixels as straight (unpremultiplied) RGBA.
pub fn render(size: u32) -> Vec<u8> {
    // Samples per pixel along each axis, for smooth edges.
    const SAMPLES: u32 = 4;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let mut sum = [0.0f32; 4];
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let u = (x as f32 + (sx as f32 + 0.5) / SAMPLES as f32) / size as f32;
                    let v = (y as f32 + (sy as f32 + 0.5) / SAMPLES as f32) / size as f32;
                    let [r, g, b, a] = shade(u, v);
                    // Average premultiplied colour, so edges don't darken.
                    sum[0] += r * a;
                    sum[1] += g * a;
                    sum[2] += b * a;
                    sum[3] += a;
                }
            }
            let n = (SAMPLES * SAMPLES) as f32;
            let a = sum[3] / n;
            let channel = |c: f32| if a > 0.0 { ((c / n / a).clamp(0.0, 1.0) * 255.0).round() as u8 } else { 0 };
            rgba.extend([channel(sum[0]), channel(sum[1]), channel(sum[2]), (a * 255.0).round() as u8]);
        }
    }
    rgba
}

/// The colour at a point, with `u`, `v` from 0 to 1 (top left to bottom right).
fn shade(u: f32, v: f32) -> [f32; 4] {
    // The design is laid out on a rounded square inset by `margin`; stretch
    // it so the square fills the icon edge to edge.
    let margin = 0.04;
    let scale = |t: f32| margin + t * (1.0 - 2.0 * margin);
    let (u, v) = (scale(u), scale(v));
    let outside = rounded_square(u, v, margin, 0.22);
    if outside > 0.0 {
        return [0.0; 4];
    }

    const HORIZON: f32 = 0.76;
    let mut colour = if v < HORIZON {
        // Sky: the charcoal greys of the UI, a little lighter towards the horizon.
        let t = (v - margin) / (HORIZON - margin);
        gradient(t, &[(0.0, rgb(0x1E, 0x1E, 0x20)), (1.0, rgb(0x34, 0x32, 0x33))])
    } else {
        // Ground: darker, like the UI's panels.
        let t = (v - HORIZON) / (1.0 - margin - HORIZON);
        gradient(t, &[(0.0, rgb(0x1B, 0x1B, 0x1C)), (1.0, rgb(0x13, 0x13, 0x14))])
    };

    // The sun, half set, the one warm colour, with a soft glow around it.
    let (sun_x, sun_y, sun_r) = (0.5, HORIZON, 0.25);
    let d = ((u - sun_x).powi(2) + (v - sun_y).powi(2)).sqrt();
    if v < HORIZON {
        let glow = (-((d - sun_r).max(0.0) / 0.14).powi(2)).exp() * 0.35;
        colour = mix(colour, rgb(0xC8, 0x7A, 0x4A), glow);
        if d < sun_r {
            let t = d / sun_r;
            colour = gradient(t, &[(0.0, rgb(0xF0, 0xB8, 0x80)), (1.0, rgb(0xD0, 0x7C, 0x48))]);
        }
    } else {
        // Its reflection on the ground, fading downwards.
        let fade = 1.0 - ((v - HORIZON) / 0.18).clamp(0.0, 1.0);
        let glow = (-((u - sun_x).abs() / 0.16).powi(2)).exp() * 0.3 * fade;
        colour = mix(colour, rgb(0xC8, 0x7A, 0x4A), glow);
    }

    // The play symbol, slightly rounded, with a soft shadow below it.
    let play = |u: f32, v: f32| triangle(u, v, (0.39, 0.27), (0.39, 0.65), (0.71, 0.46)) - 0.035;
    let shadow = play(u, v - 0.025);
    if shadow < 0.04 {
        let strength = (1.0 - (shadow.max(0.0) / 0.04)) * 0.35;
        colour = mix(colour, rgb(0x0C, 0x0C, 0x0D), strength);
    }
    let edge = play(u, v);
    if edge < 0.0 {
        colour = rgb(0xF4, 0xF2, 0xF0);
    }

    // A faint lighter rim, so the dark square still stands out on a dark
    // taskbar.
    if outside > -0.025 {
        colour = mix(colour, rgb(0x5A, 0x5A, 0x5C), 0.5);
    }
    [colour[0], colour[1], colour[2], 1.0]
}

fn rgb(r: u8, g: u8, b: u8) -> [f32; 3] {
    [f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0]
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Interpolates between colour stops `(position, colour)`, in order.
fn gradient(t: f32, stops: &[(f32, [f32; 3])]) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    for pair in stops.windows(2) {
        let ((t0, c0), (t1, c1)) = (pair[0], pair[1]);
        if t <= t1 {
            return mix(c0, c1, (t - t0) / (t1 - t0));
        }
    }
    stops[stops.len() - 1].1
}

/// Signed distance to a square inset by `margin` with corners of `radius`
/// (negative inside).
fn rounded_square(u: f32, v: f32, margin: f32, radius: f32) -> f32 {
    let half = 0.5 - margin - radius;
    let (dx, dy) = ((u - 0.5).abs() - half, (v - 0.5).abs() - half);
    let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
    outside + dx.max(dy).min(0.0) - radius
}

/// Signed distance to a triangle (negative inside).
fn triangle(u: f32, v: f32, a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> f32 {
    let p = (u, v);
    let sub = |p: (f32, f32), q: (f32, f32)| (p.0 - q.0, p.1 - q.1);
    let dot = |p: (f32, f32), q: (f32, f32)| p.0 * q.0 + p.1 * q.1;
    let cross = |p: (f32, f32), q: (f32, f32)| p.0 * q.1 - p.1 * q.0;
    let (e0, e1, e2) = (sub(b, a), sub(c, b), sub(a, c));
    let (v0, v1, v2) = (sub(p, a), sub(p, b), sub(p, c));
    let closest = |v: (f32, f32), e: (f32, f32)| {
        let t = (dot(v, e) / dot(e, e)).clamp(0.0, 1.0);
        sub(v, (e.0 * t, e.1 * t))
    };
    let (q0, q1, q2) = (closest(v0, e0), closest(v1, e1), closest(v2, e2));
    let s = cross(e0, e2).signum();
    let d = [
        (dot(q0, q0), s * cross(v0, e0)),
        (dot(q1, q1), s * cross(v1, e1)),
        (dot(q2, q2), s * cross(v2, e2)),
    ];
    let (dist, side) = d.iter().fold((f32::MAX, f32::MAX), |(dd, ss), &(dq, sq)| (dd.min(dq), ss.min(sq)));
    -dist.sqrt() * side.signum()
}

#[cfg(test)]
mod tests {
    use super::render;

    #[test]
    fn renders_an_opaque_icon_with_transparent_corners() {
        let size = 32;
        let rgba = render(size);
        assert_eq!(rgba.len(), (size * size * 4) as usize);
        let alpha = |x: u32, y: u32| rgba[((y * size + x) * 4 + 3) as usize];
        assert_eq!(alpha(0, 0), 0, "corners are transparent");
        assert_eq!(alpha(size / 2, size / 2), 255, "the middle is opaque");
        let pixel = |x: u32, y: u32| &rgba[((y * size + x) * 4) as usize..][..3];
        assert!(pixel(size / 2, size / 2).iter().all(|&c| c > 230), "the play symbol is near white");
    }
}
