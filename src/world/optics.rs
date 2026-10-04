// =========================================
// =========================================
// src/world/optics.rs

/// Physical camera values shared by raster preview and offline ray generation.
/// Focus is axial depth in scene units; one scene unit is one meter.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ResolvedCameraOptics {
    pub focus_distance: f32,
    pub focal_length_mm: f32,
    pub f_stop: f32,
}

impl ResolvedCameraOptics {
    /// Keep both backends inside the same authored optical ranges.
    pub(crate) fn new(focus_distance: f32, focal_length_mm: f32, f_stop: f32) -> Self {
        Self {
            focus_distance: focus_distance.max(0.05),
            focal_length_mm: focal_length_mm.clamp(1.0, 300.0),
            f_stop: f_stop.clamp(0.7, 64.0),
        }
    }

    pub(crate) fn aperture_radius(&self) -> f32 {
        self.focal_length_mm * 0.001 / (2.0 * self.f_stop)
    }

    /// Send the actual aperture to WGSL rather than assuming a 24 mm sensor.
    pub(crate) fn preview_uniform(&self, max_blur_px: f32) -> [f32; 4] {
        [
            self.focus_distance,
            self.aperture_radius(),
            self.f_stop,
            max_blur_px,
        ]
    }

    #[cfg(test)]
    pub(crate) fn blur_radius_pixels(&self, depth: f32, focal_px: f32) -> f32 {
        self.aperture_radius() * focal_px * (depth - self.focus_distance).abs()
            / (depth * self.focus_distance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aperture_and_projected_blur_follow_physical_lens() {
        let lens = ResolvedCameraOptics::new(6.0, 50.0, 2.0);
        assert_eq!(lens.blur_radius_pixels(6.0, 1000.0), 0.0);
        assert!((lens.blur_radius_pixels(3.0, 1000.0) - 2.0833333).abs() < 1e-5);
        let stopped_down = ResolvedCameraOptics::new(6.0, 50.0, 4.0);
        assert!((lens.aperture_radius() / stopped_down.aperture_radius() - 2.0).abs() < 1e-6);
        // Doubling image dimensions doubles the same physical blur in pixels.
        assert_eq!(
            lens.blur_radius_pixels(3.0, 2000.0),
            2.0 * lens.blur_radius_pixels(3.0, 1000.0)
        );
    }
}
