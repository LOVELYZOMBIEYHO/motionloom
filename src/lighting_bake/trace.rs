//! Deterministic CPU BVH and bounded diffuse transport, independent of cameras.
use super::*;
use crate::experimental::geometry::ResolvedMesh;
use crate::world::gltf_loader::{GlbAlphaMode, GlbTextureData};
use crate::world::{WorldLight, WorldLightKind};
use std::f32::consts::{PI, TAU};

pub(super) type V = [f32; 3];
pub(super) fn add(a: V, b: V) -> V {
    std::array::from_fn(|k| a[k] + b[k])
}
pub(super) fn sub(a: V, b: V) -> V {
    std::array::from_fn(|k| a[k] - b[k])
}
pub(super) fn mul(a: V, b: V) -> V {
    std::array::from_fn(|k| a[k] * b[k])
}
pub(super) fn scale(a: V, b: f32) -> V {
    a.map(|v| v * b)
}
pub(super) fn dot(a: V, b: V) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub(super) fn cross(a: V, b: V) -> V {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub(super) fn normalize(a: V) -> V {
    scale(a, dot(a, a).sqrt().max(1e-20).recip())
}
pub(super) fn luminance(a: V) -> f32 {
    dot(a, [0.2126, 0.7152, 0.0722])
}
pub(super) fn basis(n: V) -> (V, V) {
    let t = normalize(cross(
        if n[1].abs() < 0.95 {
            [0., 1., 0.]
        } else {
            [1., 0., 0.]
        },
        n,
    ));
    (t, cross(n, t))
}
use crate::lighting_ibl::{lambert_sh_factor, sh_basis};

pub(super) struct Rng(u64);
impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x9e3779b97f4a7c15)
    }
    pub fn unit(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        ((z ^ (z >> 31)) >> 40) as f32 / 16777216.
    }
    fn cosine(&mut self, n: V) -> V {
        let r = self.unit().sqrt();
        let p = TAU * self.unit();
        let (t, b) = basis(n);
        normalize(add(
            add(scale(t, r * p.cos()), scale(b, r * p.sin())),
            scale(n, (1. - r * r).sqrt()),
        ))
    }
}

#[derive(Clone)]
struct Triangle {
    p: V,
    e1: V,
    e2: V,
    normal: V,
    min: V,
    max: V,
    mesh: usize,
    indices: [usize; 3],
}
#[derive(Clone)]
struct BvhNode {
    min: V,
    max: V,
    start: usize,
    end: usize,
    left: usize,
    right: usize,
}
#[derive(Clone, Copy)]
struct Hit {
    distance: f32,
    triangle: usize,
    u: f32,
    v: f32,
}
struct Surface {
    normal: V,
    base: V,
    emission: V,
    transmission: V,
    alpha: f32,
    backface: bool,
}

pub(super) struct TraceScene {
    pub meshes: Vec<ResolvedMesh>,
    triangles: Vec<Triangle>,
    nodes: Vec<BvhNode>,
    pub lights: Vec<WorldLight>,
    pub environment: Option<Environment>,
    pub max_distance: f32,
}

pub(super) struct Environment {
    pub map: crate::lighting_ibl::LinearEnvironment,
    pub intensity: f32,
    pub diffuse: f32,
    pub specular: f32,
    pub rotation: f32,
}
impl Environment {
    pub fn sample(&self, d: V, reflection: bool) -> V {
        let (sin, cos) = self.rotation.sin_cos();
        let direction = [d[0] * cos - d[2] * sin, d[1], d[0] * sin + d[2] * cos];
        scale(
            self.map.sample_latlong(direction),
            self.intensity
                * if reflection {
                    self.specular
                } else {
                    self.diffuse
                },
        )
    }
}

impl TraceScene {
    pub fn new(
        meshes: Vec<ResolvedMesh>,
        lights: Vec<WorldLight>,
        environment: Option<Environment>,
        max_distance: f32,
    ) -> Result<Self, LightingBakeError> {
        let count: usize = meshes.iter().map(|m| m.indices.len() / 3).sum();
        if count > 2_000_000 {
            return Err(LightingBakeError::Limit(
                "maximum 2,000,000 triangles".into(),
            ));
        }
        let mut triangles = Vec::with_capacity(count);
        for (mesh, m) in meshes.iter().enumerate() {
            if m.indices.len() % 3 != 0
                || m.positions
                    .iter()
                    .flatten()
                    .chain(m.uvs.iter().flatten())
                    .chain(m.normals.iter().flatten())
                    .chain(m.colors.iter().flatten())
                    .chain(m.tangents.iter().flatten())
                    .any(|v| !v.is_finite())
            {
                return Err(LightingBakeError::Invalid(
                    "non-finite/incomplete geometry".into(),
                ));
            }
            for idx in m.indices.chunks_exact(3) {
                let indices = [idx[0] as usize, idx[1] as usize, idx[2] as usize];
                if indices.iter().any(|&i| {
                    i >= m.positions.len()
                        || i >= m.normals.len()
                        || i >= m.colors.len()
                        || i >= m.uvs.len()
                        || i >= m.tangents.len()
                }) {
                    return Err(LightingBakeError::Invalid("out-of-range mesh index".into()));
                }
                let p = m.positions[indices[0]];
                let b = m.positions[indices[1]];
                let c = m.positions[indices[2]];
                let e1 = sub(b, p);
                let e2 = sub(c, p);
                let normal = normalize(cross(e1, e2));
                if dot(cross(e1, e2), cross(e1, e2)) < 1e-18 {
                    continue;
                }
                triangles.push(Triangle {
                    p,
                    e1,
                    e2,
                    normal,
                    min: std::array::from_fn(|k| p[k].min(b[k]).min(c[k])),
                    max: std::array::from_fn(|k| p[k].max(b[k]).max(c[k])),
                    mesh,
                    indices,
                });
            }
        }
        let mut scene = Self {
            meshes,
            triangles,
            nodes: Vec::new(),
            lights,
            environment,
            max_distance,
        };
        if !scene.triangles.is_empty() {
            scene.build(0, scene.triangles.len());
        }
        Ok(scene)
    }
    fn build(&mut self, start: usize, end: usize) -> usize {
        let min = std::array::from_fn(|k| {
            self.triangles[start..end]
                .iter()
                .map(|t| t.min[k])
                .fold(f32::INFINITY, f32::min)
        });
        let max = std::array::from_fn(|k| {
            self.triangles[start..end]
                .iter()
                .map(|t| t.max[k])
                .fold(f32::NEG_INFINITY, f32::max)
        });
        let node = self.nodes.len();
        self.nodes.push(BvhNode {
            min,
            max,
            start,
            end,
            left: 0,
            right: 0,
        });
        if end - start > 8 {
            let axis = (0..3)
                .max_by(|&a, &b| (max[a] - min[a]).total_cmp(&(max[b] - min[b])))
                .unwrap();
            let middle = start + (end - start) / 2;
            self.triangles[start..end].select_nth_unstable_by(middle - start, |a, b| {
                (a.min[axis] + a.max[axis]).total_cmp(&(b.min[axis] + b.max[axis]))
            });
            let left = self.build(start, middle);
            let right = self.build(middle, end);
            self.nodes[node].left = left;
            self.nodes[node].right = right;
        }
        node
    }
    fn aabb(n: &BvhNode, o: V, d: V, limit: f32) -> bool {
        let mut near: f32 = 0.;
        let mut far = limit;
        for k in 0..3 {
            if d[k].abs() < 1e-12 {
                if o[k] < n.min[k] || o[k] > n.max[k] {
                    return false;
                }
                continue;
            }
            let a = (n.min[k] - o[k]) / d[k];
            let b = (n.max[k] - o[k]) / d[k];
            near = near.max(a.min(b));
            far = far.min(a.max(b));
            if far < near {
                return false;
            }
        }
        true
    }
    fn nearest(&self, o: V, d: V, limit: f32) -> Option<Hit> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut stack = [0usize; 64];
        let mut len = 1;
        let mut best = None;
        let mut distance = limit;
        while len > 0 {
            len -= 1;
            let n = &self.nodes[stack[len]];
            if !Self::aabb(n, o, d, distance) {
                continue;
            }
            if n.left != 0 {
                stack[len] = n.left;
                stack[len + 1] = n.right;
                len += 2;
                continue;
            }
            for index in n.start..n.end {
                let t = &self.triangles[index];
                let p = cross(d, t.e2);
                let determinant = dot(t.e1, p);
                if determinant.abs() < 1e-10 {
                    continue;
                }
                let inv = 1. / determinant;
                let s = sub(o, t.p);
                let u = dot(s, p) * inv;
                if !(0. ..=1.).contains(&u) {
                    continue;
                }
                let q = cross(s, t.e1);
                let v = dot(d, q) * inv;
                if v < 0. || u + v > 1. {
                    continue;
                }
                let dst = dot(t.e2, q) * inv;
                if dst > 0.0001 && dst < distance {
                    distance = dst;
                    best = Some(Hit {
                        distance: dst,
                        triangle: index,
                        u,
                        v,
                    });
                }
            }
        }
        best
    }
    fn tex(t: &GlbTextureData, uv: [f32; 2], srgb: bool) -> [f32; 4] {
        if t.width == 0 || t.height == 0 || t.rgba.len() < t.width as usize * t.height as usize * 4
        {
            return [1.; 4];
        }
        let fx = uv[0].rem_euclid(1.) * t.width as f32 - 0.5;
        let fy = uv[1].rem_euclid(1.) * t.height as f32 - 0.5;
        let x = fx.floor() as i32;
        let y = fy.floor() as i32;
        let tx = fx - x as f32;
        let ty = fy - y as f32;
        let sample = |x: i32, y: i32, k: usize| {
            let i = ((y.rem_euclid(t.height as i32) as u32 * t.width
                + x.rem_euclid(t.width as i32) as u32)
                * 4) as usize
                + k;
            let v = t.rgba[i] as f32 / 255.;
            if srgb && k < 3 {
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            } else {
                v
            }
        };
        std::array::from_fn(|k| {
            (sample(x, y, k) * (1. - tx) + sample(x + 1, y, k) * tx) * (1. - ty)
                + (sample(x, y + 1, k) * (1. - tx) + sample(x + 1, y + 1, k) * tx) * ty
        })
    }
    fn surface(&self, h: Hit, d: V) -> Surface {
        let t = &self.triangles[h.triangle];
        let mesh = &self.meshes[t.mesh];
        let m = &mesh.material;
        let w = [1. - h.u - h.v, h.u, h.v];
        let lerp3 = |values: &[V]| {
            std::array::from_fn(|k| (0..3).map(|j| values[t.indices[j]][k] * w[j]).sum::<f32>())
        };
        let uv = std::array::from_fn(|k| (0..3).map(|j| mesh.uvs[t.indices[j]][k] * w[j]).sum());
        let color: [f32; 4] = std::array::from_fn(|k| {
            (0..3)
                .map(|j| mesh.colors[t.indices[j]][k] * w[j])
                .sum::<f32>()
                * m.base_color_factor[k]
        });
        let albedo = mesh
            .textures
            .first()
            .map_or([1.; 4], |t| Self::tex(t, uv, true));
        let mut base = std::array::from_fn(|k| (color[k] * albedo[k]).clamp(0., 0.98));
        let metallic_sample = if let Some(tex) = mesh.textures.get(2) {
            let p = Self::tex(tex, uv, false);
            let channel = match m.metallic_channel {
                crate::MaterialTextureChannel::R => p[0],
                crate::MaterialTextureChannel::G => p[1],
                crate::MaterialTextureChannel::B => p[2],
                crate::MaterialTextureChannel::A => p[3],
                crate::MaterialTextureChannel::Luminance => luminance([p[0], p[1], p[2]]),
            };
            let v = if m.metallic_invert {
                1. - channel
            } else {
                channel
            };
            v
        } else {
            1.
        };
        base = scale(
            base,
            1. - (m.metallic_factor * metallic_sample).clamp(0., 1.),
        );
        let backface = dot(t.normal, d) > 0.;
        let mut normal = normalize(lerp3(&mesh.normals));
        if dot(normal, d) > 0. {
            normal = scale(normal, -1.);
        }
        let coat_cosine = dot(normal, scale(d, -1.)).abs().clamp(0., 1.);
        if let Some(tex) = mesh.textures.get(1) {
            let n = Self::tex(tex, uv, false);
            let tangent: V = std::array::from_fn(|k| {
                (0..3).map(|j| mesh.tangents[t.indices[j]][k] * w[j]).sum()
            });
            let sign = (0..3)
                .map(|j| mesh.tangents[t.indices[j]][3] * w[j])
                .sum::<f32>()
                .signum();
            let tangent = normalize(sub(tangent, scale(normal, dot(normal, tangent))));
            let b = scale(cross(normal, tangent), sign);
            let mapped = normalize(add(
                add(
                    scale(tangent, (2. * n[0] - 1.) * m.normal_scale),
                    scale(b, (2. * n[1] - 1.) * m.normal_scale),
                ),
                scale(normal, 2. * n[2] - 1.),
            ));
            if dot(mapped, normal) > 0.05 {
                normal = mapped;
            }
        }
        let emission_texture = mesh
            .textures
            .get(3)
            .map_or([1.; 4], |t| Self::tex(t, uv, true));
        let layer_cosine = dot(normal, scale(d, -1.)).abs().clamp(0., 1.);
        let sheen_color = m
            .sheen_color
            .map(|v| if m.unlit { 0. } else { v * m.sheen });
        base = scale(
            base,
            crate::material_layers::layer_base_scale(
                sheen_color,
                m.sheen_roughness,
                0.0,
                layer_cosine,
            ),
        );
        let coat_base = if m.unlit {
            1.
        } else {
            1. - m.clearcoat * crate::material_layers::coat_fresnel(coat_cosine)
        };
        base = scale(base, coat_base);
        let mut emission = std::array::from_fn(|k| {
            m.emissive_factor[k] * m.emissive_strength * emission_texture[k] * coat_base
        });
        if m.unlit {
            emission = add(emission, base);
            base = [0.; 3];
        }
        let cosine = dot(normal, scale(d, -1.)).abs().max(0.05);
        let f0 = ((m.ior - 1.) / (m.ior + 1.)).powi(2);
        let fresnel = f0 + (1. - f0) * (1. - cosine).powi(5);
        let distance = m.thickness_factor.max(0.) / cosine;
        let transmission = std::array::from_fn(|k| {
            m.transmission_factor.clamp(0., 1.)
                * coat_base
                * (1. - fresnel)
                * m.attenuation_color[k]
                    .clamp(1e-5, 1.)
                    .powf(distance / m.attenuation_distance.max(1e-5))
        });
        let alpha = if m.alpha_mode == GlbAlphaMode::Opaque {
            1.
        } else {
            color[3] * albedo[3]
        };
        let alpha = if m.alpha_mode == GlbAlphaMode::Mask {
            if alpha < m.alpha_cutoff { 0. } else { 1. }
        } else {
            alpha.clamp(0., 1.)
        };
        Surface {
            normal,
            base,
            emission,
            transmission,
            alpha,
            backface,
        }
    }
    /// Straight thin-sheet transmission: no refractive caustic/solid glass claim.
    pub fn visibility(&self, mut o: V, d: V, limit: f32) -> (V, f32) {
        let mut through = [1.; 3];
        let mut traveled = 0.;
        for _ in 0..32 {
            let Some(h) = self.nearest(o, d, limit - traveled) else {
                return (through, limit);
            };
            let s = self.surface(h, d);
            through = mul(
                through,
                add(scale(s.transmission, s.alpha), [1. - s.alpha; 3]),
            );
            if luminance(through) < 0.001 {
                return ([0.; 3], traveled + h.distance);
            }
            let delta = h.distance + 0.001;
            traveled += delta;
            o = add(o, scale(d, delta));
            if traveled >= limit {
                return (through, limit);
            }
        }
        ([0.; 3], traveled.min(limit))
    }
    fn direct(&self, p: V, n: V, rng: &mut Rng, terminal: bool) -> V {
        let mut result = [0.; 3];
        for light in &self.lights {
            let (direction, distance, energy) = match light.kind {
                WorldLightKind::Directional => {
                    let mut direction = normalize(scale(light.direction, -1.));
                    if light.angular_diameter > 0. {
                        // Uniform projected solar disk, preserving total source
                        // intensity. Zero diameter consumes no additional RNG.
                        let radius =
                            rng.unit().sqrt() * (0.5 * light.angular_diameter.to_radians()).tan();
                        let angle = TAU * rng.unit();
                        let (t, b) = basis(direction);
                        direction = normalize(add(
                            direction,
                            add(
                                scale(t, radius * angle.cos()),
                                scale(b, radius * angle.sin()),
                            ),
                        ));
                    }
                    (direction, self.max_distance, light.intensity)
                }
                WorldLightKind::RectArea => {
                    let normal = normalize(light.direction);
                    let (t, b) = basis(normal);
                    let q = add(
                        light.position,
                        add(
                            scale(t, (rng.unit() - 0.5) * light.width),
                            scale(b, (rng.unit() - 0.5) * light.height),
                        ),
                    );
                    let delta = sub(q, p);
                    let dst = dot(delta, delta).sqrt().max(0.01);
                    let d = scale(delta, 1. / dst);
                    let cosine = dot(normal, scale(d, -1.)).max(0.);
                    let cutoff = if light.range > 0.001 {
                        (1. - (dst / light.range).powi(4)).clamp(0., 1.).powi(2)
                    } else {
                        1.
                    };
                    (
                        d,
                        dst,
                        light.intensity * light.width * light.height * cosine * cutoff
                            / (dst * dst).max(1e-8),
                    )
                }
                _ => {
                    let mut delta = sub(light.position, p);
                    if light.source_radius > 0. {
                        // A spherical finite source projects to a disk normal
                        // to the receiver direction; average fixed total power.
                        let (t, b) = basis(normalize(delta));
                        let radius = light.source_radius * rng.unit().sqrt();
                        let angle = TAU * rng.unit();
                        delta = add(
                            delta,
                            add(
                                scale(t, radius * angle.cos()),
                                scale(b, radius * angle.sin()),
                            ),
                        );
                    }
                    let dst = dot(delta, delta).sqrt().max(0.01);
                    let d = scale(delta, 1. / dst);
                    let cutoff = (1. - (dst / light.range.max(0.001)).powi(4))
                        .clamp(0., 1.)
                        .powi(2);
                    // Match the renderer's inverse-square/range conventions.
                    let mut energy = light.intensity * cutoff / (dst * dst).max(0.25);
                    if light.kind == WorldLightKind::Spot {
                        let angle = dot(normalize(light.direction), scale(d, -1.));
                        let outer = light.outer_cone_degrees.to_radians().cos();
                        let inner = light.inner_cone_degrees.to_radians().cos();
                        let cone = ((angle - outer) / (inner - outer).max(0.001)).clamp(0., 1.);
                        energy *= cone * cone * (3. - 2. * cone);
                    }
                    (d, dst, energy)
                }
            };
            let cosine = dot(n, direction).max(0.);
            if cosine <= 0. || energy <= 0. {
                continue;
            }
            // Baking traces actual occlusion for every source, independently of
            // preview castShadow flags and analytic shadow-strength shortcuts.
            let (visible, _) = self.visibility(
                add(p, scale(n, 0.001)),
                direction,
                (distance - 0.002).max(0.001),
            );
            result = add(result, scale(mul(light.color, visible), energy * cosine));
        }
        // At the last bounce only, integrate the environment explicitly. At
        // earlier bounces it is reached by the continuation ray; doing both
        // would count the same environment energy twice without MIS.
        if terminal && let Some(environment) = &self.environment {
            let d = rng.cosine(n);
            let (visible, _) = self.visibility(add(p, scale(n, 0.001)), d, self.max_distance);
            result = add(
                result,
                scale(mul(visible, environment.sample(d, false)), PI),
            );
        }
        result
    }
    pub fn radiance(&self, mut o: V, mut d: V, bounces: u32, rng: &mut Rng, reflection: bool) -> V {
        let mut throughput = [1.; 3];
        let mut output = [0.; 3];
        let mut bounced = 0;
        let mut sheets = 0;
        loop {
            let Some(h) = self.nearest(o, d, self.max_distance) else {
                if let Some(e) = &self.environment {
                    output = add(
                        output,
                        mul(throughput, e.sample(d, reflection && bounced == 0)),
                    );
                }
                break;
            };
            let s = self.surface(h, d);
            let p = add(o, scale(d, h.distance));
            // Alpha is surface coverage. Stochastic coverage preserves both
            // covered diffuse radiance and uncovered transmission in expectation.
            if s.alpha < 0.999 && rng.unit() >= s.alpha {
                o = add(p, scale(d, 0.001));
                sheets += 1;
                if sheets >= 32 {
                    break;
                }
                continue;
            }
            if luminance(s.transmission) > 0.001 {
                throughput = mul(throughput, s.transmission);
                o = add(p, scale(d, 0.001));
                sheets += 1;
                if sheets >= 32 {
                    break;
                }
                continue;
            }
            output = add(output, mul(throughput, s.emission));
            let direct = self.direct(p, s.normal, rng, bounced + 1 >= bounces);
            output = add(output, mul(throughput, scale(mul(s.base, direct), 1. / PI)));
            bounced += 1;
            if bounced >= bounces {
                break;
            }
            throughput = mul(throughput, s.base);
            if luminance(throughput) < 1e-5 {
                break;
            }
            d = rng.cosine(s.normal);
            o = add(p, scale(s.normal, 0.001));
        }
        output.map(|v| if v.is_finite() { v.max(0.) } else { 0. })
    }
    pub fn valid_probe(&self, p: V) -> bool {
        let mut backs = 0;
        let mut hits = 0;
        for axis in 0..3 {
            for sign in [-1., 1.] {
                let mut d = [0.; 3];
                d[axis] = sign;
                if let Some(h) = self.nearest(p, d, self.max_distance) {
                    let s = self.surface(h, d);
                    if luminance(s.transmission) < 0.01 && s.alpha > 0.9 {
                        if h.distance < 0.025 {
                            return false;
                        }
                        hits += 1;
                        if s.backface {
                            backs += 1;
                        }
                    }
                }
            }
        }
        !(hits >= 4 && backs == hits)
    }
}

pub(super) fn oct_decode(uv: [f32; 2]) -> V {
    let mut d = [uv[0] * 2. - 1., uv[1] * 2. - 1., 0.];
    d[2] = 1. - d[0].abs() - d[1].abs();
    if d[2] < 0. {
        let x = d[0];
        d[0] = (1. - d[1].abs()) * x.signum();
        d[1] = (1. - x.abs()) * d[1].signum();
    }
    normalize(d)
}

pub(super) fn bake_probe(
    scene: &TraceScene,
    p: V,
    options: &LightingBakeOptions,
    seed: u64,
) -> IrradianceProbe {
    let valid = scene.valid_probe(p);
    let mut sh = [[0.; 3]; 9];
    let mut rng = Rng::new(seed);
    if valid {
        let phase = rng.unit();
        for i in 0..options.rays_per_probe {
            let z = 1. - 2. * (i as f32 + 0.5) / options.rays_per_probe as f32;
            let phi = TAU * (i as f32 * 0.618034 + phase).fract();
            let r = (1. - z * z).sqrt();
            let d = [r * phi.cos(), r * phi.sin(), z];
            let value = scene.radiance(p, d, options.max_bounces, &mut rng, false);
            let basis = sh_basis(d);
            for k in 0..9 {
                sh[k] = add(
                    sh[k],
                    scale(value, basis[k] * 4. * PI / options.rays_per_probe as f32),
                );
            }
        }
        for k in 0..9 {
            sh[k] = scale(sh[k], lambert_sh_factor(k));
        }
    }
    let mut depth_moments = Vec::with_capacity(64);
    let mut visibility = Vec::with_capacity(64);
    for y in 0..8 {
        for x in 0..8 {
            let mut moments = [0.; 2];
            let mut vis = 0.;
            for _ in 0..4 {
                let d = oct_decode([
                    (x as f32 + 0.25 + 0.5 * rng.unit()) / 8.,
                    (y as f32 + 0.25 + 0.5 * rng.unit()) / 8.,
                ]);
                let (trans, depth) = scene.visibility(p, d, options.max_ray_distance);
                moments[0] += depth * 0.25;
                moments[1] += depth * depth * 0.25;
                vis += luminance(trans) * 0.25;
            }
            depth_moments.push(moments);
            visibility.push(if valid { vis } else { 0. });
        }
    }
    IrradianceProbe {
        position: p,
        valid,
        irradiance_sh: sh,
        depth_moments,
        visibility,
    }
}

#[cfg(test)]
mod finite_emitter_tests {
    use super::*;
    use crate::world::gltf_loader::GlbMaterialData;

    fn source(kind: WorldLightKind) -> WorldLight {
        WorldLight {
            id: None,
            kind,
            position: [0., 2., 0.],
            direction: [0., -1., 0.],
            color: [1.; 3],
            intensity: 2.,
            range: 10.,
            inner_cone_degrees: 30.,
            outer_cone_degrees: 45.,
            width: 1.,
            height: 1.,
            cast_shadow: true,
            shadow_strength: 1.,
            angular_diameter: 0.,
            source_radius: 0.,
        }
    }

    #[test]
    fn zero_size_sources_preserve_legacy_energy_and_rng_sequence() {
        for kind in [
            WorldLightKind::Directional,
            WorldLightKind::Point,
            WorldLightKind::Spot,
        ] {
            let scene = TraceScene::new(vec![], vec![source(kind)], None, 100.).unwrap();
            let mut actual_rng = Rng::new(99);
            let mut legacy_rng = Rng::new(99);
            let result = scene.direct([0.; 3], [0., 1., 0.], &mut actual_rng, false);
            let legacy = if kind == WorldLightKind::Directional {
                2.
            } else {
                2. * (1.0f32 - (2.0f32 / 10.).powi(4)).powi(2) / 4.
            };
            assert_eq!(result, [legacy; 3]);
            assert_eq!(
                actual_rng.unit(),
                legacy_rng.unit(),
                "zero size must not consume a disk sample"
            );
        }
        let scene =
            TraceScene::new(vec![], vec![source(WorldLightKind::RectArea)], None, 100.).unwrap();
        let mut actual_rng = Rng::new(99);
        let mut legacy_rng = Rng::new(99);
        scene.direct([0.; 3], [0., 1., 0.], &mut actual_rng, false);
        legacy_rng.unit();
        legacy_rng.unit();
        assert_eq!(
            actual_rng.unit(),
            legacy_rng.unit(),
            "area sampling retains exactly two rectangle draws"
        );
    }

    #[test]
    fn finite_sun_point_and_spot_sources_form_a_penumbra_at_an_occluder_edge() {
        let edge = ResolvedMesh {
            name: "half-plane".into(),
            positions: vec![
                [0., 1., -10.],
                [10., 1., -10.],
                [10., 1., 10.],
                [0., 1., 10.],
            ],
            normals: vec![[0., -1., 0.]; 4],
            tangents: vec![[1., 0., 0., 1.]; 4],
            uvs: vec![[0.; 2]; 4],
            colors: vec![[1.; 4]; 4],
            indices: vec![0, 1, 2, 0, 2, 3],
            material: GlbMaterialData::default(),
            textures: vec![],
            uv_source: "fixture".into(),
        };
        for kind in [
            WorldLightKind::Directional,
            WorldLightKind::Point,
            WorldLightKind::Spot,
        ] {
            let delta =
                TraceScene::new(vec![edge.clone()], vec![source(kind)], None, 100.).unwrap();
            assert_eq!(
                delta.direct([0.; 3], [0., 1., 0.], &mut Rng::new(99), false),
                [0.; 3]
            );
            let mut light = source(kind);
            if kind == WorldLightKind::Directional {
                light.angular_diameter = 2.;
            } else {
                light.source_radius = 0.5;
            }
            let open = TraceScene::new(vec![], vec![light.clone()], None, 100.).unwrap();
            let blocked = TraceScene::new(vec![edge.clone()], vec![light], None, 100.).unwrap();
            let mut ratio = 0.;
            for seed in 0..512 {
                let full = open.direct([0.; 3], [0., 1., 0.], &mut Rng::new(seed), false)[0];
                let shaded = blocked.direct([0.; 3], [0., 1., 0.], &mut Rng::new(seed), false)[0];
                ratio += shaded / full;
            }
            ratio /= 512.;
            assert!(
                (0.35..0.65).contains(&ratio),
                "{kind:?} lit fraction {ratio}"
            );
        }
    }
}

#[cfg(test)]
mod material_layer_tests {
    use super::*;

    #[test]
    fn bake_surface_layer_loss_matches_renderer_directional_contract() {
        let mesh = ResolvedMesh {
            name: "layer-energy".into(),
            positions: vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
            normals: vec![[0., 0., 1.]; 3],
            tangents: vec![[1., 0., 0., 1.]; 3],
            uvs: vec![[0., 0.]; 3],
            colors: vec![[1.; 4]; 3],
            indices: vec![0, 1, 2],
            material: crate::world::gltf_loader::GlbMaterialData {
                base_color_factor: [0.4, 0.5, 0.6, 1.],
                metallic_factor: 0.,
                emissive_factor: [0.2; 3],
                transmission_factor: 0.8,
                ..Default::default()
            },
            textures: Vec::new(),
            uv_source: "authored".into(),
        };
        let hit = Hit {
            distance: 1.,
            triangle: 0,
            u: 0.2,
            v: 0.2,
        };
        let direction = normalize([3.0_f32.sqrt(), 0., -1.]);
        let mut scene = TraceScene::new(vec![mesh], Vec::new(), None, 50.).unwrap();
        let plain = scene.surface(hit, direction);
        let m = &mut scene.meshes[0].material;
        m.sheen = 0.6;
        m.sheen_roughness = 0.7;
        m.clearcoat = 0.5;
        let layered = scene.surface(hit, direction);
        let scale = crate::material_layers::layer_base_scale([0.6; 3], 0.7, 0.5, 0.5);
        let coat = 1. - 0.5 * crate::material_layers::coat_fresnel(0.5);
        for k in 0..3 {
            assert!((layered.base[k] - plain.base[k] * scale).abs() < 1e-6);
            assert!((layered.emission[k] - plain.emission[k] * coat).abs() < 1e-6);
            assert!((layered.transmission[k] - plain.transmission[k] * coat).abs() < 1e-6);
        }
        scene.meshes[0].material.sheen = 0.;
        scene.meshes[0].material.clearcoat = 0.;
        let neutral = scene.surface(hit, direction);
        assert_eq!(neutral.base, plain.base);
        assert_eq!(neutral.emission, plain.emission);
        assert_eq!(neutral.transmission, plain.transmission);
    }
}
