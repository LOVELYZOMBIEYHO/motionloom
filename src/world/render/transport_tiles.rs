//! Conservative screen coverage for replaying rigid geometry only in relevant tiles.

use super::{GpuWorldParams, quat_normalize_xyzw, quat_rotate_vec3};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ScreenBounds {
    Empty,
    /// Half-open pixel edges: left, top, right, bottom.
    Rect([u32; 4]),
    Full,
}

impl ScreenBounds {
    /// `None` local bounds retain every tile, including skinned geometry.
    pub(super) fn bounds(
        local_bounds: Option<([f32; 3], [f32; 3])>,
        params: GpuWorldParams,
        target: [u32; 2],
    ) -> Self {
        if target.contains(&0) {
            return Self::Empty;
        }
        let Some((minimum, maximum)) = local_bounds else {
            return Self::Full;
        };
        if params.vegetation[0] != 0.0
            || params.canvas[0] <= 0.0
            || params.canvas[1] <= 0.0
            || params.camera1[3] < 0.0
            || minimum.iter().zip(maximum).any(|(a, b)| *a > b)
            || minimum
                .into_iter()
                .chain(maximum)
                .chain(params.canvas)
                .chain(params.model)
                .chain(params.actor)
                .chain(params.actor_rotation)
                .chain(params.camera0)
                .chain(params.camera1)
                .chain(params.camera2)
                .chain(params.camera3)
                .any(|value| !value.is_finite())
        {
            return Self::Full;
        }

        // CPU quaternion normalization has an identity fallback for very small
        // lengths; WGSL normalizes directly. Avoid using different transforms.
        let rotation_length = params
            .actor_rotation
            .iter()
            .map(|value| value * value)
            .sum::<f32>()
            .sqrt();
        if !rotation_length.is_finite() || rotation_length <= f32::EPSILON {
            return Self::Full;
        }

        let rotation = quat_normalize_xyzw(params.actor_rotation);
        let mut view_corners = [[0.0; 3]; 8];
        for (corner, view) in view_corners.iter_mut().enumerate() {
            let local = std::array::from_fn(|axis| {
                let value = if corner & (1 << axis) == 0 {
                    minimum[axis]
                } else {
                    maximum[axis]
                };
                (value - params.model[axis]) * params.model[3]
            });
            let rotated = quat_rotate_vec3(rotation, local);
            let relative: [f32; 3] = std::array::from_fn(|axis| {
                rotated[axis] + params.actor[axis] - params.camera0[axis]
            });
            for (axis, basis) in [params.camera1, params.camera2, params.camera3]
                .into_iter()
                .enumerate()
            {
                view[axis] = (0..3).map(|i| relative[i] * basis[i]).sum();
            }
            if view.iter().any(|value| !value.is_finite()) {
                return Self::Full;
            }
        }

        let near = params.camera1[3];
        // A box strictly behind the near plane is entirely clipped. Keep a
        // coplanar box conservative because depth exactly on the plane is valid.
        if view_corners.iter().all(|view| view[2] < near) {
            return Self::Empty;
        }
        // Corner-only perspective extrema are safe when the whole box is in
        // front of the near plane. Clipped/deformed geometry retains all tiles.
        if view_corners.iter().any(|view| view[2] <= near) {
            return Self::Full;
        }

        let scale = [
            target[0] as f32 / params.canvas[0],
            target[1] as f32 / params.canvas[1],
        ];
        let mut lower = [f32::INFINITY; 2];
        let mut upper = [f32::NEG_INFINITY; 2];
        for view in view_corners {
            let projection_w = if params.camera3[3] > 0.5 {
                1.0
            } else {
                view[2]
            };
            // Match geometry.wgsl's canvas-origin and Y inversion. The target
            // viewport can differ from the logical dimensions used by the DSL.
            let pixel = [
                (params.canvas[2] + view[0] * params.camera0[3] / projection_w) * scale[0],
                (params.canvas[3] - view[1] * params.camera0[3] / projection_w) * scale[1],
            ];
            if pixel.iter().any(|value| !value.is_finite()) {
                return Self::Full;
            }
            for axis in 0..2 {
                lower[axis] = lower[axis].min(pixel[axis]);
                upper[axis] = upper[axis].max(pixel[axis]);
            }
        }
        // Two physical pixels cover raster rounding and conservative edge
        // coverage. One logical pixel additionally bounds the Halton jitter.
        let padding = [2.0 + scale[0], 2.0 + scale[1]];
        let left = (lower[0] - padding[0]).floor().clamp(0.0, target[0] as f32) as u32;
        let top = (lower[1] - padding[1]).floor().clamp(0.0, target[1] as f32) as u32;
        let right = (upper[0] + padding[0]).ceil().clamp(0.0, target[0] as f32) as u32;
        let bottom = (upper[1] + padding[1]).ceil().clamp(0.0, target[1] as f32) as u32;
        if left >= right || top >= bottom {
            Self::Empty
        } else {
            Self::Rect([left, top, right, bottom])
        }
    }

    pub(super) fn union(self, other: Self) -> Self {
        match (self, other) {
            (Self::Full, _) | (_, Self::Full) => Self::Full,
            (Self::Empty, bounds) | (bounds, Self::Empty) => bounds,
            (Self::Rect(a), Self::Rect(b)) => Self::Rect([
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[2].max(b[2]),
                a[3].max(b[3]),
            ]),
        }
    }

    pub(super) fn intersects(self, tile: [u32; 4]) -> bool {
        if tile[2] == 0 || tile[3] == 0 {
            return false;
        }
        match self {
            Self::Empty => false,
            Self::Full => true,
            Self::Rect(bounds) => {
                bounds[0] < tile[0].saturating_add(tile[2])
                    && bounds[1] < tile[1].saturating_add(tile[3])
                    && bounds[2] > tile[0]
                    && bounds[3] > tile[1]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> GpuWorldParams {
        GpuWorldParams {
            canvas: [800.0, 600.0, 400.0, 300.0],
            model: [0.0, 0.0, 0.0, 1.0],
            actor_rotation: [0.0, 0.0, 0.0, 1.0],
            camera0: [0.0, 0.0, 0.0, 200.0],
            camera1: [1.0, 0.0, 0.0, 0.1],
            camera2: [0.0, 1.0, 0.0, 100.0],
            camera3: [0.0, 0.0, 1.0, 0.0],
            ..GpuWorldParams::default()
        }
    }

    #[test]
    fn perspective_projection_uses_nearest_box_extrema() {
        let bounds = ScreenBounds::bounds(
            Some(([-1.0, -1.0, 4.0], [1.0, 1.0, 6.0])),
            camera(),
            [800, 600],
        );
        assert_eq!(bounds, ScreenBounds::Rect([347, 247, 453, 353]));
        assert!(bounds.intersects([400, 300, 128, 128]));
        assert!(!bounds.intersects([0, 0, 128, 128]));
    }

    #[test]
    fn orthographic_projection_ignores_box_depth_and_scales_off_axis_origin() {
        let mut params = camera();
        params.camera3[3] = 1.0;
        params.canvas[2] = 300.0;
        params.canvas[3] = 220.0;
        let bounds = ScreenBounds::bounds(
            Some(([-1.0, -0.5, 4.0], [1.0, 0.5, 6.0])),
            params,
            [400, 300],
        );
        assert_eq!(bounds, ScreenBounds::Rect([47, 57, 253, 163]));
        params.camera3[3] = 0.0;
        let perspective = ScreenBounds::bounds(
            Some(([-1.0, -0.5, 4.0], [1.0, 0.5, 6.0])),
            params,
            [400, 300],
        );
        assert_eq!(perspective, ScreenBounds::Rect([122, 95, 178, 125]));
    }

    #[test]
    fn transformed_box_contains_projected_surface_points_and_jitter() {
        let mut params = camera();
        params.model = [0.3, -0.4, 0.1, -1.7];
        params.actor = [1.2, 0.2, 6.0, 0.0];
        let angle = 0.73_f32;
        params.actor_rotation = [0.0, (angle * 0.5).sin(), 0.0, (angle * 0.5).cos()];
        params.canvas[2] = 280.0;
        params.canvas[3] = 370.0;
        let min = [-1.0, -0.8, -0.6];
        let max = [1.3, 0.7, 0.9];
        let target = [1600, 1200];
        let bounds = ScreenBounds::bounds(Some((min, max)), params, target);
        assert!(matches!(bounds, ScreenBounds::Rect(_)));
        // Independently sample the box, including its faces and corners. This
        // checks negative scale, rotation, off-axis projection and target scaling.
        for x in 0..=8 {
            for y in 0..=8 {
                for z in 0..=8 {
                    let coordinate = [x, y, z];
                    let local = std::array::from_fn(|axis| {
                        let value =
                            min[axis] + (max[axis] - min[axis]) * coordinate[axis] as f32 / 8.0;
                        (value - params.model[axis]) * params.model[3]
                    });
                    let world = quat_rotate_vec3(params.actor_rotation, local);
                    let world: [f32; 3] =
                        std::array::from_fn(|axis| world[axis] + params.actor[axis]);
                    let normalized = [
                        (params.canvas[2] + world[0] * params.camera0[3] / world[2])
                            / params.canvas[0],
                        (params.canvas[3] - world[1] * params.camera0[3] / world[2])
                            / params.canvas[1],
                    ];
                    for jitter in [-0.5_f32, 0.5] {
                        let pixel = [
                            normalized[0] * target[0] as f32 + jitter * 2.0,
                            normalized[1] * target[1] as f32 + jitter * 2.0,
                        ];
                        if pixel[0] >= 0.0
                            && pixel[0] < target[0] as f32
                            && pixel[1] >= 0.0
                            && pixel[1] < target[1] as f32
                        {
                            assert!(bounds.intersects([
                                pixel[0].floor() as u32,
                                pixel[1].floor() as u32,
                                1,
                                1
                            ]));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn near_plane_and_deformation_fallback_never_remove_valid_tiles() {
        let params = camera();
        assert_eq!(
            ScreenBounds::bounds(
                Some(([-1.0, -1.0, -2.0], [1.0, 1.0, -1.0])),
                params,
                [800, 600]
            ),
            ScreenBounds::Empty
        );
        assert_eq!(
            ScreenBounds::bounds(
                Some(([-1.0, -1.0, -1.0], [1.0, 1.0, 1.0])),
                params,
                [800, 600]
            ),
            ScreenBounds::Full
        );
        assert_eq!(
            ScreenBounds::bounds(
                Some(([-1.0, -1.0, 0.1], [1.0, 1.0, 0.1])),
                params,
                [800, 600]
            ),
            ScreenBounds::Full
        );
        assert_eq!(
            ScreenBounds::bounds(None, params, [800, 600]),
            ScreenBounds::Full
        );
        let mut wind = params;
        wind.vegetation[0] = 1.0;
        assert_eq!(
            ScreenBounds::bounds(
                Some(([100.0, 0.0, 4.0], [101.0, 1.0, 5.0])),
                wind,
                [800, 600]
            ),
            ScreenBounds::Full
        );
        let mut invalid = params;
        invalid.actor_rotation[0] = f32::NAN;
        assert_eq!(
            ScreenBounds::bounds(
                Some(([-1.0, -1.0, 4.0], [1.0, 1.0, 5.0])),
                invalid,
                [800, 600]
            ),
            ScreenBounds::Full
        );
        invalid.actor_rotation = [0.0, 1.0e-12, 0.0, 0.0];
        assert_eq!(
            ScreenBounds::bounds(
                Some(([-1.0, -1.0, 4.0], [1.0, 1.0, 5.0])),
                invalid,
                [800, 600]
            ),
            ScreenBounds::Full
        );
    }

    #[test]
    fn offscreen_boxes_are_empty_and_batch_union_keeps_each_instance() {
        let params = camera();
        assert_eq!(
            ScreenBounds::bounds(
                Some(([100.0, 0.0, 4.0], [101.0, 1.0, 5.0])),
                params,
                [800, 600]
            ),
            ScreenBounds::Empty
        );
        let left = ScreenBounds::Rect([10, 20, 40, 60]);
        let right = ScreenBounds::Rect([300, 100, 360, 130]);
        let batch = ScreenBounds::Empty.union(left).union(right);
        assert_eq!(batch, ScreenBounds::Rect([10, 20, 360, 130]));
        assert!(batch.intersects([0, 0, 128, 128]));
        assert!(batch.intersects([256, 128, 128, 128]));
        assert!(!batch.intersects([384, 0, 128, 128]));
        assert_eq!(batch.union(ScreenBounds::Full), ScreenBounds::Full);
        assert!(!ScreenBounds::Full.intersects([0, 0, 0, 128]));
        assert!(!ScreenBounds::Rect([0, 0, 128, 128]).intersects([128, 0, 128, 128]));
    }
}
