// The window pass: space with stars, and the map's frame over it.
//
// On the globe mln-rs draws the planet opaque, its atmosphere translucent and
// clears the rest to transparent, in premultiplied alpha, so
// `map + stars * (1 - alpha)` shows the stars around the planet, dimmed
// behind the atmosphere's glow.

struct Params {
    // The framebuffer's size in pixels.
    size: vec2<f32>,
    // Seconds since the window opened.
    time: f32,
    // Framebuffer pixels per logical pixel.
    pixel_ratio: f32,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var map_frame: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    // One triangle that covers the screen.
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}

// Three random numbers in [0, 1] for a grid cell: pcg3d, from Jarzynski and
// Olano, "Hash Functions for GPU Rendering" (2020).
fn random3(cell: vec2<i32>, seed: u32) -> vec3<f32> {
    var v = vec3<u32>(bitcast<vec2<u32>>(cell), seed) * 1664525u + 1013904223u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v ^= v >> vec3<u32>(16u);
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return vec3<f32>(v) / 4294967295.0;
}

// Stars on a grid of `cell` logical pixels: one in `chance` of the cells, at a
// random place in its cell, most of them faint and a few bright.
fn stars(p: vec2<f32>, cell: f32, chance: f32, seed: u32, brightness: f32) -> vec3<f32> {
    // The four cells nearest to p hold every star whose light reaches it.
    let first = vec2<i32>(floor(p / cell - 0.5));
    var light = vec3<f32>(0.0);
    for (var y = 0; y < 2; y += 1) {
        for (var x = 0; x < 2; x += 1) {
            let id = first + vec2<i32>(x, y);
            let r = random3(id, seed);
            if (r.x >= chance) {
                continue;
            }
            let s = random3(id, seed + 1000u);
            let center = (vec2<f32>(id) + 0.1 + 0.8 * r.yz) * cell;
            let d = length(p - center);
            let magnitude = pow(s.x, 10.0);
            let radius = 0.45 + 1.4 * magnitude;
            let twinkle = 1.0 + 0.3 * magnitude * sin(params.time * (1.5 + 3.0 * s.y) + 6.2831853 * s.z);
            let tint = mix(vec3<f32>(0.68, 0.78, 1.0), vec3<f32>(1.0, 0.88, 0.72), s.y);
            light += tint * (0.1 + 2.2 * magnitude) * brightness * twinkle * exp(-d * d / (radius * radius));
        }
    }
    return light;
}

// Smooth noise in [0, 1].
fn noise(p: vec2<f32>) -> f32 {
    let i = vec2<i32>(floor(p));
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = random3(i, 7u).x;
    let b = random3(i + vec2<i32>(1, 0), 7u).x;
    let c = random3(i + vec2<i32>(0, 1), 7u).x;
    let d = random3(i + vec2<i32>(1, 1), 7u).x;
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// Noise over five octaves, for the Milky Way's clouds.
fn clouds(p: vec2<f32>) -> f32 {
    var sum = 0.0;
    var amplitude = 0.5;
    var q = p;
    for (var i = 0; i < 5; i += 1) {
        sum += amplitude * noise(q);
        q = q * 2.03 + vec2<f32>(17.0, 9.0);
        amplitude *= 0.5;
    }
    return sum;
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let frame = textureLoad(map_frame, vec2<i32>(position.xy), 0);
    if (frame.a >= 1.0) {
        return vec4<f32>(frame.rgb, 1.0);
    }
    let p = position.xy / params.pixel_ratio;
    let view = params.size / params.pixel_ratio;

    // The Milky Way: a band of faint light and dense faint stars.
    var sky = vec3<f32>(0.0);
    let across = dot(p - view * vec2<f32>(0.5, 0.45), normalize(vec2<f32>(-0.42, 1.0))) / view.y;
    var band = exp(-across * across / 0.02);
    if (band > 0.01) {
        band *= smoothstep(0.25, 0.75, clouds(p / 140.0));
        sky += vec3<f32>(0.42, 0.48, 0.66) * band * 0.06;
        sky += stars(p, 4.0, 0.3 * band, 3u, 0.5);
    }
    sky += stars(p, 9.0, 0.22, 0u, 0.7);
    sky += stars(p, 32.0, 0.4, 1u, 1.0);
    sky += stars(p, 110.0, 0.6, 2u, 1.5);
    // Dither, so the faint glow doesn't band.
    sky += (random3(vec2<i32>(position.xy), 9u).x - 0.5) / 255.0;

    return vec4<f32>(frame.rgb + max(sky, vec3<f32>(0.0)) * (1.0 - frame.a), 1.0);
}
