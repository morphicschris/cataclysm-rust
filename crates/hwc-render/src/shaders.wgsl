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
};

struct Object {
    model: mat4x4<f32>,
    normal_model: mat4x4<f32>,
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

struct ShipOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) front: vec3<f32>,
    @location(2) back: vec3<f32>,
};

fn lit(n: vec3<f32>) -> vec3<f32> {
    var c = material.ambient.rgb * frame.global_ambient.rgb;
    let count = i32(frame.params.x);
    for (var i = 0; i < count; i++) {
        let d = max(dot(n, frame.light_dir[i].xyz), 0.0);
        c += material.ambient.rgb * frame.light_ambient[i].rgb
            + material.diffuse.rgb * frame.light_diffuse[i].rgb * d;
    }
    return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
}

@vertex
fn vs_ship(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
) -> ShipOut {
    var out: ShipOut;
    let world = object.model * vec4<f32>(position, 1.0);
    out.clip = frame.proj * frame.view * world;
    out.uv = uv;
    let n = normalize((object.normal_model * vec4<f32>(normal, 0.0)).xyz);
    out.front = lit(n);
    out.back = lit(-n);
    return out;
}

@fragment
fn fs_ship(in: ShipOut, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
    let shade = select(in.back, in.front, front_facing);
    if (material.flags.y < 0.5) {
        return vec4<f32>(shade, material.diffuse.a);
    }
    let t = textureSample(material_tex, material_sampler, in.uv);
    if (material.flags.x > 0.5) {
        return t;
    }
    return vec4<f32>(t.rgb * shade, t.a * material.diffuse.a);
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
    out.color = color;
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
    // x: 0 alpha as stored, 1 alpha inverted, 2 alpha ignored
    @location(4) mode: vec2<f32>,
};

struct SpriteOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) mode: vec2<f32>,
};

@vertex
fn vs_star(in: SpriteIn) -> SpriteOut {
    var out: SpriteOut;
    var c = sky_clip(in.centre);
    let px = vec2<f32>(2.0 / frame.params.y, -2.0 / frame.params.z);
    c = vec4<f32>(c.xy + in.corner * px * c.w, c.z, c.w);
    out.clip = c;
    out.uv = in.uv;
    out.color = in.color;
    out.mode = in.mode;
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
    return out;
}

@fragment
fn fs_sprite(in: SpriteOut) -> @location(0) vec4<f32> {
    let t = textureSample(sprite_tex, sprite_sampler, in.uv);
    var a = t.a;
    if (in.mode.x > 1.5) {
        a = 1.0;
    } else if (in.mode.x > 0.5) {
        a = 1.0 - t.a;
    }
    return vec4<f32>(t.rgb * in.color.rgb, a * in.color.a);
}
