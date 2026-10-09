//! Select extra visibility work only for large islands with expensive lighting.
use super::GpuWorldLighting;

pub(super) fn use_prepass(items: usize, lighting: &GpuWorldLighting) -> bool {
    #[cfg(test)]
    if let Some(enabled) = TEST_PREPASS.with(std::cell::Cell::get) { return enabled; }
    #[cfg(not(target_arch = "wasm32"))]
    if std::env::var("MOTIONLOOM_TRACE_RASTER_VISIBILITY").as_deref() == Ok("0") {
        return false;
    }
    profitable(
        items,
        lighting.params.surface0[0],
        lighting.per_light_shadows,
        lighting.params.baked0[2],
    )
}

#[cfg(test)]
thread_local! {
    static TEST_PREPASS: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}
#[cfg(test)]
pub(super) struct TestPrepass(Option<bool>);
#[cfg(test)]
impl Drop for TestPrepass {
    fn drop(&mut self) { TEST_PREPASS.with(|setting| setting.set(self.0)); }
}
#[cfg(test)]
pub(super) fn test_prepass(enabled: bool) -> TestPrepass {
    TestPrepass(TEST_PREPASS.with(|setting| setting.replace(Some(enabled))))
}

fn profitable(items: usize, shading: f32, per_light: bool, baked: f32) -> bool {
    items >= 256 && shading < 0.5 && (per_light || baked > 0.0)
}

#[cfg(test)]
mod tests {
    use super::profitable;
    #[test]
    fn extra_depth_work_requires_large_physical_lit_islands() {
        assert!(profitable(256, 0.0, true, 0.0));
        assert!(profitable(256, 0.0, false, 1.0));
        assert!(!profitable(255, 0.0, true, 1.0));
        assert!(!profitable(770, 1.0, true, 1.0));
        assert!(!profitable(770, 0.0, false, 0.0));
    }
}
