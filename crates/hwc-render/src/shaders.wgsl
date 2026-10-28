// Ship meshes lit like the fixed-function pipeline, the background sphere and sprites.

struct Frame {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    sky_view_proj: mat4x4<f32>,
    global_ambient: vec4<f32>,
    light_dir: array<vec4<f32>, 2>,
    light_ambient: array<vec4<f32>, 2>,
    light_diffuse: array<vec4<f32>, 2>,
    // x: light count, y: viewport width, z: viewport height
    params: vec4<f32>,
    inv_view: mat4x4<f32>,
    // The modern ship look (all 0: the original). x: per-pixel lighting, y: highlight
    // strength, z: bump depth in texels, w: sky light strength.
    look: vec4<f32>,
    // x: tone map, y: shadows, z: ambient occlusion strength, w: shadow map texel in world
    // units.
    look2: vec4<f32>,
    // The sky's light from +X, -X, +Y, -Y, +Z and -Z.
    sky: array<vec4<f32>, 6>,
    // World to the shadow map.
    shadow_view_proj: mat4x4<f32>,
};

struct Object {
    model: mat4x4<f32>,
    normal_model: mat4x4<f32>,
    // World-space clip plane (used when params.z > 0.5).
    clip_plane: vec4<f32>,
    // x: fade alpha, y: tint enabled, z: clip enabled, w: glow exponent
    params: vec4<f32>,
    // x: glow darkening offset (-0.15 x glow), y: flash level, z: construction ghost
    tint: vec4<f32>,
    // Engine glow level per trail slot.
    engine_glow: vec4<f32>,
    // rgb: normalised team colour (glow meshes: colour, a: alpha)
    team: vec4<f32>,
};

struct Material {
    ambient: vec4<f32>,
    diffuse: vec4<f32>,
    // x: self-illuminated, y: textured
    flags: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;

@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var material_tex: texture_2d<f32>;
@group(1) @binding(2) var material_sampler: sampler;

@group(2) @binding(0) var<uniform> object: Object;

@group(3) @binding(0) var shadow_map: texture_depth_2d;
@group(3) @binding(1) var shadow_sampler: sampler_comparison;

struct ShipOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) front: vec3<f32>,
    @location(2) back: vec3<f32>,
    @location(3) world: vec3<f32>,
    // World normal and per-vertex tint offset, for per-pixel lighting.
    @location(4) normal: vec3<f32>,
    @location(5) tint: vec3<f32>,
    // Baked ambient occlusion, scaled by its strength.
    @location(6) occlusion: f32,
};

fn signed_byte(v: u32, shift: u32) -> f32 {
    let b = (v >> shift) & 0xffu;
    return f32(select(i32(b), i32(b) - 256, b >= 128u));
}

// The engine's per-vertex tint, as an offset added to the lit colour (0..1 units).
fn tint_offset(color: u32, flags: u32) -> vec3<f32> {
    if (object.params.y < 0.5) {
        return vec3<f32>(0.0);
    }
    let delta = vec3<f32>(signed_byte(color, 0u), signed_byte(color, 8u), signed_byte(color, 16u));
    if ((flags & 0x31u) != 0u) {
        var off = vec3<f32>(object.tint.x);
        if ((flags & 0x30u) != 0u) {
            for (var n = 0u; n < 4u; n++) {
                if ((flags & (0x10u << n)) != 0u) {
                    off += object.team.rgb * object.engine_glow[n];
                }
            }
        } else if (object.tint.y > 0.0) {
            off += delta / 256.0;
        } else {
            return vec3<f32>(0.0);
        }
        // The offsets are added twice over in byte units.
        return off * 512.0 / 255.0;
    }
    if ((color & 0xffffffu) != 0u) {
        return delta / 255.0;
    }
    return vec3<f32>(0.0);
}

fn lit(n: vec3<f32>) -> vec3<f32> {
    // The unbuilt part of a hull under construction takes a blue ambient (tint.z set).
    let ambient = select(material.ambient.rgb, vec3<f32>(0.0, 0.0, 0.75), object.tint.z > 0.5);
    var c = ambient * frame.global_ambient.rgb;
    let count = i32(frame.params.x);
    for (var i = 0; i < count; i++) {
        let d = max(dot(n, frame.light_dir[i].xyz), 0.0);
        c += ambient * frame.light_ambient[i].rgb
            + material.diffuse.rgb * frame.light_diffuse[i].rgb * d;
    }
    return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
}

@vertex
fn vs_ship(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tint: vec3<u32>,
) -> ShipOut {
    var out: ShipOut;
    let world = object.model * vec4<f32>(position, 1.0);
    out.clip = frame.proj * frame.view * world;
    out.world = world.xyz;
    out.uv = uv;
    let n = normalize((object.normal_model * vec4<f32>(normal, 0.0)).xyz);
    let t = tint_offset(tint.x, tint.y);
    out.front = clamp(lit(n) + t, vec3<f32>(0.0), vec3<f32>(1.0));
    out.back = clamp(lit(-n) + t, vec3<f32>(0.0), vec3<f32>(1.0));
    out.normal = n;
    out.tint = t;
    out.occlusion = f32(tint.z) / 255.0 * frame.look2.z;
    return out;
}

// Depth only, from the main light.
@vertex
fn vs_shadow(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return frame.shadow_view_proj * object.model * vec4<f32>(position, 1.0);
}

// The sky's light on a surface facing `n`, blended between the six axes.
fn sky_light(n: vec3<f32>) -> vec3<f32> {
    let q = n * n;
    return q.x * select(frame.sky[1].rgb, frame.sky[0].rgb, n.x >= 0.0)
        + q.y * select(frame.sky[3].rgb, frame.sky[2].rgb, n.y >= 0.0)
        + q.z * select(frame.sky[5].rgb, frame.sky[4].rgb, n.z >= 0.0);
}

// How much of the main light reaches `world` (1: all), from the shadow map with a 3x3 filter,
// fading out towards the map's edge.
fn shadow_at(world: vec3<f32>, n: vec3<f32>) -> f32 {
    if (frame.look2.y < 0.5) {
        return 1.0;
    }
    let lifted = world + n * frame.look2.w * 1.5;
    let c = frame.shadow_view_proj * vec4<f32>(lifted, 1.0);
    let ndc = c.xyz / c.w;
    let edge = max(abs(ndc.x), abs(ndc.y));
    if (edge >= 1.0 || ndc.z <= 0.0 || ndc.z >= 1.0) {
        return 1.0;
    }
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    let texel = 1.0 / vec2<f32>(textureDimensions(shadow_map));
    var lit_share = 0.0;
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            lit_share += textureSampleCompareLevel(shadow_map, shadow_sampler,
                uv + vec2<f32>(f32(i), f32(j)) * texel, ndc.z - 0.0005);
        }
    }
    return mix(lit_share / 9.0, 1.0, smoothstep(0.8, 1.0, edge));
}

// The modern look's light: the original's terms with the sky's light in the ambient, the
// occlusion taken off the ambient (and half off the diffuse), and the main light's diffuse
// shadowed. Unclamped when tone mapping.
fn lit_modern(n: vec3<f32>, shadow: f32, occlusion: f32) -> vec3<f32> {
    let ambient = select(material.ambient.rgb, vec3<f32>(0.0, 0.0, 0.75), object.tint.z > 0.5);
    var light = frame.global_ambient.rgb;
    let count = i32(frame.params.x);
    for (var i = 0; i < count; i++) {
        light += frame.light_ambient[i].rgb;
    }
    let sky = frame.look.w;
    if (sky > 0.0) {
        light = light * (1.0 - 0.35 * min(sky, 1.0)) + sky_light(n) * sky * 2.5;
    }
    var c = ambient * light * (1.0 - occlusion);
    for (var i = 0; i < count; i++) {
        let s = select(1.0, shadow, i == 0);
        let d = max(dot(n, frame.light_dir[i].xyz), 0.0) * s;
        c += material.diffuse.rgb * frame.light_diffuse[i].rgb * d * (1.0 - 0.5 * occlusion);
    }
    if (frame.look2.x > 0.5) {
        return c;
    }
    return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
}

// Rolls colours off towards white from 0.8 instead of clipping them.
fn tone(c: vec3<f32>) -> vec3<f32> {
    if (frame.look2.x < 0.5) {
        return c;
    }
    let k = 0.8;
    let over = max(c - vec3<f32>(k), vec3<f32>(0.0));
    let eased = vec3<f32>(k) + (1.0 - k) * (vec3<f32>(1.0) - exp(-over / (1.0 - k)));
    return select(c, eased, c > vec3<f32>(k));
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}

// The modern look's normal: the interpolated one, turned to the side being drawn and bumped
// by the texture's brightness (a surface-gradient bump: Mikkelsen, "Bump Mapping Unparametrized
// Surfaces on the GPU", 2010), with the height `look.z` texels per unit of brightness.
fn look_normal(in: ShipOut, front_facing: bool, dp_x: vec3<f32>, dp_y: vec3<f32>,
               duv_x: vec2<f32>, duv_y: vec2<f32>, h0: f32) -> vec3<f32> {
    var n = normalize(in.normal);
    if (!front_facing) {
        n = -n;
    }
    if (frame.look.z <= 0.0 || material.flags.y < 0.5) {
        return n;
    }
    let hx = luma(textureSampleGrad(material_tex, material_sampler, in.uv + duv_x, duv_x, duv_y).rgb);
    let hy = luma(textureSampleGrad(material_tex, material_sampler, in.uv + duv_y, duv_x, duv_y).rgb);
    let dims = vec2<f32>(textureDimensions(material_tex));
    // World size of one texel here.
    let texels = max(length(duv_x * dims) + length(duv_y * dims), 1e-6);
    let texel = (length(dp_x) + length(dp_y)) / texels;
    let k = frame.look.z * texel;
    let r1 = cross(dp_y, n);
    let r2 = cross(n, dp_x);
    let det = dot(dp_x, r1);
    let grad = sign(det) * ((hx - h0) * k * r1 + (hy - h0) * k * r2);
    let bumped = abs(det) * n - grad;
    if (dot(bumped, bumped) < 1e-20) {
        return n;
    }
    return normalize(bumped);
}

// The highlight the original never had: Blinn-Phong from each light's diffuse colour.
fn highlight(n: vec3<f32>, world: vec3<f32>, shadow: f32) -> vec3<f32> {
    let eye = (frame.inv_view * vec4<f32>(0.0, 0.0, 0.0, 1.0)).xyz;
    let v = normalize(eye - world);
    var c = vec3<f32>(0.0);
    let count = i32(frame.params.x);
    for (var i = 0; i < count; i++) {
        let l = frame.light_dir[i].xyz;
        if (dot(n, l) > 0.0) {
            let h = normalize(l + v);
            c += frame.light_diffuse[i].rgb * pow(max(dot(n, h), 0.0), 32.0) * select(1.0, shadow, i == 0);
        }
    }
    return c * frame.look.y;
}

@fragment
fn fs_ship(in: ShipOut, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
    if (object.params.z > 0.5 && dot(object.clip_plane.xyz, in.world) + object.clip_plane.w < 0.0) {
        discard;
    }
    let fade = object.params.x;
    let dp_x = dpdx(in.world);
    let dp_y = dpdy(in.world);
    let duv_x = dpdx(in.uv);
    let duv_y = dpdy(in.uv);
    let t = textureSample(material_tex, material_sampler, in.uv);
    if (frame.look.x > 0.5) {
        let h0 = luma(t.rgb);
        let n = look_normal(in, front_facing, dp_x, dp_y, duv_x, duv_y, h0);
        let geo_n = select(-normalize(in.normal), normalize(in.normal), front_facing);
        let shadow = shadow_at(in.world, geo_n);
        var shade = lit_modern(n, shadow, in.occlusion) + in.tint;
        if (frame.look2.x < 0.5) {
            shade = clamp(shade, vec3<f32>(0.0), vec3<f32>(1.0));
        }
        let spec = highlight(n, in.world, shadow) * (1.0 - in.occlusion);
        if (material.flags.y < 0.5) {
            return vec4<f32>(tone(shade + spec), material.diffuse.a * fade);
        }
        if (material.flags.x > 0.5) {
            return vec4<f32>(t.rgb, t.a * fade);
        }
        // Brighter paint shines more than dark panelling.
        let gloss = 0.35 + 0.65 * h0;
        return vec4<f32>(tone(t.rgb * shade + spec * gloss), t.a * material.diffuse.a * fade);
    }
    let shade = select(in.back, in.front, front_facing);
    if (material.flags.y < 0.5) {
        return vec4<f32>(shade, material.diffuse.a * fade);
    }
    if (material.flags.x > 0.5) {
        return vec4<f32>(t.rgb, t.a * fade);
    }
    return vec4<f32>(t.rgb * shade, t.a * material.diffuse.a * fade);
}

// ---------------------------------------------------------------------------------------------
// Engine glow meshes: additive, brightest where the surface faces the eye.

struct GlowOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) world: vec3<f32>,
};

@vertex
fn vs_glow(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>) -> GlowOut {
    var out: GlowOut;
    let world = object.model * vec4<f32>(position, 1.0);
    out.clip = frame.proj * frame.view * world;
    out.world = world.xyz;
    out.normal = (object.normal_model * vec4<f32>(normal, 0.0)).xyz;
    return out;
}

@fragment
fn fs_glow(in: GlowOut) -> @location(0) vec4<f32> {
    let eye = (frame.inv_view * vec4<f32>(0.0, 0.0, 0.0, 1.0)).xyz;
    let v = normalize(eye - in.world);
    let n = normalize(in.normal);
    let k = pow(abs(dot(n, v)), max(object.params.w, 0.01));
    return vec4<f32>(object.team.rgb * k, object.team.a);
}

// ---------------------------------------------------------------------------------------------
// Background

struct SkyOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
};

fn sky_clip(direction: vec3<f32>) -> vec4<f32> {
    var c = frame.sky_view_proj * vec4<f32>(direction, 0.0);
    // Infinitely far: keep the depth inside the clip volume whenever the point is in front.
    c.z = c.w * 0.5;
    return c;
}

@vertex
fn vs_sky(@location(0) direction: vec3<f32>, @location(1) color: vec3<f32>) -> SkyOut {
    var out: SkyOut;
    out.clip = sky_clip(direction);
    // frame.params.w: the background's colour multiplier (1 unless the plasma cloud fades it).
    out.color = color * frame.params.w;
    return out;
}

@fragment
fn fs_sky(in: SkyOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}

// ---------------------------------------------------------------------------------------------
// Sprites: background stars (pixel-sized, on the sky sphere) and nav lights (world-sized
// billboards). Both blend additively, weighted by alpha.

@group(3) @binding(0) var sprite_tex: texture_2d<f32>;
@group(3) @binding(1) var sprite_sampler: sampler;

struct SpriteIn {
    // Star: direction from the eye. Billboard: world-space centre.
    @location(0) centre: vec3<f32>,
    // Star: offset in pixels (x right, y down). Billboard: offset in world units (x right, y up).
    @location(1) corner: vec2<f32>,
    @location(2) uv: vec2<f32>,
    // rgb tint, a: alpha scale
    @location(3) color: vec4<f32>,
    // x: 0 alpha as stored, 1 alpha inverted, 2 alpha ignored; y: 1 when `plane` clips
    @location(4) mode: vec2<f32>,
    // Billboard: world-space clip plane (the ship's hyperspace window); the parts with
    // `plane . (p, 1) < 0` are cut away, like the hull.
    @location(5) plane: vec4<f32>,
};

struct SpriteOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) mode: vec2<f32>,
    // Signed distance to the clip plane (only read when mode.y is set).
    @location(3) plane_distance: f32,
};

@vertex
fn vs_star(in: SpriteIn) -> SpriteOut {
    var out: SpriteOut;
    var c = sky_clip(in.centre);
    let px = vec2<f32>(2.0 / frame.params.y, -2.0 / frame.params.z);
    c = vec4<f32>(c.xy + in.corner * px * c.w, c.z, c.w);
    out.clip = c;
    out.uv = in.uv;
    out.color = vec4<f32>(in.color.rgb * frame.params.w, in.color.a);
    out.mode = in.mode;
    out.plane_distance = 0.0;
    return out;
}

@vertex
fn vs_billboard(in: SpriteIn) -> SpriteOut {
    var out: SpriteOut;
    var v = frame.view * vec4<f32>(in.centre, 1.0);
    v = vec4<f32>(v.xy + in.corner, v.z, v.w);
    out.clip = frame.proj * v;
    out.uv = in.uv;
    out.color = in.color;
    out.mode = in.mode;
    // The corner back in world space: the plane cuts the quad as a user clip plane would.
    let world = (frame.inv_view * v).xyz;
    out.plane_distance = dot(in.plane.xyz, world) + in.plane.w;
    return out;
}

@fragment
fn fs_sprite(in: SpriteOut) -> @location(0) vec4<f32> {
    if (in.mode.y > 0.5 && in.plane_distance < 0.0) {
        discard;
    }
    let t = textureSample(sprite_tex, sprite_sampler, in.uv);
    var a = t.a;
    if (in.mode.x > 1.5) {
        a = 1.0;
    } else if (in.mode.x > 0.5) {
        a = 1.0 - t.a;
    }
    return vec4<f32>(t.rgb * in.color.rgb, a * in.color.a);
}

// ---------------------------------------------------------------------------------------------
// Effects: world-space coloured triangles (sprites, ribbons, strips), screen-width lines and
// screen-space quads. Texture alpha multiplies the vertex alpha.

struct EffectOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
};

@vertex
fn vs_effect(
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) uv: vec2<f32>,
) -> EffectOut {
    var out: EffectOut;
    out.clip = frame.proj * frame.view * vec4<f32>(position, 1.0);
    out.color = color;
    out.uv = uv;
    return out;
}

@vertex
fn vs_screen(
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) uv: vec2<f32>,
) -> EffectOut {
    var out: EffectOut;
    out.clip = vec4<f32>(position.xy, 0.0, 1.0);
    out.color = color;
    out.uv = uv;
    return out;
}

@fragment
fn fs_effect(in: EffectOut) -> @location(0) vec4<f32> {
    let t = textureSample(sprite_tex, sprite_sampler, in.uv);
    return vec4<f32>(t.rgb * in.color.rgb, t.a * in.color.a);
}

// Pixel triangles: a 2D layer blended like a CPU canvas (8-bit alpha, truncated).
@fragment
fn fs_pixel(in: EffectOut) -> @location(0) vec4<f32> {
    let a = floor(clamp(in.color.a, 0.0, 1.0) * 255.0 + 1e-3) / 255.0;
    return vec4<f32>(in.color.rgb, a);
}

// Lines: both end points come with every vertex; the vertex is pushed sideways in pixels.
@vertex
fn vs_line(
    @location(0) a: vec3<f32>,
    @location(1) b: vec3<f32>,
    @location(2) color: vec4<f32>,
    // x: 0 at a, 1 at b; y: side (-1 or 1); z: width in pixels
    @location(3) shape: vec3<f32>,
) -> EffectOut {
    var out: EffectOut;
    let vp = frame.proj * frame.view;
    let ca = vp * vec4<f32>(a, 1.0);
    let cb = vp * vec4<f32>(b, 1.0);
    let here = select(ca, cb, shape.x > 0.5);
    let px = vec2<f32>(frame.params.y, frame.params.z) * 0.5;
    let sa = ca.xy / max(ca.w, 1e-4) * px;
    let sb = cb.xy / max(cb.w, 1e-4) * px;
    var d = sb - sa;
    if (length(d) < 1e-5) {
        d = vec2<f32>(1.0, 0.0);
    }
    let perp = normalize(vec2<f32>(-d.y, d.x));
    let offset = perp * shape.y * shape.z * 0.5 / px * here.w;
    out.clip = vec4<f32>(here.xy + offset, here.z, here.w);
    out.color = color;
    out.uv = vec2<f32>(0.5, 0.5);
    return out;
}
