// =========================================
// =========================================
// src/character_authoring/rig/math.rs

// Column-major rigid transforms match glTF and MotionLoom's diagnostic matrices.
pub(crate) type V3 = [f32; 3];
pub(crate) type Q4 = [f32; 4];
pub(crate) type M4 = [f32; 16];
pub(crate) const ID: M4 = [
    1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
];
pub(crate) const QID: Q4 = [0., 0., 0., 1.];
pub(crate) fn add(a: V3, b: V3) -> V3 {
    std::array::from_fn(|i| a[i] + b[i])
}
pub(crate) fn sub(a: V3, b: V3) -> V3 {
    std::array::from_fn(|i| a[i] - b[i])
}
pub(crate) fn scale(a: V3, s: f32) -> V3 {
    a.map(|v| v * s)
}
pub(crate) fn dot(a: V3, b: V3) -> f32 {
    (0..3).map(|i| a[i] * b[i]).sum()
}
pub(crate) fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub(crate) fn length(v: V3) -> f32 {
    dot(v, v).sqrt()
}
pub(crate) fn unit(v: V3) -> V3 {
    scale(v, 1. / length(v).max(1e-12))
}
pub(crate) fn qnorm(q: Q4) -> Q4 {
    let n = q.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n < 1e-12 { QID } else { q.map(|x| x / n) }
}
pub(crate) fn conjugate(q: Q4) -> Q4 {
    [-q[0], -q[1], -q[2], q[3]]
}
pub(crate) fn qmul(a: Q4, b: Q4) -> Q4 {
    qnorm([
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ])
}
pub(crate) fn rotate(q: Q4, v: V3) -> V3 {
    let u = [q[0], q[1], q[2]];
    add(
        add(scale(u, 2. * dot(u, v)), scale(v, q[3] * q[3] - dot(u, u))),
        scale(cross(u, v), 2. * q[3]),
    )
}
pub(crate) fn arc(a: V3, b: V3) -> Q4 {
    let a = unit(a);
    let b = unit(b);
    let d = dot(a, b);
    if d < -0.99999 {
        let axis = unit(cross(
            a,
            if a[0].abs() < 0.8 {
                [1., 0., 0.]
            } else {
                [0., 1., 0.]
            },
        ));
        [axis[0], axis[1], axis[2], 0.]
    } else {
        let c = cross(a, b);
        qnorm([c[0], c[1], c[2], 1. + d])
    }
}
pub(crate) fn trs(p: V3, q: Q4, s: V3) -> M4 {
    let x = scale(rotate(q, [1., 0., 0.]), s[0]);
    let y = scale(rotate(q, [0., 1., 0.]), s[1]);
    let z = scale(rotate(q, [0., 0., 1.]), s[2]);
    [
        x[0], x[1], x[2], 0., y[0], y[1], y[2], 0., z[0], z[1], z[2], 0., p[0], p[1], p[2], 1.,
    ]
}
pub(crate) fn mul(a: M4, b: M4) -> M4 {
    std::array::from_fn(|i| {
        let r = i % 4;
        let c = i / 4;
        (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum()
    })
}
pub(crate) fn point(m: M4, p: V3) -> V3 {
    std::array::from_fn(|r| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r])
}
pub(crate) fn rigid_inverse(p: V3, q: Q4) -> M4 {
    let qi = conjugate(q);
    trs(rotate(qi, scale(p, -1.)), qi, [1.; 3])
}
pub(crate) fn position(m: M4) -> V3 {
    [m[12], m[13], m[14]]
}
pub(crate) fn quat(m: M4) -> Q4 {
    let x = unit([m[0], m[1], m[2]]);
    let y = unit([m[4], m[5], m[6]]);
    let z = unit([m[8], m[9], m[10]]);
    let trace = x[0] + y[1] + z[2];
    qnorm(if trace > 0. {
        let s = (trace + 1.).sqrt() * 2.;
        [
            (y[2] - z[1]) / s,
            (z[0] - x[2]) / s,
            (x[1] - y[0]) / s,
            s / 4.,
        ]
    } else if x[0] > y[1] && x[0] > z[2] {
        let s = (1. + x[0] - y[1] - z[2]).max(0.).sqrt() * 2.;
        [
            s / 4.,
            (y[0] + x[1]) / s,
            (z[0] + x[2]) / s,
            (y[2] - z[1]) / s,
        ]
    } else if y[1] > z[2] {
        let s = (1. + y[1] - x[0] - z[2]).max(0.).sqrt() * 2.;
        [
            (y[0] + x[1]) / s,
            s / 4.,
            (z[1] + y[2]) / s,
            (z[0] - x[2]) / s,
        ]
    } else {
        let s = (1. + z[2] - x[0] - y[1]).max(0.).sqrt() * 2.;
        [
            (z[0] + x[2]) / s,
            (z[1] + y[2]) / s,
            s / 4.,
            (x[1] - y[0]) / s,
        ]
    })
}
pub(crate) fn euler(q: Q4) -> V3 {
    let [x, y, z, w] = q;
    [
        (2. * (w * x + y * z)).atan2(1. - 2. * (x * x + y * y)),
        (2. * (w * y - z * x)).clamp(-1., 1.).asin(),
        (2. * (w * z + x * y)).atan2(1. - 2. * (y * y + z * z)),
    ]
    .map(f32::to_degrees)
}
pub(crate) fn distance_segment(p: V3, a: V3, b: V3) -> f32 {
    let d = sub(b, a);
    let t = (dot(sub(p, a), d) / dot(d, d).max(1e-12)).clamp(0., 1.);
    length(sub(p, add(a, scale(d, t))))
}
