//! Immediate rendering uses bounded raster effects, never implicit full-scene rays.
//!
//! The former geometry transport remains an internal native comparison path.
//! This switch is not authored scene state and cannot enter a browser build.

pub(super) fn reference_geometry_transport_enabled() -> bool {
    #[cfg(test)]
    if let Some(enabled) = TEST_REFERENCE_TRANSPORT.with(std::cell::Cell::get) {
        return enabled;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var("MOTIONLOOM_REFERENCE_GEOMETRY_TRANSPORT").as_deref() == Ok("1")
    }
    #[cfg(target_arch = "wasm32")]
    {
        false
    }
}

#[cfg(test)]
thread_local! {
    static TEST_REFERENCE_TRANSPORT: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(super) struct TestReferenceTransport(Option<bool>);

#[cfg(test)]
impl Drop for TestReferenceTransport {
    fn drop(&mut self) {
        TEST_REFERENCE_TRANSPORT.with(|setting| setting.set(self.0));
    }
}

#[cfg(test)]
pub(super) fn test_reference_transport(enabled: bool) -> TestReferenceTransport {
    TestReferenceTransport(TEST_REFERENCE_TRANSPORT.with(|setting| setting.replace(Some(enabled))))
}
