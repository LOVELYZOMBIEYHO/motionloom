//! Retained portable irradiance grids and local HDR reflection probes.
use super::*;
use crate::lighting_bake::{BakedLightingAsset, LIGHTING_BAKE_SCHEMA_VERSION};
use std::cell::RefCell;

pub(super) const PROBE_STRIDE: usize = 148;

#[derive(Debug)]
pub(super) struct GpuBakedLightingData {
    pub vectors: Vec<[f32; 4]>,
    pub reflections: Vec<WorldEnvironmentImage>,
    pub volume_count: u32,
    pub signature: u64,
    pub dependencies: Vec<PathBuf>,
    pub authoring_fingerprint: String,
}

/// A stale authoring revision is recoverable in the interactive renderer;
/// malformed or unavailable assets are still errors.
pub(super) struct PreviewBakedLightingLoad {
    pub lighting: Option<Arc<GpuBakedLightingData>>,
    pub diagnostic: Option<PreviewBakedLightingDiagnostic>,
}

pub(super) struct PreviewBakedLightingDiagnostic {
    pub source_ref: String,
    pub message: String,
}

const STALE_SOURCE_MESSAGE: &str =
    "stale lighting bake: authored geometry, material or lights changed; rebake the scene";

thread_local! {
    static FILE_CACHE: RefCell<HashMap<PathBuf,(u64,Vec<(PathBuf,u64)>,Arc<GpuBakedLightingData>)>> = RefCell::new(HashMap::new());
    static CACHE: RefCell<HashMap<(PathBuf,u64),Arc<GpuBakedLightingData>>> = RefCell::new(HashMap::new());
}

fn asset_error(src: &str, message: impl Into<String>) -> WorldRenderError {
    WorldRenderError::BakedLighting {
        source_ref: src.to_string(),
        message: message.into(),
    }
}

fn verify_source(
    binding: &crate::world::WorldBakedLighting,
    fingerprint: &str,
) -> Result<(), WorldRenderError> {
    if !binding.expected_authoring_fingerprint.is_empty()
        && binding.expected_authoring_fingerprint != fingerprint
    {
        return Err(asset_error(&binding.src, STALE_SOURCE_MESSAGE));
    }
    Ok(())
}

/// Interactive editing may change the source without producing a new bake.
/// Retain the published fingerprint contract, but omit stale lighting rather
/// than apply it or stop the preview. With no baked data, lighting uses its
/// ordinary environment fallback and profile-controlled screen-space GI.
pub(super) fn load_preview_baked_lighting(
    binding: &crate::world::WorldBakedLighting,
    asset_root: &Path,
    resolver: &dyn AssetResolver,
) -> Result<PreviewBakedLightingLoad, WorldRenderError> {
    // Validate the complete payload first. A stale source must not hide broken
    // JSON, invalid probe grids or missing reflection images.
    let mut unchecked_source = binding.clone();
    unchecked_source.expected_authoring_fingerprint.clear();
    let lighting = load_baked_lighting(&unchecked_source, asset_root, resolver)?;
    match verify_source(binding, &lighting.authoring_fingerprint) {
        Ok(()) => Ok(PreviewBakedLightingLoad {
            lighting: Some(lighting),
            diagnostic: None,
        }),
        Err(WorldRenderError::BakedLighting {
            source_ref,
            message,
        }) if message == STALE_SOURCE_MESSAGE => Ok(PreviewBakedLightingLoad {
            lighting: None,
            diagnostic: Some(PreviewBakedLightingDiagnostic {
                source_ref,
                message: format!(
                    "{message}; preview omits this bake and uses environment lighting with profile-controlled screen-space GI"
                ),
            }),
        }),
        Err(error) => Err(error),
    }
}

fn file_stamp(path: &Path) -> Option<u64> {
    let metadata = std::fs::metadata(path).ok()?;
    let mut h = DefaultHasher::new();
    metadata.len().hash(&mut h);
    metadata.modified().ok()?.hash(&mut h);
    Some(h.finish())
}

pub(super) fn load_baked_lighting(
    binding: &crate::world::WorldBakedLighting,
    asset_root: &Path,
    resolver: &dyn AssetResolver,
) -> Result<Arc<GpuBakedLightingData>, WorldRenderError> {
    let authored_path = asset_root.join(&binding.src);
    if let Some(stamp) = file_stamp(&authored_path) {
        if let Some(data) = FILE_CACHE.with(|c| {
            c.borrow()
                .get(&authored_path)
                .and_then(|(old, dependencies, data)| {
                    (*old == stamp
                        && dependencies
                            .iter()
                            .all(|(path, expected)| file_stamp(path) == Some(*expected)))
                    .then(|| data.clone())
                })
        }) {
            verify_source(binding, &data.authoring_fingerprint)?;
            return Ok(data);
        }
    }
    let resolved =
        resolve_world_asset_source(asset_root, &binding.src, WorldPathStyle::Relative, resolver)?;
    let bytes = match &resolved {
        ResolvedWorldAsset::Path(path) => {
            if std::fs::metadata(path)
                .map_err(|e| asset_error(&binding.src, e.to_string()))?
                .len()
                > 64 * 1024 * 1024
            {
                return Err(asset_error(&binding.src, "lighting JSON exceeds64MB"));
            }
            std::fs::read(path).map_err(|e| asset_error(&binding.src, e.to_string()))?
        }
        ResolvedWorldAsset::Bytes { bytes, .. } => bytes.clone(),
        ResolvedWorldAsset::Missing { .. } => {
            return Err(asset_error(&binding.src, "asset is missing"));
        }
    };
    if bytes.len() > 64 * 1024 * 1024 {
        return Err(asset_error(&binding.src, "lighting JSON exceeds64MB"));
    }
    // Hash JSON and every referenced HDR's metadata. Cameras and blend values
    // do not enter this key; changed files do invalidate retained GPU resources.
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    let asset: BakedLightingAsset =
        serde_json::from_slice(&bytes).map_err(|e| asset_error(&binding.src, e.to_string()))?;
    verify_source(binding, &asset.authoring_fingerprint)?;
    validate(&asset).map_err(|e| asset_error(&binding.src, e))?;
    let parent = resolved.key().parent().unwrap_or(asset_root);
    for state in &asset.states {
        for volume in &state.volumes {
            for probe in &volume.reflections {
                let path = parent.join(&probe.src);
                path.hash(&mut hasher);
                if let Ok(meta) = std::fs::metadata(&path) {
                    meta.len().hash(&mut hasher);
                    meta.modified().ok().hash(&mut hasher);
                }
            }
        }
    }
    let signature = hasher.finish();
    let key = (resolved.key().to_path_buf(), signature);
    if let Some(data) = CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return Ok(data);
    }
    let mut vectors = vec![[0.0; 4]; asset.states[0].volumes.len() * 4];
    let mut reflections = Vec::new();
    let mut dependencies = Vec::new();
    for (index, (day, dusk)) in asset.states[0]
        .volumes
        .iter()
        .zip(&asset.states[1].volumes)
        .enumerate()
    {
        let base = vectors.len();
        let reflection_base = reflections.len();
        // A volume chooses its first local capture. Separate room volumes
        // provide spatial selection, and day/dusk occupy consecutive layers.
        let reflection_position = day
            .reflections
            .first()
            .map(|p| p.position)
            .unwrap_or([0.0; 3]);
        for state_volume in [day, dusk] {
            if let Some(probe) = state_volume.reflections.first() {
                let src = parent.join(&probe.src).to_string_lossy().into_owned();
                let source = resolve_world_asset_source(
                    asset_root,
                    &src,
                    WorldPathStyle::Relative,
                    resolver,
                )?;
                dependencies.push(source.key().to_path_buf());
                reflections.push(load_environment_image_from_resolved(&source)?);
            }
        }
        vectors[index * 4] = [
            day.bounds_min[0],
            day.bounds_min[1],
            day.bounds_min[2],
            base as f32,
        ];
        vectors[index * 4 + 1] = [
            day.bounds_max[0],
            day.bounds_max[1],
            day.bounds_max[2],
            reflection_base as f32,
        ];
        vectors[index * 4 + 2] = [
            day.counts[0] as f32,
            day.counts[1] as f32,
            day.counts[2] as f32,
            day.probes.len() as f32,
        ];
        vectors[index * 4 + 3] = [
            reflection_position[0],
            reflection_position[1],
            reflection_position[2],
            (!day.reflections.is_empty()) as u8 as f32,
        ];
        for (a, b) in day.probes.iter().zip(&dusk.probes) {
            vectors.push([
                a.position[0],
                a.position[1],
                a.position[2],
                a.valid as u8 as f32,
            ]);
            vectors.push([
                b.position[0],
                b.position[1],
                b.position[2],
                b.valid as u8 as f32,
            ]);
            for probe in [a, b] {
                for rgb in probe.irradiance_sh {
                    vectors.push([rgb[0], rgb[1], rgb[2], 0.0]);
                }
            }
            for probe in [a, b] {
                for (moment, visibility) in probe.depth_moments.iter().zip(&probe.visibility) {
                    vectors.push([moment[0], moment[1], *visibility, 0.0]);
                }
            }
        }
        debug_assert_eq!(vectors.len() - base, day.probes.len() * PROBE_STRIDE);
    }
    if let Some(first) = reflections.first() {
        if reflections.iter().any(|image| {
            image.width != first.width
                || image.height != first.height
                || image.mip_bytes.len() != first.mip_bytes.len()
        }) {
            return Err(asset_error(
                &binding.src,
                "reflection captures must share dimensions and mip count",
            ));
        }
    }
    let data = Arc::new(GpuBakedLightingData {
        vectors,
        reflections,
        volume_count: asset.states[0].volumes.len() as u32,
        signature,
        dependencies,
        authoring_fingerprint: asset.authoring_fingerprint.clone(),
    });
    CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        if cache.len() > 8 {
            cache.clear();
        }
        cache.insert(key, data.clone());
    });
    if let Some(stamp) = file_stamp(&authored_path) {
        let dependencies = data
            .dependencies
            .iter()
            .filter_map(|p| file_stamp(p).map(|s| (p.clone(), s)))
            .collect();
        FILE_CACHE.with(|c| {
            let mut cache = c.borrow_mut();
            if cache.len() > 8 {
                cache.clear();
            }
            cache.insert(authored_path, (stamp, dependencies, data.clone()));
        });
    }
    Ok(data)
}

pub(super) fn validate(asset: &BakedLightingAsset) -> Result<(), String> {
    if asset.schema_version != LIGHTING_BAKE_SCHEMA_VERSION || asset.states.len() != 2 {
        return Err("unsupported schema or lighting state count (expected two)".into());
    }
    let states = &asset.states;
    if states[0].volumes.is_empty()
        || states[0].volumes.len() > 32
        || states[0].volumes.len() != states[1].volumes.len()
    {
        return Err("expected 1–32 matching room volumes".into());
    }
    let mut probe_count = 0usize;
    for (a, b) in states[0].volumes.iter().zip(&states[1].volumes) {
        if a.reflections.len() > 1 {
            return Err("schema v1 supports at most one reflection capture per room volume".into());
        }
        for (p, q) in a.reflections.iter().zip(&b.reflections) {
            if p.position != q.position
                || p.bounds_min != q.bounds_min
                || p.bounds_max != q.bounds_max
                || p.position
                    .iter()
                    .chain(p.bounds_min.iter())
                    .chain(p.bounds_max.iter())
                    .any(|v| !v.is_finite())
                || p.src.is_empty()
                || q.src.is_empty()
            {
                return Err("incompatible or malformed local reflection captures".into());
            }
        }
        if a.id != b.id
            || a.counts != b.counts
            || a.bounds_min != b.bounds_min
            || a.bounds_max != b.bounds_max
            || a.reflections.len() != b.reflections.len()
        {
            return Err("lighting states have incompatible volumes".into());
        }
        if a.counts.iter().any(|n| *n < 2 || *n > 32)
            || a.bounds_min
                .iter()
                .zip(a.bounds_max)
                .any(|(x, y)| !x.is_finite() || !y.is_finite() || *x >= y)
        {
            return Err("invalid probe grid bounds/counts".into());
        }
        let count = a.counts.iter().fold(1usize, |n, v| n * *v as usize);
        probe_count += count;
        if probe_count > 16384 {
            return Err("probe allocation exceeds 16384 probes".into());
        }
        for v in [a, b] {
            if v.probes.len() != count {
                return Err("probe count does not match grid dimensions".into());
            }
            for p in &v.probes {
                if p.depth_moments.len() != 64
                    || p.visibility.len() != 64
                    || p.position.iter().any(|x| !x.is_finite())
                    || p.irradiance_sh.iter().flatten().any(|x| !x.is_finite())
                    || p.depth_moments
                        .iter()
                        .any(|m| !m[0].is_finite() || !m[1].is_finite() || m[0] < 0.0 || m[1] < 0.0)
                    || p.visibility
                        .iter()
                        .any(|x| !x.is_finite() || *x < 0.0 || *x > 1.0)
                {
                    return Err("nonfinite or malformed probe data".into());
                }
            }
        }
    }
    Ok(())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod preview_fallback_tests {
    use super::*;
    use crate::lighting_bake::{
        BakeFingerprint, BakedLightingState, BakedLightingVolume, IrradianceProbe,
        LocalReflectionProbe,
    };
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture {
        root: PathBuf,
        binding: crate::world::WorldBakedLighting,
        asset: BakedLightingAsset,
    }

    impl Fixture {
        fn new() -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "motionloom-preview-bake-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&root).unwrap();
            let volume = BakedLightingVolume {
                id: "room".into(),
                bounds_min: [0.; 3],
                bounds_max: [1.; 3],
                counts: [2; 3],
                probes: vec![
                    IrradianceProbe {
                        position: [0.5; 3],
                        valid: true,
                        irradiance_sh: [[1.; 3]; 9],
                        depth_moments: vec![[10., 100.]; 64],
                        visibility: vec![0.; 64],
                    };
                    8
                ],
                reflections: vec![],
            };
            let asset = BakedLightingAsset {
                schema_version: LIGHTING_BAKE_SCHEMA_VERSION,
                baker_version: crate::lighting_bake::LIGHTING_BAKER_VERSION.into(),
                scene_id: "room".into(),
                authoring_fingerprint: "published-source".into(),
                fingerprint: BakeFingerprint {
                    geometry: "geometry".into(),
                    materials: "materials".into(),
                    textures: "textures".into(),
                    lighting: "lights".into(),
                    environments: "environments".into(),
                    settings: "settings".into(),
                    combined: "combined".into(),
                },
                states: ["day", "dusk"]
                    .into_iter()
                    .enumerate()
                    .map(|(frame, name)| BakedLightingState {
                        name: name.into(),
                        frame: frame as u32,
                        volumes: vec![volume.clone()],
                    })
                    .collect(),
                diagnostics: vec![],
            };
            let fixture = Self {
                root,
                binding: crate::world::WorldBakedLighting {
                    src: "room.json".into(),
                    blend: 0.,
                    intensity: 1.,
                    specular_intensity: 1.,
                    expected_authoring_fingerprint: "published-source".into(),
                },
                asset,
            };
            fixture.write();
            fixture
        }

        fn write(&self) {
            std::fs::write(
                self.root.join(&self.binding.src),
                serde_json::to_vec(&self.asset).unwrap(),
            )
            .unwrap();
        }

        fn load(&self) -> Result<PreviewBakedLightingLoad, WorldRenderError> {
            load_preview_baked_lighting(&self.binding, &self.root, &crate::PathAssetResolver)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn preview_omits_stale_bake_while_strict_loading_keeps_rejecting_it() {
        let mut fixture = Fixture::new();
        let valid = fixture.load().unwrap();
        assert!(valid.lighting.is_some());
        assert!(valid.diagnostic.is_none());
        fixture.binding.expected_authoring_fingerprint = "changed-surface-style".into();
        for _ in 0..2 {
            let stale = fixture.load().unwrap();
            assert!(stale.lighting.is_none());
            let diagnostic = stale.diagnostic.unwrap();
            assert_eq!(diagnostic.source_ref, "room.json");
            assert!(diagnostic.message.contains("preview omits this bake"));
            assert!(load_baked_lighting(
                &fixture.binding,
                &fixture.root,
                &crate::PathAssetResolver
            )
            .is_err());
        }
        fixture.binding.expected_authoring_fingerprint = "published-source".into();
        let restored = fixture.load().unwrap();
        assert!(Arc::ptr_eq(
            valid.lighting.as_ref().unwrap(),
            restored.lighting.as_ref().unwrap()
        ));
    }

    #[test]
    fn stale_source_does_not_hide_missing_malformed_or_invalid_bake() {
        let mut fixture = Fixture::new();
        fixture.binding.expected_authoring_fingerprint = "stale".into();
        let path = fixture.root.join(&fixture.binding.src);
        std::fs::remove_file(&path).unwrap();
        assert!(fixture.load().is_err());
        std::fs::write(&path, b"not valid JSON").unwrap();
        assert!(fixture.load().is_err());
        fixture.asset.states[0].volumes[0].probes.pop();
        fixture.write();
        assert!(fixture.load().is_err());
    }

    #[test]
    fn stale_source_does_not_hide_missing_reflection_image() {
        let mut fixture = Fixture::new();
        fixture.binding.expected_authoring_fingerprint = "stale".into();
        for state in &mut fixture.asset.states {
            state.volumes[0].reflections.push(LocalReflectionProbe {
                position: [0.5; 3],
                bounds_min: [0.; 3],
                bounds_max: [1.; 3],
                src: "missing.hdr".into(),
            });
        }
        fixture.write();
        assert!(fixture.load().is_err());
    }
}
