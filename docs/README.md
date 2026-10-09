<!-- ========================================= -->
<!-- ========================================= -->
<!-- docs/README.md -->

# MotionLoom documentation index

Start with the [package overview](../README.md) for installation and a quick
start. This index routes humans and AI callers to task workflows and API
contracts. Use `motionloom::api` as the recommended integration surface;
MotionLoom is headless and the DSL is the source of truth for playback.

## Choose a reading path

| Task | Read in this order |
| --- | --- |
| Integrate a Rust or WASM host | [Public API](PUBLIC_API.md), then the relevant feature guide below |
| Choose an AI workflow across features | [MotionLoom entry skill](../skills/motionloom/SKILL.md), then the task's feature guides |
| Author or repair DSL with an AI caller | [LLM authoring](LLM_AUTHORING.md), then the relevant API contract |
| Bind an imported humanoid GLB | [Humanoid binding](HUMANOID_BINDING.md#seven-step-api-workflow) for the seven-step workflow, [Rig authoring](RIG_AUTHORING.md) for typed calls, then [Rig diagnostics](RIG_DIAGNOSTICS.md) for Scene playback evidence |
| Build or edit character geometry | [Character authoring](CHARACTER_AUTHORING.md), then [Mesh authoring](MESH_AUTHORING.md) for general mesh operations |
| Fit supported semantic Head parameters | [Head reference fitting](HEAD_REFERENCE_FITTING.md), then [Facial cages](FACIAL_CAGES.md) |
| Fit a MeshAsset to image references | [Image to MeshAsset workflow](IMAGE_TO_MESHASSET.md), [Mesh authoring](MESH_AUTHORING.md), then [Mesh reference fitting](MESH_REFERENCE_FITTING.md) |
| Configure rendering and preview | [Render style](RENDER_STYLE.md), then [Immediate preview](IMMEDIATE_PREVIEW.md); use [Weaver](../src/weaver/README.md) for the opt-in native rendering path |

## API and AI authoring

| Guide | When to read it |
| --- | --- |
| [Public API](PUBLIC_API.md) | Public API layers, native/WASM integration, supported operations and stability policy |
| [LLM authoring](LLM_AUTHORING.md) | DSL rules, evidence-based authoring and the AI repair protocol |

## Characters, skeletons and facial geometry

| Guide | When to read it |
| --- | --- |
| [Character authoring](CHARACTER_AUTHORING.md) | Editable starter meshes, semantic selections, attachments, revisions and persistence |
| [Humanoid binding](HUMANOID_BINDING.md) | Mesh-only anatomy fitting, the seven-step binding workflow, head constraints and UV preservation/audit responsibilities |
| [Rig authoring](RIG_AUTHORING.md) | Humanoid65 requests, reviewed weight refinement, immutable candidates, verification and export APIs |
| [Rig diagnostics](RIG_DIAGNOSTICS.md) | Actual Scene pose stages, action execution, coordinate frames and pose comparisons |
| [Head reference fitting](HEAD_REFERENCE_FITTING.md) | Typed head parameters, multiview fitting and comparison overlays |
| [Face components](FACE_COMPONENTS.md) | Face component geometry, textures, coordinates and migration rules |
| [Facial cages](FACIAL_CAGES.md) | Editable facial and subdivision control cages |
| [Hair cards](HAIR_CARDS.md) | Guide-based hair construction, orientation, normals and tapering |

## Meshes and geometry

| Guide | When to read it |
| --- | --- |
| [Geometry assets](GEOMETRY_ASSETS.md) | Canonical generated-geometry structure and supported asset design |
| [Image to MeshAsset](IMAGE_TO_MESHASSET.md) | Reference preparation, measured fitting, proposal review, acceptance and delivery workflow |
| [Mesh authoring](MESH_AUTHORING.md) | GeometryRecipe operations, semantic regions and revision-safe mesh edits |
| [Mesh reference fitting](MESH_REFERENCE_FITTING.md) | Image analysis, frozen-view evaluation and fingerprint-safe mesh proposals |
| [Geometry tooling](GEOMETRY_TOOLING.md) | Static geometry inspection, UV checks and GLB export |

## Rendering and effects

| Guide | When to read it |
| --- | --- |
| [Render style](RENDER_STYLE.md) | Scene styles, cel shading, outlines, material controls and anti-aliasing |
| [Material layers](MATERIAL_LAYERS.md) | Fabric sheen, dielectric clearcoat, energy accounting and supported glTF factors |
| [Per-light shadows](PER_LIGHT_SHADOWS.md) | Independent emitter visibility, source-size softness, cube faces, retained caches and profile budgets |
| [Indoor lighting and reflections](BAKED_LIGHTING.md) | CPU irradiance baking, local HDR probes, planar mirrors and ordered glass |
| [Reflections and closed glass](HYBRID_REFLECTIONS.md) | Raster Preview reflection/refraction, native geometry reference, Weaver optics and platform limits |
| [Immediate preview](IMMEDIATE_PREVIEW.md) | Retained GPU preview, host quality profiles, capability reports and frame metrics |
| [Procedural surface](PROCEDURAL_SURFACE.md) | GPU procedural surface effects and their parameter contract |
| [Volumetrics](VOLUMETRICS.md) | Atmosphere media, froxel transport and volume controls |
| [Weaver](../src/weaver/README.md) | Opt-in native rendering, camera optics and export capabilities |

## Audio and tools

| Guide | When to read it |
| --- | --- |
| [Audio](AUDIO.md) | Native/WASM audio timing, editing semantics and adapter boundaries |
| [Formatting](FORMATTING.md) | Shared DSL formatting through Rust, WASM and native tooling |
| [CLI](CLI.md) | Native command-line rendering, export and formatting for hosts that choose the CLI |

## AI skills

One universal skill selects task-specific documents. Workflows, API contracts
and request examples stay in `docs/`.
The API remains usable directly from these docs without loading a skill.

| Skill | Use it for |
| --- | --- |
| [MotionLoom](../skills/motionloom/SKILL.md) | Universal AI entry point that selects task-specific workflow and API documents |

For image fitting, the entry skill reads [Image to MeshAsset](IMAGE_TO_MESHASSET.md),
which links [Mesh authoring](MESH_AUTHORING.md) and
[Mesh reference fitting](MESH_REFERENCE_FITTING.md) for maintained API contracts.
That workflow does not govern semantic Head fitting or imported GLB binding.
[AGENTS.md](../AGENTS.md) retains coding-agent rules and skill registration;
it is separate from this user/API documentation index.

## Design documents

| Document | Status |
| --- | --- |
| [Humanoid pose and knee API](HUMANOID_POSE_API.md) | Proposed API design, not a callable implementation; use the active host's schema and [Rig authoring](RIG_AUTHORING.md) for implemented operations |

## Release history and examples

- [Changelog](../CHANGELOG.md)
- [Path DSL benchmark](../benchmarks/path-dsl/README.md)
- [Portable MotionLoom examples](https://github.com/LOVELYZOMBIEYHO/motionloom-example)
- [Rust API documentation](https://docs.rs/motionloom)
