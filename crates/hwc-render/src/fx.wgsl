// Particle effects drawn the way the engine's immediate-mode OpenGL path drew them: one
// textured, vertex-coloured triangle list with the GL state (blend, depth write, culling)
// chosen per draw. World-space vertices may carry a pixel offset (wide lines and points);
// screen-space vertices (lens flares) are already in normalised device coordinates.

struct FxFrame {
    view_proj: mat4x4<f32>,
    // xy: viewport size in pixels
    viewport: vec4<f32>,
};

@group(0) @binding(0) var<uniform> fx: FxFrame;
@group(1) @binding(0) var fx_tex: texture_2d<f32>;
@group(1) @binding(1) var fx_sampler: sampler;

struct FxOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
};

@vertex
fn vs_fx_world(
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) offset: vec2<f32>,
) -> FxOut {
    var out: FxOut;
    var clip = fx.view_proj * vec4<f32>(position, 1.0);
    let px = 2.0 / max(fx.viewport.xy, vec2<f32>(1.0, 1.0));
    clip = vec4<f32>(clip.xy + offset * px * clip.w, clip.z, clip.w);
    out.clip = clip;
    out.color = color;
    out.uv = uv;
    return out;
}

@vertex
fn vs_fx_screen(
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) offset: vec2<f32>,
) -> FxOut {
    var out: FxOut;
    out.clip = vec4<f32>(position.xy, 0.0, 1.0);
    out.color = color;
    out.uv = uv;
    return out;
}

// GL_MODULATE.
@fragment
fn fs_fx(in: FxOut) -> @location(0) vec4<f32> {
    let t = textureSample(fx_tex, fx_sampler, in.uv);
    return t * in.color;
}
