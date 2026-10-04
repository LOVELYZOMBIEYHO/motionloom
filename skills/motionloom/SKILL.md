---
name: motionloom
description: Universal entry point for authoring, inspecting and validating MotionLoom DSL or using the headless Rust/WASM API. Select task-specific docs for scenes, image-to-MeshAsset fitting, character geometry, humanoid binding, animation, rendering and audio.
---

# MotionLoom

Use the public headless engine through `motionloom::api`. The accepted DSL is
the source of truth for playback; host-owned sessions, analyses and reports are
authoring evidence. A UI or ACP transport is not required.

## Locate the active contract

Start with the [documentation index](../../docs/README.md), then read only the
guides needed for the request. Use the checked-out implementation and the active
host's schema to resolve API availability. Documents marked as proposed designs
do not establish callable APIs. If this skill was copied outside the crate,
locate the matching MotionLoom package and resolve `docs/` there; do not use a
different engine version's contract silently.

## Choose the workflow

Read the task's workflow document completely before execution, then load only
the linked API contracts needed for that task. Workflows live in `docs/`; this
is the sole skill entry point for the crate.

| Request | Read and use |
| --- | --- |
| DSL generation or repair | [LLM authoring](../../docs/LLM_AUTHORING.md), [Public API](../../docs/PUBLIC_API.md); query the DSL schema and analyze the actual source |
| Character templates, proportions, semantic selections or attachments | [Character authoring](../../docs/CHARACTER_AUTHORING.md); use `api::character_authoring` and its candidate/revision checks |
| New binding for an imported humanoid GLB | [Humanoid binding](../../docs/HUMANOID_BINDING.md#seven-step-api-workflow), then [Rig authoring](../../docs/RIG_AUTHORING.md); use `api::character_authoring::rig` |
| Action playback, orientation or retargeting diagnosis | [Rig diagnostics](../../docs/RIG_DIAGNOSTICS.md); evaluate actual Scene poses at matching Action phases |
| Explicit MeshAsset fitting to raster references | Read [Image to MeshAsset](../../docs/IMAGE_TO_MESHASSET.md), then its linked mesh authoring/reference contracts; use the measured fitting workflow |
| Semantic head parameter fitting | [Head reference fitting](../../docs/HEAD_REFERENCE_FITTING.md) and [Facial cages](../../docs/FACIAL_CAGES.md); use `api::head_fitting` for supported Head parameters |
| General geometry construction, edits, UV inspection or GLB export | [Geometry assets](../../docs/GEOMETRY_ASSETS.md), [Mesh authoring](../../docs/MESH_AUTHORING.md), and [Geometry tooling](../../docs/GEOMETRY_TOOLING.md) as needed |
| Cel/material style, previews or offline rendering | [Render style](../../docs/RENDER_STYLE.md), [Immediate preview](../../docs/IMMEDIATE_PREVIEW.md), or [Weaver](../../src/weaver/README.md) for the requested rendering path |
| Audio timing or mixing | [Audio](../../docs/AUDIO.md) |

A supplied image does not by itself select MeshAsset fitting. Choose the
representation required by the task: a semantic Head, a character template, an
explicit cage, or an imported GLB may need different APIs. A request combining
modeling and binding can use those workflows in sequence.

## Preserve evidence through edits

Keep proposals tied to the current source, topology and revision. Inspect and
verify candidates before accepting them; an exported file alone is not proof
of a valid binding or a reference match. For new mesh-only GLB binding, ignore
the imported skeleton, skin weights and animations as fitting evidence, as
specified by the rig API. This rule does not apply to playback or inspection
of a user-requested existing rig.

Treat unresolved anatomy, missing measurements and failed checks as unresolved.
Use the guide's refinement loop and report the remaining failures. Preserve
editable DSL alongside requested derived exports, and distinguish numerical
checks from visual review and unavailable checks.
