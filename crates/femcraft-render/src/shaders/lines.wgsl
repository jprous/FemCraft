struct Uniforms {
    view_proj: mat4x4<f32>,
    // x, y: target size in pixels; z: line width in pixels.
    viewport: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> u: Uniforms;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(@location(0) pos: vec3<f32>, @location(1) color: vec4<f32>) -> VsOut {
    var out: VsOut;
    out.pos = u.view_proj * vec4<f32>(pos, 1.0);
    out.color = color;
    return out;
}

// Lines are drawn as screen-space quads (one instance per segment, 6 vertices) so their width is
// independent of the GPU's fixed 1 px hardware line.
@vertex
fn vs_line(
    @builtin(vertex_index) vi: u32,
    @location(0) pa: vec3<f32>,
    @location(1) ca: vec4<f32>,
    @location(2) wa: f32,
    @location(3) pb: vec3<f32>,
    @location(4) cb: vec4<f32>,
    @location(5) wb: f32,
) -> VsOut {
    var a = u.view_proj * vec4<f32>(pa, 1.0);
    var b = u.view_proj * vec4<f32>(pb, 1.0);
    let eps = 0.0001;
    // Clip against the near side (w > eps) so a segment crossing behind the camera stays sane.
    if (a.w < eps && b.w < eps) {
        var dead: VsOut;
        dead.pos = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        dead.color = ca;
        return dead;
    }
    if (a.w < eps) {
        let t = (eps - a.w) / (b.w - a.w);
        a = a + (b - a) * t;
    } else if (b.w < eps) {
        let t = (eps - b.w) / (a.w - b.w);
        b = b + (a - b) * t;
    }
    let size = u.viewport.xy;
    let na = a.xy / a.w;
    let nb = b.xy / b.w;
    var d = (nb - na) * size;
    let len = length(d);
    if (len > 0.0001) {
        d = d / len;
    } else {
        d = vec2<f32>(1.0, 0.0);
    }
    let n = vec2<f32>(-d.y, d.x);
    var ends = array<u32, 6>(0u, 0u, 1u, 1u, 1u, 0u);
    var sides = array<f32, 6>(-1.0, 1.0, -1.0, -1.0, 1.0, 1.0);
    let end = ends[vi];
    let side = sides[vi];
    // Half width plus half a pixel of extension along the line so endpoints are covered.
    // A zero-length segment is a grid dot: drawn as a small square.
    let dot = len < 0.5;
    var half_w = u.viewport.z * 0.5 * wa;
    var ext = 0.5;
    if (dot) {
        half_w = 1.0;
        ext = 1.0;
    }
    var out: VsOut;
    var c = a;
    out.color = ca;
    var dir_sign = -1.0;
    if (end == 1u) {
        c = b;
        out.color = cb;
        dir_sign = 1.0;
    }
    let off_px = n * side * half_w + d * dir_sign * ext;
    let ndc = c.xy / c.w + off_px * 2.0 / size;
    out.pos = vec4<f32>(ndc * c.w, c.z, c.w);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
