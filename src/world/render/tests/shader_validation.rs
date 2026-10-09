// =========================================
// =========================================
// src/world/render/tests/shader_validation.rs

//! Validates the assembled shader and the presence of each shading module.

#[test]
fn raster_visibility_shader_specializations_validate() {
    let source = super::super::WGPU_WORLD_SHADER.as_str();
    for enabled in [false, true] {
        let source = source.replace("override RASTER_VISIBILITY_ENABLED: bool = false;",
            &format!("const RASTER_VISIBILITY_ENABLED: bool = {enabled};"));
        let module = wgpu::naga::front::wgsl::parse_str(&source).expect("raster visibility shader parses");
        wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all()).validate(&module)
            .expect("raster visibility shader validates");
    }
}

#[test]
fn assembled_world_shader_contains_every_shading_mode() {
    let source = super::super::WGPU_WORLD_SHADER.as_str();
    for function in [
        "fn shade_physical",
        "fn shade_stylized",
        "fn toon_intensity",
        "fn shade_cel",
        "fn clay_base_color",
    ] {
        assert!(
            source.contains(function),
            "missing shader function {function}"
        );
    }
    let module = wgpu::naga::front::wgsl::parse_str(source).expect("assembled WGSL must parse");
    wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("assembled WGSL must validate");
}

#[test]
fn assembled_world_shader_validates_every_static_transport_specialization() {
    let source = super::super::WGPU_WORLD_SHADER.as_str();
    let solid_declaration = "override HYBRID_SOLID_TRANSPORT: bool = true;";
    let primary_glass_declaration = "override PRIMARY_GLASS_SHADOWS_ENABLED: bool = true;";
    let reflection_declaration = "override GEOMETRY_REFLECTION_ENABLED: bool = true;";
    let coarse_declaration = "override COARSE_REFLECTION_ENABLED: bool = false;";
    let fast_declaration = "override COARSE_ROUTE_FAST_ONLY: bool = false;";
    let full_declaration = "override COARSE_ROUTE_FULL_ONLY: bool = false;";
    assert_eq!(source.matches(solid_declaration).count(), 1);
    assert_eq!(source.matches(primary_glass_declaration).count(), 1);
    assert_eq!(source.matches(reflection_declaration).count(), 1);
    assert_eq!(source.matches(coarse_declaration).count(), 1);
    assert_eq!(source.matches(fast_declaration).count(), 1);
    assert_eq!(source.matches(full_declaration).count(), 1);
    for (solid, primary_glass) in [(false, false), (false, true), (true, false), (true, true)] {
        for reflection in [false, true] {
            for coarse in [false, true] {
                for (fast, full) in [(false, false), (true, false), (false, true)] {
                    // Pipeline overrides are compile-time values. Constant
                    // substitution validates all equivalent assembled variants
                    // without requiring a native adapter or backend compiler.
                    let specialized = source
                        .replace(
                            solid_declaration,
                            &format!("const HYBRID_SOLID_TRANSPORT: bool = {solid};"),
                        )
                        .replace(
                            primary_glass_declaration,
                            &format!("const PRIMARY_GLASS_SHADOWS_ENABLED: bool = {primary_glass};"),
                        )
                        .replace(
                            reflection_declaration,
                            &format!("const GEOMETRY_REFLECTION_ENABLED: bool = {reflection};"),
                        )
                        .replace(
                            coarse_declaration,
                            &format!("const COARSE_REFLECTION_ENABLED: bool = {coarse};"),
                        )
                        .replace(
                            fast_declaration,
                            &format!("const COARSE_ROUTE_FAST_ONLY: bool = {fast};"),
                        )
                        .replace(
                            full_declaration,
                            &format!("const COARSE_ROUTE_FULL_ONLY: bool = {full};"),
                        );
                    let description = format!(
                        "solid={solid}, primary glass shadows={primary_glass}, geometry reflection={reflection}, coarse={coarse}, fast={fast}, full={full}"
                    );
                    let module =
                        wgpu::naga::front::wgsl::parse_str(&specialized).unwrap_or_else(|error| {
                            panic!("WGSL parse failed for {description}: {error}")
                        });
                    wgpu::naga::valid::Validator::new(
                        wgpu::naga::valid::ValidationFlags::all(),
                        wgpu::naga::valid::Capabilities::all(),
                    )
                    .validate(&module)
                    .unwrap_or_else(|error| {
                        panic!("WGSL validation failed for {description}: {error}")
                    });
                }
            }
        }
    }
}

#[test]
fn ordinary_world_entries_do_not_require_rough_evidence_bind_group() {
    let source = super::super::WGPU_WORLD_SHADER.as_str();
    let sampling_call = "/* sample rough evidence */sample_rough_reflection_evidence(";
    let route_call = "/* sample reflection route */sample_primary_reflection_route(";
    assert_eq!(source.matches(sampling_call).count(), 2);
    assert_eq!(source.matches(route_call).count(), 1);
    for ordinary in [false, true] {
        let source = if ordinary {
            source
                .replace(sampling_call, "rough_reflection_unavailable(")
                .replace(route_call, "rough_reflection_default_route(")
        } else {
            source.to_owned()
        };
        let module = wgpu::naga::front::wgsl::parse_str(&source).expect("world variant parses");
        let info = wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("world variant validates");
        let evidence_globals = module
            .global_variables
            .iter()
            .filter(|(_, global)| {
                global
                    .binding
                    .as_ref()
                    .is_some_and(|binding| binding.group == 3)
            })
            .map(|(handle, _)| handle)
            .collect::<Vec<_>>();
        assert_eq!(evidence_globals.len(), 6);
        let entry_info = |name: &str| {
            let index = module
                .entry_points
                .iter()
                .position(|entry| entry.name == name)
                .unwrap_or_else(|| panic!("missing world entry {name}"));
            info.get_entry_point(index)
        };
        // Evidence textures are attachments while this entry executes. Its
        // call graph must never require them as sampled resources.
        for &global in &evidence_globals {
            assert!(entry_info("fs_rough_reflection_evidence")[global].is_empty());
            let binding = module.global_variables[global]
                .binding
                .as_ref()
                .unwrap()
                .binding;
            assert_eq!(
                entry_info("fs_reflection_route")[global].is_empty(),
                binding == 5,
                "route classifier must sample evidence, never its own route attachment: binding {binding}"
            );
        }
        if ordinary {
            for entry in [
                "fs_main",
                "fs_main_gbuffer",
                "fs_main_mrt",
                "fs_capture_opaque",
                "fs_transmissive",
                "fs_transmissive_mrt",
                "fs_transmissive_planar_cached_mrt",
                "fs_transmissive_planar_full_mrt",
            ] {
                for &global in &evidence_globals {
                    assert!(
                        entry_info(entry)[global].is_empty(),
                        "ordinary entry {entry} requires missing group 3"
                    );
                }
            }
        } else {
            for &global in &evidence_globals {
                assert!(
                    !entry_info("fs_main_gbuffer")[global].is_empty(),
                    "coarse surface variant lost rough evidence binding"
                );
            }
        }
    }
}

fn reachable_function_names(
    module: &wgpu::naga::Module,
    entry: &str,
) -> std::collections::HashSet<String> {
    // A source-specialized const guard removes a whole call graph. Respect
    // its branch and terminating return without assuming runtime uniforms or
    // default override values are constant in an actual pipeline.
    fn constant_bool(
        expression: wgpu::naga::Handle<wgpu::naga::Expression>,
        expressions: &wgpu::naga::Arena<wgpu::naga::Expression>,
        module: &wgpu::naga::Module,
    ) -> Option<bool> {
        use wgpu::naga::{BinaryOperator, Expression, Literal, UnaryOperator};
        match &expressions[expression] {
            Expression::Literal(Literal::Bool(value)) => Some(*value),
            Expression::Constant(constant) => constant_bool(
                module.constants[*constant].init,
                &module.global_expressions,
                module,
            ),
            Expression::Unary { op: UnaryOperator::LogicalNot, expr } => {
                constant_bool(*expr, expressions, module).map(|value| !value)
            }
            Expression::Binary { op, left, right } => {
                let left = constant_bool(*left,expressions,module);
                let right = constant_bool(*right,expressions,module);
                match op {
                    BinaryOperator::LogicalAnd => {
                        if left == Some(false) || right == Some(false) { Some(false) }
                        else if left == Some(true) && right == Some(true) { Some(true) }
                        else { None }
                    }
                    BinaryOperator::LogicalOr => {
                        if left == Some(true) || right == Some(true) { Some(true) }
                        else if left == Some(false) && right == Some(false) { Some(false) }
                        else { None }
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn visit(
        block: &wgpu::naga::Block,
        function: &wgpu::naga::Function,
        module: &wgpu::naga::Module,
        visited: &mut std::collections::HashSet<wgpu::naga::Handle<wgpu::naga::Function>>,
    ) -> bool {
        use wgpu::naga::Statement;
        for statement in block {
            match statement {
                Statement::Call { function: callee, .. } => {
                    if visited.insert(*callee) {
                        let callee = &module.functions[*callee];
                        visit(&callee.body, callee, module, visited);
                    }
                }
                Statement::Block(inner) => {
                    if !visit(inner, function, module, visited) { return false; }
                }
                Statement::If { condition, accept, reject } => {
                    let falls_through = match constant_bool(*condition, &function.expressions, module) {
                        Some(true) => visit(accept, function, module, visited),
                        Some(false) => visit(reject, function, module, visited),
                        None => {
                            let accepted = visit(accept, function, module, visited);
                            let rejected = visit(reject, function, module, visited);
                            accepted || rejected
                        }
                    };
                    if !falls_through { return false; }
                }
                Statement::Switch { cases, .. } => {
                    for case in cases {
                        visit(&case.body, function, module, visited);
                    }
                }
                Statement::Loop {
                    body, continuing, ..
                } => {
                    visit(body, function, module, visited);
                    visit(continuing, function, module, visited);
                }
                Statement::Return { .. } | Statement::Kill | Statement::Break | Statement::Continue => return false,
                _ => {}
            }
        }
        true
    }
    let entry = module
        .entry_points
        .iter()
        .find(|candidate| candidate.name == entry)
        .unwrap_or_else(|| panic!("missing shader entry {entry}"));
    let mut visited = std::collections::HashSet::new();
    visit(&entry.function.body, &entry.function, module, &mut visited);
    visited
        .into_iter()
        .filter_map(|handle| module.functions[handle].name.clone())
        .collect()
}

#[test]
fn physical_style_contraction_removes_art_directed_calls_and_retains_physical_controls() {
    let universal = super::super::WGPU_WORLD_SHADER.as_str();
    assert_eq!(universal.matches("const PHYSICAL_STYLE_ONLY: bool = false;").count(),1);
    for physical_only in [false,true] {
        let source = super::super::shader_specialization::physical_style_shader_source(universal,physical_only);
        if !physical_only { assert_eq!(source,universal,"the disabled experiment preserves the universal source"); }
        let module = wgpu::naga::front::wgsl::parse_str(&source).expect("physical style variant parses");
        wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(),wgpu::naga::valid::Capabilities::all())
            .validate(&module).expect("physical style variant validates");
        for entry in ["fs_main_gbuffer","fs_main_mrt","fs_transmissive_mrt","fs_capture_opaque",
            "fs_transmissive_planar_cached_mrt","fs_transmissive_planar_full_mrt"] {
            let functions = reachable_function_names(&module,entry);
            for function in ["shade_cel","stylized_intensity","shade_stylized","toon_intensity",
                "clay_base_color","clay_metallic","clay_roughness"] {
                assert_eq!(functions.contains(function),!physical_only,
                    "physical_only={physical_only}, entry={entry}, function={function}");
            }
            for function in ["shade_physical","shade_wrapped_physical","sheen_brdf",
                "material_coat_fresnel","trace_geometry_reflection","surface_glass_visibility",
                "authored_area_light_radiance","inverse_display_curve"] {
                assert!(functions.contains(function),
                    "physical_only={physical_only} must retain {function} in {entry}");
            }
        }
        for entry in ["vs_main","vs_outline"] {
            let functions = reachable_function_names(&module,entry);
            assert_eq!(functions.contains("cel_basis"),!physical_only,
                "physical_only={physical_only} must contract only optional cel vertex work in {entry}");
            assert!(functions.contains("bone_transform"));
            assert!(functions.contains("vegetation_deform_at"));
        }
        for entry in ["fs_reflection_route","fs_rough_reflection_evidence"] {
            assert_eq!(reachable_function_names(&module,entry).contains("clay_roughness"),!physical_only,
                "physical_only={physical_only} must also specialize rough/classifier entry {entry}");
        }
        let query_source = super::super::shader_specialization::physical_style_shader_source(
            &reflection_query_shader_source(false),physical_only);
        let query = wgpu::naga::front::wgsl::parse_str(&query_source).expect("physical compact query variant parses");
        wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(),wgpu::naga::valid::Capabilities::all())
            .validate(&query).expect("physical compact query variant validates");
        assert_eq!(reachable_function_names(&query,"fs_reflection_query_inputs").contains("clay_roughness"),!physical_only);
    }
}

#[test]
fn whole_scene_coverage_proof_removes_only_cutoff_alpha_evaluation() {
    let original = super::super::WGPU_WORLD_SHADER.as_str();
    let declaration = "override HYBRID_ALPHA_EVALUATION_ENABLED: bool = true;";
    assert_eq!(original.matches(declaration).count(),1,
        "unmodified and retained fixtures must default to exact alpha evaluation");
    for enabled in [false,true] {
        for physical_only in [false,true] {
            let source = super::super::shader_specialization::physical_style_shader_source(original,physical_only)
                .replace(declaration,&format!("const HYBRID_ALPHA_EVALUATION_ENABLED: bool = {enabled};"))
                .replace("override HYBRID_SOLID_TRANSPORT: bool = true;","const HYBRID_SOLID_TRANSPORT: bool = true;")
                .replace("override PRIMARY_GLASS_SHADOWS_ENABLED: bool = true;","const PRIMARY_GLASS_SHADOWS_ENABLED: bool = true;");
            let (module,_) = query_shader_module(&source);
            for entry in ["fs_main_gbuffer","fs_transmissive_mrt","fs_capture_opaque"] {
                let functions = reachable_function_names(&module,entry);
                for alpha in ["hybrid_alpha","hybrid_texture_alpha","hybrid_texel_alpha"] {
                    assert_eq!(functions.contains(alpha),enabled,
                        "alpha_enabled={enabled}, physical_only={physical_only}, entry={entry}, function={alpha}");
                }
                for retained in ["hybrid_surface","hybrid_texture","hybrid_uv","hybrid_srgb_decode",
                    "hybrid_intersect_filtered","hybrid_intersect_triangle","hybrid_triangle_matches",
                    "hybrid_ray_box_near","hybrid_child_near","hybrid_consume_node_visit",
                    "hybrid_trace_visibility","hybrid_visibility_surface"] {
                    assert!(functions.contains(retained),
                        "alpha_enabled={enabled}, physical_only={physical_only}: {entry} lost {retained}");
                }
                if entry == "fs_transmissive_mrt" {
                    assert!(functions.contains("primary_solid_interface_visible"),
                        "coverage specialization must retain nearest solid ownership");
                }
            }
        }
    }
}

#[test]
fn routed_cached_surface_cannot_reach_primary_geometry_reflection_queries() {
    let source = super::super::WGPU_WORLD_SHADER.as_str();
    let query_call = "/* primary reflection query */trace_geometry_reflection(";
    assert_eq!(source.matches(query_call).count(), 2);
    let primary_glass_declaration = "override PRIMARY_GLASS_SHADOWS_ENABLED: bool = true;";
    assert_eq!(source.matches(primary_glass_declaration).count(), 1);
    for (fast, primary_glass) in [(false, false), (false, true), (true, false), (true, true)] {
        let source = if fast {
            source.replace(query_call, "rough_reflection_unavailable_query(")
        } else {
            source.to_owned()
        };
        let source = source.replace(primary_glass_declaration,
            &format!("const PRIMARY_GLASS_SHADOWS_ENABLED: bool = {primary_glass};"));
        let module = wgpu::naga::front::wgsl::parse_str(&source).expect("routed surface parses");
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("routed surface validates");
        let functions = reachable_function_names(&module, "fs_main_gbuffer");
        for query in ["trace_geometry_reflection", "hybrid_trace_reflection_ray"] {
            assert_eq!(
                functions.contains(query),
                !fast,
                "primary query reachability differs for fast={fast}: {query}"
            );
        }
        assert!(functions.contains("surface_glass_visibility"));
        // Removing primary reflection queries retains casting-glass shadows;
        // a separate complete-scene proof removes that traversal only when no
        // casting transmissive draw can populate the primary visibility tree.
        // The full reflection shader also shades reflected receivers. Their
        // retained-map path still reaches this helper, whose static empty-tree
        // branch removes colored boundary work under the same complete proof.
        assert_eq!(functions.contains("hybrid_transmission_visibility"), primary_glass || !fast,
            "primary glass visibility differs for fast={fast}, primary_glass={primary_glass}");
        if !primary_glass {
            for query in ["hybrid_trace_visibility", "hybrid_trace_slab_visibility", "hybrid_trace_mixed_visibility"] {
                assert!(!functions.contains(query),
                    "shader without casting glass still reaches shadow boundary work {query}");
            }
            if !fast {
                assert!(functions.contains("hybrid_light_visibility"));
                assert!(functions.contains("hybrid_intersect_filtered"),
                    "reflected receivers must retain complete opaque shadow fallback");
                assert!(functions.contains("hybrid_trace_transmission_core"),
                    "noncasting glass must remain in reflection/refraction transport");
            }
        }
        if fast && !primary_glass {
            for query in [
                "hybrid_intersect", "hybrid_intersect_filtered", "hybrid_trace_visibility",
                "hybrid_trace_slab_visibility", "hybrid_trace_mixed_visibility",
                "trace_geometry_transmission", "hybrid_trace_transmission_core", "hybrid_hit_local",
            ] {
                assert!(!functions.contains(query),
                    "cached shader without casting glass still reaches {query}");
            }
        }
        let classifier = reachable_function_names(&module, "fs_reflection_route");
        for query in [
            "trace_geometry_reflection",
            "hybrid_trace_reflection_ray",
            "hybrid_transmission_visibility",
        ] {
            assert!(
                !classifier.contains(query),
                "route classifier unexpectedly shades/traces {query}"
            );
        }
    }
}

#[test]
fn casting_glass_proof_removes_only_shadow_boundary_transport() {
    let source = super::super::WGPU_WORLD_SHADER.as_str();
    for solid in [false, true] {
        for casting_glass in [false, true] {
            let specialized = source
                .replace("override HYBRID_SOLID_TRANSPORT: bool = true;",
                    &format!("const HYBRID_SOLID_TRANSPORT: bool = {solid};"))
                .replace("override PRIMARY_GLASS_SHADOWS_ENABLED: bool = true;",
                    &format!("const PRIMARY_GLASS_SHADOWS_ENABLED: bool = {casting_glass};"));
            let module = wgpu::naga::front::wgsl::parse_str(&specialized)
                .expect("shadow-specialized world parses");
            wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::all(),
            ).validate(&module).expect("shadow-specialized world validates");
            let functions = reachable_function_names(&module, "fs_main_gbuffer");
            assert!(functions.contains("hybrid_intersect_filtered"));
            assert!(functions.contains("hybrid_light_visibility"));
            assert!(functions.contains("hybrid_trace_reflection_ray"));
            assert!(functions.contains("hybrid_trace_transmission_core"));
            assert_eq!(functions.contains("hybrid_trace_slab_transmission"), !solid);
            assert_eq!(functions.contains("hybrid_trace_mixed_transmission"), solid);
            assert_eq!(functions.contains("hybrid_trace_visibility"), casting_glass);
            assert_eq!(functions.contains("hybrid_trace_slab_visibility"), casting_glass && !solid);
            assert_eq!(functions.contains("hybrid_trace_mixed_visibility"), casting_glass && solid);
        }
    }
}

fn reflection_query_shader_source(cached: bool) -> String {
    let source = super::super::WGPU_WORLD_SHADER.as_str();
    let sample_marker = "/* sample completed query */rough_reflection_unavailable(";
    let route_marker = "/* completed query route */rough_reflection_default_route(";
    let primary_marker = "/* primary reflection query */trace_geometry_reflection(";
    assert_eq!(source.matches(sample_marker).count(), 1);
    assert_eq!(source.matches(route_marker).count(), 1);
    assert_eq!(source.matches(primary_marker).count(), 2);
    let mut source = source
        .replace(sample_marker, "sample_completed_reflection_query(")
        .replace(route_marker, "completed_reflection_query_route(");
    source.push_str(include_str!("../shaders/reflection_query_compute.wgsl"));
    if cached {
        source = source.replace(primary_marker, "rough_reflection_unavailable_query(");
    }
    // Match the supported opt-in route: no solid transport or primary casting
    // glass, with unchanged geometry reflection and enabled incident evidence.
    for (name, value) in [
        ("HYBRID_SOLID_TRANSPORT", false),
        ("PRIMARY_GLASS_SHADOWS_ENABLED", false),
        ("GEOMETRY_REFLECTION_ENABLED", true),
        ("COARSE_REFLECTION_ENABLED", true),
        ("COARSE_ROUTE_FAST_ONLY", cached),
        ("COARSE_ROUTE_FULL_ONLY", !cached),
        ("REFLECTION_QUERY_ENABLED", true),
    ] {
        let old_default = matches!(
            name,
            "HYBRID_SOLID_TRANSPORT" | "PRIMARY_GLASS_SHADOWS_ENABLED" | "GEOMETRY_REFLECTION_ENABLED"
        );
        let declaration = format!("override {name}: bool = {old_default};");
        assert_eq!(source.matches(&declaration).count(), 1);
        source = source.replace(&declaration, &format!("const {name}: bool = {value};"));
    }
    source
}

fn query_shader_module(source: &str) -> (wgpu::naga::Module, wgpu::naga::valid::ModuleInfo) {
    let module = wgpu::naga::front::wgsl::parse_str(source)
        .unwrap_or_else(|error| panic!("{}", error.emit_to_string(source)));
    let info = wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("{}", error.emit_to_string(source)));
    (module, info)
}

fn entry_bound_resources(
    module: &wgpu::naga::Module,
    info: &wgpu::naga::valid::ModuleInfo,
    name: &str,
) -> std::collections::BTreeSet<(u32, u32)> {
    let index = module.entry_points.iter().position(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("missing shader entry {name}"));
    module.global_variables.iter().filter_map(|(handle, global)| {
        let binding = global.binding.as_ref()?;
        (!info.get_entry_point(index)[handle].is_empty()).then_some((binding.group, binding.binding))
    }).collect()
}

#[test]
fn experimental_planar_slab_pair_keeps_snapshots_and_removes_cached_bvh_queries() {
    let original = super::super::WGPU_WORLD_SHADER.as_str();
    for cached in [false,true] {
        // Match constructor source specialization, rather than assume a
        // pipeline's override default proves the private BVH graph absent.
        let source = original
            .replace("/* sample rough evidence */sample_rough_reflection_evidence(",
                "rough_reflection_unavailable(")
            .replace("/* sample reflection route */sample_primary_reflection_route(",
                "rough_reflection_default_route(")
            .replace("override HYBRID_SOLID_TRANSPORT: bool = true;",
                "const HYBRID_SOLID_TRANSPORT: bool = false;")
            .replace("override PRIMARY_GLASS_SHADOWS_ENABLED: bool = true;",
                "const PRIMARY_GLASS_SHADOWS_ENABLED: bool = false;")
            .replace("override GEOMETRY_REFLECTION_ENABLED: bool = true;",
                &format!("const GEOMETRY_REFLECTION_ENABLED: bool = {};",!cached));
        let (module,info) = query_shader_module(&source);
        let entry = if cached { "fs_transmissive_planar_cached_mrt" }
            else { "fs_transmissive_planar_full_mrt" };
        let resources = entry_bound_resources(&module,&info,entry);
        assert!(resources.iter().all(|(group,_)| *group < 3),
            "slab entry {entry} requires a fourth bind group");
        for binding in [(2,0),(2,1),(2,2),(0,9),(0,11)] {
            assert!(resources.contains(&binding),
                "slab entry {entry} lost snapshot or planar binding {binding:?}");
        }
        let functions = reachable_function_names(&module,entry);
        for retained in ["initialize_surface_gradients","transmissive_snapshot_visible",
            "surface_primary_visible","transmissive_planar_cached","surface_mapped_normal",
            "surface_planar_projection","planar_surface_reuse_allowed","shade_transmissive_accepted",
            "transmission_scene_view_depth","transmission_project"] {
            assert!(functions.contains(retained),"slab entry {entry} lost {retained}");
        }
        assert_eq!(functions.contains("trace_geometry_reflection"),!cached);
        for solid in ["primary_solid_interface_visible","trace_geometry_transmission"] {
            assert!(!functions.contains(solid),"slab-only proof retained solid query {solid}");
        }
        if cached {
            for query in ["hybrid_intersect","hybrid_intersect_filtered","hybrid_hit_local",
                "hybrid_trace_reflection_ray","hybrid_trace_visibility","hybrid_transmission_visibility",
                "hybrid_trace_transmission_core","hybrid_trace_slab_visibility","hybrid_trace_mixed_visibility"] {
                assert!(!functions.contains(query),"cached slab entry still reaches BVH query {query}");
            }
        } else {
            assert!(functions.contains("hybrid_trace_reflection_ray"));
            assert!(functions.contains("hybrid_intersect_filtered"));
        }
        // Shared predicates ensure route and accepted shading use the same
        // projection and normal mapping, including explicit quad gradients.
        let route = module.functions.iter().find_map(|(_,function)|
            (function.name.as_deref()==Some("transmissive_planar_cached")).then_some(function)).unwrap();
        for (_,expression) in route.expressions.iter() {
            if let wgpu::naga::Expression::ImageSample { level,.. } = expression {
                assert!(matches!(level,wgpu::naga::SampleLevel::Gradient { .. }),
                    "slab route must use the same explicit material gradients as shading");
            }
        }
    }
}

#[test]
fn opt_in_query_surface_variants_keep_stage_resources_and_draw_ownership() {
    for cached in [false, true] {
        let source = reflection_query_shader_source(cached);
        let (module, info) = query_shader_module(&source);
        let group_three = |entry| entry_bound_resources(&module, &info, entry).into_iter()
            .filter_map(|(group, binding)| (group == 3).then_some(binding))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(group_three("fs_main_gbuffer"), [0, 1, 2, 3, 4, 5, 6, 7, 9].into());
        assert_eq!(group_three("fs_reflection_route"), [0, 1, 2, 3, 4, 6, 7, 9].into());
        assert!(group_three("fs_rough_reflection_evidence").is_empty());
        // The producer reads the original route and reaches completed-query
        // compatibility checks. Its input bind group must supply separate dummy
        // query targets; binding its active MRT textures would create feedback.
        assert_eq!(group_three("fs_reflection_query_inputs"), [5, 6, 7, 9].into());
        let output = module.global_variables.iter().find_map(|(handle, global)| {
            (global.name.as_deref() == Some("reflection_query_outputs")).then_some(handle)
        }).expect("query output storage");
        for entry in ["fs_main_gbuffer", "fs_reflection_route", "fs_reflection_query_inputs"] {
            let index = module.entry_points.iter().position(|candidate| candidate.name == entry).unwrap();
            assert!(!info.get_entry_point(index)[output].contains(wgpu::naga::valid::GlobalUse::WRITE),
                "fragment entry {entry} writes completed query output");
            let functions = reachable_function_names(&module, entry);
            assert!(functions.contains("sample_completed_reflection_query"));
            if entry == "fs_main_gbuffer" {
                assert_eq!(functions.contains("trace_geometry_reflection"), !cached);
            } else {
                assert!(!functions.contains("trace_geometry_reflection"));
                assert!(!functions.contains("hybrid_hit_local"));
            }
        }
        // These six declarations are exposed by the shared explicit layouts,
        // even when an individual entry reads only a subset. Four storage slots
        // cannot support the opt-in layout; the device guard reserves six.
        let storage = module.global_variables.iter().filter_map(|(_, global)| {
            if !matches!(global.space, wgpu::naga::AddressSpace::Storage { .. }) { return None; }
            let binding = global.binding.as_ref().expect("bound storage resource");
            Some((binding.group, binding.binding))
        }).collect::<std::collections::BTreeSet<_>>();
        assert_eq!(storage, [(0, 0), (0, 1), (1, 8), (1, 20), (3, 8), (3, 9)].into());
    }
}

#[test]
fn opt_in_query_compute_entries_are_derivative_free_and_use_disjoint_layouts() {
    for cached in [false, true] {
        let source = reflection_query_shader_source(cached);
        let (module, info) = query_shader_module(&source);
        let compact = "cs_compact_reflection_queries";
        let trace = "cs_trace_reflection_queries";
        assert_eq!(entry_bound_resources(&module, &info, compact),
            [(3, 6), (3, 7), (3, 8), (3, 9)].into());
        let mut trace_bindings = [(2, 0), (3, 6), (3, 7), (3, 8), (3, 9)]
            .into_iter().collect::<std::collections::BTreeSet<_>>();
        trace_bindings.extend([0, 1, 2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 20].map(|binding| (1, binding)));
        assert_eq!(entry_bound_resources(&module, &info, trace), trace_bindings);
        for name in [compact, trace] {
            let index = module.entry_points.iter().position(|entry| entry.name == name).unwrap();
            let entry = &module.entry_points[index];
            assert_eq!(entry.stage, wgpu::naga::ShaderStage::Compute);
            assert_eq!(entry.workgroup_size, [64, 1, 1]);
            let functions = reachable_function_names(&module, name);
            assert_eq!(functions.contains("trace_geometry_reflection"), name == trace);
            assert!(!functions.contains("hybrid_trace_mixed_transmission"));
            assert!(!functions.contains("hybrid_trace_mixed_visibility"));
            let expression_sets = std::iter::once((&entry.function, name.to_owned())).chain(
                module.functions.iter().filter_map(|(_, function)| {
                    let function_name = function.name.as_ref()?;
                    functions.contains(function_name).then(|| (function, function_name.clone()))
                })
            );
            for (function, function_name) in expression_sets {
                for (_, expression) in function.expressions.iter() {
                    assert!(!matches!(expression, wgpu::naga::Expression::Derivative { .. }),
                        "{name} reaches a derivative through {function_name}");
                    if let wgpu::naga::Expression::ImageSample { level, .. } = expression {
                        assert!(!matches!(level, wgpu::naga::SampleLevel::Auto | wgpu::naga::SampleLevel::Bias(_)),
                            "{name} reaches implicit-LOD sampling through {function_name}");
                    }
                }
            }
            for (handle, global) in module.global_variables.iter() {
                match global.name.as_deref() {
                    Some("instance_params" | "bones" | "opaque_scene_texture" | "opaque_scene_depth") => {
                        assert!(info.get_entry_point(index)[handle].is_empty(),
                            "compute entry {name} unexpectedly needs raster resource {:?}", global.name);
                    }
                    Some("reflection_query_dispatch") => {
                        assert_eq!(info.get_entry_point(index)[handle].is_empty(), name == compact,
                            "only tracing may use the dynamic first-index uniform");
                    }
                    _ => {}
                }
            }
        }
    }
}
