// Music visualizer, MilkDrop style: every frame the previous frame is warped
// (zoomed, rotated, swirled, mirrored), faded and tinted, then new "ink"
// driven by the audio is drawn on top. The trails left behind are the look.

struct Uniforms {
    res: vec2f,
    time: f32,
    drive: f32,
    bass: f32,
    mid: f32,
    treble: f32,
    beat: f32,
    level: f32,
    preset: f32,
    flash: f32,
    aspect: f32,
    spec: array<vec4f, 16>,
    wave: array<vec4f, 16>,
};

@group(0) @binding(0) var prev_tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> u: Uniforms;

struct VOut {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VOut {
    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    var o: VOut;
    o.pos = vec4f(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2f(p.x, 1.0 - p.y);
    return o;
}

const TAU: f32 = 6.2831853;
// Overall brightness of newly drawn ink; the feedback loop accumulates it.
const INK: f32 = 0.35;
// Subtracted from the trails every frame so the background returns to black.
const DARKEN: f32 = 0.006;

fn band(i: i32) -> f32 {
    let c = clamp(i, 0, 63);
    return u.spec[c / 4][c % 4];
}

fn wsample(i: i32) -> f32 {
    let c = clamp(i, 0, 63);
    return u.wave[c / 4][c % 4];
}

// Linear interpolation over the 64 bands / samples, x in 0..1.
fn spec_at(x: f32) -> f32 {
    let f = clamp(x, 0.0, 1.0) * 63.0;
    let i = i32(floor(f));
    return mix(band(i), band(i + 1), fract(f));
}

fn wave_at(x: f32) -> f32 {
    let f = clamp(x, 0.0, 1.0) * 63.0;
    let i = i32(floor(f));
    return mix(wsample(i), wsample(i + 1), fract(f));
}

// Cosine palette (Inigo Quilez).
fn palette(t: f32, a: vec3f, b: vec3f, c: vec3f, d: vec3f) -> vec3f {
    return a + b * cos(TAU * (c * t + d));
}

fn rainbow(t: f32) -> vec3f {
    return palette(t, vec3f(0.5), vec3f(0.5), vec3f(1.0), vec3f(0.0, 0.33, 0.67));
}

fn rot(p: vec2f, a: f32) -> vec2f {
    let c = cos(a);
    let s = sin(a);
    return vec2f(c * p.x - s * p.y, s * p.x + c * p.y);
}

// Glowing line profile for a distance d (in screen-height units).
fn glow(d: f32, width: f32) -> f32 {
    let core = exp(-(d * d) / (width * width));
    let halo = exp(-abs(d) / (width * 3.0)) * 0.12;
    return (core + halo) * INK;
}

fn hash(p: vec2f) -> f32 {
    return fract(sin(dot(p, vec2f(127.1, 311.7))) * 43758.5453);
}

fn noise(p: vec2f) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let s = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(i), hash(i + vec2f(1.0, 0.0)), s.x),
               mix(hash(i + vec2f(0.0, 1.0)), hash(i + vec2f(1.0, 1.0)), s.x), s.y);
}

// Mirrors the angle of p into `n` symmetric slices.
fn kaleido(p: vec2f, n: f32) -> vec2f {
    let r = length(p);
    var a = atan2(p.y, p.x);
    let slice = TAU / n;
    a = abs((a - slice * floor(a / slice)) - slice * 0.5);
    return vec2f(cos(a), sin(a)) * r;
}

// Distance to a ring of radius r whose radius is modulated by the waveform.
fn wave_ring(p: vec2f, r: f32, amp: f32, spin: f32) -> f32 {
    let a = atan2(p.y, p.x) + spin;
    let t = abs(fract(a / TAU) * 2.0 - 1.0); // mirrored so the seam is continuous
    return abs(length(p) - (r + wave_at(t) * amp));
}

// Screen-centered coordinates: y in -0.5..0.5, x scaled by aspect.
fn centered(uv: vec2f) -> vec2f {
    return (uv - 0.5) * vec2f(u.aspect, 1.0);
}

fn to_uv(c: vec2f) -> vec2f {
    return c / vec2f(u.aspect, 1.0) + 0.5;
}

struct Step {
    src: vec2f,   // where to sample the previous frame (centered coords)
    decay: f32,
    ink: vec3f,
};

// --- presets ---------------------------------------------------------------

fn nebula(c: vec2f) -> Step {
    var s: Step;
    let r = length(c);
    let zoom = 0.985 - u.bass * 0.035;
    let swirl = 0.012 * sin(u.drive * 0.4) * (1.0 - r);
    s.src = rot(c * zoom, 0.004 + swirl + u.mid * 0.01);
    s.src += 0.004 * vec2f(sin(c.y * 9.0 + u.drive), cos(c.x * 9.0 - u.drive));
    s.decay = 0.95;
    let w = 1.6 / u.res.y;
    let ring = glow(wave_ring(c, 0.17 + u.bass * 0.12, 0.10, u.drive * 0.2), w * (1.5 + u.beat * 2.0));
    let col = rainbow(u.drive * 0.05 + atan2(c.y, c.x) / TAU);
    s.ink = col * ring * (0.6 + u.level);
    s.ink += vec3f(0.9, 0.6, 1.0) * glow(r, 0.03 + u.beat * 0.05) * u.beat * 0.5;
    return s;
}

fn vortex(c: vec2f) -> Step {
    var s: Step;
    let k = kaleido(rot(c, u.drive * 0.15), 6.0);
    s.src = rot(k * (1.03 + u.bass * 0.05), 0.03 + u.treble * 0.04);
    s.decay = 0.94;
    let w = 1.8 / u.res.y;
    let y = wave_at(c.x / u.aspect + 0.5) * (0.08 + u.mid * 0.1);
    let line = glow(c.y - y, w * 1.5);
    let col = palette(u.drive * 0.07 + c.x, vec3f(0.5), vec3f(0.5), vec3f(1.0, 0.7, 0.4), vec3f(0.0, 0.15, 0.2));
    s.ink = col * line * (0.7 + u.level);
    // Spectrum as dots orbiting the center.
    let a = atan2(c.y, c.x);
    let t = abs(fract(a / TAU + u.drive * 0.05) * 2.0 - 1.0);
    let rr = 0.25 + spec_at(t) * 0.18;
    s.ink += rainbow(t + u.drive * 0.1) * glow(length(c) - rr, w * 2.0) * spec_at(t) * 0.8;
    return s;
}

fn aurora(c: vec2f) -> Step {
    var s: Step;
    let sway = sin(c.y * 7.0 + u.drive * 0.8) * 0.004 + (noise(c * 4.0 + u.drive * 0.3) - 0.5) * 0.006;
    s.src = vec2f(c.x * 0.997 + sway, c.y + 0.0035 + u.treble * 0.004);
    s.decay = 0.965;
    let w = 1.8 / u.res.y;
    let x = c.x / u.aspect + 0.5;
    let mirrored = abs(x * 2.0 - 1.0);
    let h = spec_at(mirrored) * (0.3 + u.bass * 0.15);
    let curve = glow(c.y - (0.42 - h), w * 2.0);
    let col = palette(mirrored * 0.6 + u.drive * 0.03, vec3f(0.3, 0.5, 0.45), vec3f(0.35, 0.45, 0.4),
                      vec3f(1.0), vec3f(0.55, 0.25, 0.0));
    s.ink = col * curve * (0.8 + u.level);
    return s;
}

fn starburst(c: vec2f) -> Step {
    var s: Step;
    s.src = rot(c * (0.955 - u.bass * 0.04), 0.0);
    s.decay = 0.92;
    let w = 1.8 / u.res.y;
    var ink = vec3f(0.0);
    // 32 spectrum sparks on a rotating circle.
    for (var i = 0; i < 32; i++) {
        let t = f32(i) / 32.0;
        let v = spec_at(t);
        let a = t * TAU + u.drive * 0.3;
        let p = vec2f(cos(a), sin(a)) * (0.08 + v * 0.3);
        ink += rainbow(t + u.drive * 0.05) * glow(length(c - p), w * (1.0 + v * 1.5)) * (v * v * 2.0 + 0.05);
    }
    // Shockwave ring on beats.
    let ring = glow(length(c) - (1.0 - u.beat) * 0.7, w * 3.0) * u.beat;
    s.ink = ink + vec3f(1.0, 0.85, 0.7) * ring;
    return s;
}

fn liquid(c: vec2f) -> Step {
    var s: Step;
    let n = vec2f(noise(c * 3.0 + u.drive * 0.25), noise(c * 3.0 - u.drive * 0.2 + 7.0)) - 0.5;
    s.src = c * (0.995 - u.bass * 0.01) + n * (0.012 + u.mid * 0.02);
    s.decay = 0.955;
    let w = 1.6 / u.res.y;
    // Lissajous figure from the waveform against a delayed copy of itself.
    var d = 10.0;
    var prev = vec2f(0.0);
    for (var i = 0; i < 48; i++) {
        let p = rot(vec2f(wsample(i), wsample(i + 12)) * (0.18 + u.level * 0.2), u.drive * 0.2);
        if (i > 0) {
            let pa = c - prev;
            let ba = p - prev;
            let h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
            d = min(d, length(pa - ba * h));
        }
        prev = p;
    }
    s.ink = rainbow(u.drive * 0.08 + length(c)) * glow(d, w * 1.5) * (0.7 + u.level);
    return s;
}

fn prism(c: vec2f) -> Step {
    var s: Step;
    let m = abs(rot(c, u.drive * 0.1));
    s.src = rot(m * (0.99 - u.bass * 0.04), 0.01 + u.mid * 0.02) - vec2f(0.002);
    s.decay = 0.94;
    let w = 1.8 / u.res.y;
    // Radial spectrum rays.
    let a = atan2(c.y, c.x);
    let t = abs(fract(a / TAU * 2.0) * 2.0 - 1.0);
    let len = 0.06 + spec_at(t) * 0.35;
    let r = length(c);
    let ray = smoothstep(len, len - 0.02, r) * smoothstep(0.03, 0.06, r);
    let edge = abs(fract(t * 32.0) - 0.5);
    let bars = ray * smoothstep(0.1, 0.25, edge);
    s.ink = rainbow(t * 0.7 + r * 1.5 - u.drive * 0.08) * bars * 0.12;
    s.ink += vec3f(1.0) * glow(wave_ring(c, 0.05 + u.bass * 0.05, 0.03, 0.0), w) * 0.6;
    return s;
}

fn run_preset(c: vec2f) -> Step {
    let p = i32(u.preset + 0.5);
    switch p {
        case 1: { return vortex(c); }
        case 2: { return aurora(c); }
        case 3: { return starburst(c); }
        case 4: { return liquid(c); }
        case 5: { return prism(c); }
        default: { return nebula(c); }
    }
}

// Feedback pass: warp the previous frame and add new ink.
@fragment
fn fs_warp(in: VOut) -> @location(0) vec4f {
    let c = centered(in.uv);
    let s = run_preset(c);
    var col = textureSample(prev_tex, samp, to_uv(s.src)).rgb * s.decay;
    // Slow hue drift of the trails keeps colors evolving.
    let drift = 0.004 + u.treble * 0.01;
    col = vec3f(col.r * (1.0 - drift) + col.b * drift, col.g * (1.0 - drift) + col.r * drift, col.b * (1.0 - drift) + col.g * drift);
    col = max(col - vec3f(DARKEN), vec3f(0.0)) + s.ink;
    return vec4f(min(col, vec3f(4.0)), 1.0);
}

// Display pass: tone map, chromatic split on beats, vignette.
@fragment
fn fs_composite(in: VOut) -> @location(0) vec4f {
    let off = (in.uv - 0.5) * (0.004 + u.beat * 0.012);
    let r = textureSample(prev_tex, samp, in.uv + off).r;
    let g = textureSample(prev_tex, samp, in.uv).g;
    let b = textureSample(prev_tex, samp, in.uv - off).b;
    var col = 1.0 - exp(-vec3f(r, g, b) * (1.0 + u.flash * 0.6));
    let v = length(in.uv - 0.5);
    col *= smoothstep(0.85, 0.3, v);
    return vec4f(col, 1.0);
}
