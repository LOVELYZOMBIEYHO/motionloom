// =========================================
// =========================================
// src/world/render/shaders/mod.rs

//! Embedded WGSL sources used to build the shared world-render pipelines.

/// Retained geometry-transport reference used by explicit internal comparisons.
pub(super) static WGPU_WORLD_SHADER: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| assemble_world_shader(false));

/// Native and browser preview share a raster PBR module without a scene BVH.
pub(super) static WGPU_WORLD_REALTIME_SHADER: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| assemble_world_shader(true));

fn assemble_world_shader(realtime: bool) -> String {
    let per_light_shadows = include_str!("per_light_shadows.wgsl");
    // Only reflected geometry receivers use this helper. Removing its body
    // also removes the last reference to the transport storage binding.
    let per_light_shadows = if realtime {
        let start = per_light_shadows
            .find("fn secondary_emitter_opaque_evidence(")
            .expect("secondary shadow helper exists");
        let end = per_light_shadows[start..]
            .find("fn directional_emitter_visibility(")
            .map(|offset| start + offset)
            .expect("primary shadow helper follows secondary helper");
        format!(
            "{}{}",
            &per_light_shadows[..start],
            &per_light_shadows[end..]
        )
    } else {
        per_light_shadows.to_owned()
    };
    let surface = include_str!("surface.wgsl");
    let surface = if realtime {
        surface.replace(
            "override GEOMETRY_REFLECTION_ENABLED: bool = true;",
            "const GEOMETRY_REFLECTION_ENABLED: bool = false;",
        )
    } else {
        surface.to_owned()
    };
    [
        include_str!("bindings.wgsl"),
        include_str!("environment.wgsl"),
        include_str!("indirect_lighting.wgsl"),
        include_str!("shadows.wgsl"),
        per_light_shadows.as_str(),
        include_str!("geometry.wgsl"),
        include_str!("outline.wgsl"),
        include_str!("shading/physical.wgsl"),
        include_str!("shading/stylized.wgsl"),
        include_str!("shading/toon.wgsl"),
        include_str!("shading/cel.wgsl"),
        include_str!("shading/clay.wgsl"),
        include_str!("lighting.wgsl"),
        include_str!("color.wgsl"),
        include_str!("fog.wgsl"),
        if realtime {
            include_str!("realtime_transport.wgsl")
        } else {
            include_str!("hybrid_transport.wgsl")
        },
        surface.as_str(),
        include_str!("rough_reflection.wgsl"),
    ]
    .concat()
}
pub(super) const WGPU_WORLD_DOF_SHADER: &str = concat!(
    include_str!("dof.wgsl"),
    "\n",
    include_str!("presets/filmic_bokeh_v1.wgsl"),
    "\n",
    include_str!("presets/cinematic_bokeh_v1.wgsl"),
    "\n",
    include_str!("presets/ink_wash_soft_v1.wgsl"),
    "\n",
    include_str!("presets/pbr_npr_soft_v1.wgsl"),
    "\n",
    include_str!("universal_style.wgsl"),
);
pub(super) const WGPU_GROUND_GRID_SHADER: &str = include_str!("ground_grid.wgsl");
pub(super) const WGPU_FROXEL_INJECT_SHADER: &str = include_str!("volumetric_inject.wgsl");
pub(super) const WGPU_FROXEL_INTEGRATE_SHADER: &str = include_str!("volumetric_integrate.wgsl");
pub(super) const WGPU_FROXEL_COMPOSITE_SHADER: &str = include_str!("volumetric_composite.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn realtime_world_shader_has_no_scene_transport_storage() {
        for physical_only in [false, true] {
            let source = super::super::shader_specialization::physical_style_shader_source(
                WGPU_WORLD_REALTIME_SHADER.as_str(),
                physical_only,
            );
            let source = source
                .replace(
                    "/* sample rough evidence */sample_rough_reflection_evidence(",
                    "rough_reflection_unavailable(",
                )
                .replace(
                    "/* sample reflection route */sample_primary_reflection_route(",
                    "rough_reflection_default_route(",
                );
            let module = wgpu::naga::front::wgsl::parse_str(&source)
                .expect("realtime world shader must parse");
            let info = wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .expect("realtime world shader must validate");
            assert!(module.global_variables.iter().all(|(_, global)| {
                global.name.as_deref() != Some("hybrid_scene")
                    && !global
                        .binding
                        .as_ref()
                        .is_some_and(|binding| binding.group == 1 && binding.binding == 20)
            }));
            for traversal in [
                "hybrid_intersect_filtered",
                "hybrid_trace_reflection_ray",
                "hybrid_trace_transmission_core",
                "hybrid_consume_node_visit",
                "hybrid_visibility_surface",
            ] {
                assert!(module
                    .functions
                    .iter()
                    .all(|(_, function)| function.name.as_deref() != Some(traversal)));
            }
            // Every ordinary raster entry can use the established layout:
            // no rough-query evidence texture is required behind a guard.
            for entry in [
                "fs_main",
                "fs_main_gbuffer",
                "fs_main_mrt",
                "fs_capture_opaque",
                "fs_transmissive",
                "fs_transmissive_mrt",
            ] {
                let index = module
                    .entry_points
                    .iter()
                    .position(|candidate| candidate.name == entry)
                    .unwrap_or_else(|| panic!("missing realtime entry {entry}"));
                for (global, variable) in module.global_variables.iter() {
                    if variable
                        .binding
                        .as_ref()
                        .is_some_and(|binding| binding.group == 3)
                    {
                        assert!(
                            info.get_entry_point(index)[global].is_empty(),
                            "realtime {entry} cannot require query-evidence resources"
                        );
                    }
                }
            }
        }
        assert!(WGPU_WORLD_SHADER.contains("var<storage, read> hybrid_scene"));
        assert!(WGPU_WORLD_SHADER.contains("fn hybrid_trace_reflection_ray"));
    }

    #[test]
    fn realtime_screen_resolve_and_optical_snapshot_validate() {
        let module = wgpu::naga::front::wgsl::parse_str(WGPU_WORLD_DOF_SHADER)
            .expect("screen-space reflection and resolve shader must parse");
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("screen-space reflection and resolve shader must validate");
        let realtime = wgpu::naga::front::wgsl::parse_str(WGPU_WORLD_REALTIME_SHADER.as_str())
            .expect("realtime optical source must parse");
        // The shared screen approximation retains material-layer optics and
        // per-tap snapshot rejection without importing geometry traversal.
        for helper in [
            "material_sheen_scale",
            "material_coat_fresnel",
            "sample_local_specular",
            "transmission_snapshot_sample",
            "transmission_scene_view_depth",
            "hybrid_fresnel",
        ] {
            assert!(realtime
                .functions
                .iter()
                .any(|(_, function)| function.name.as_deref() == Some(helper)));
        }
    }
}
