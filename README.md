# MotionLoom

**An AI-native, headless engine for motion graphics, 3D scenes and character authoring.**

Build editable scenes with a text DSL. Use typed APIs to inspect geometry,
refine characters, verify bindings and render through Rust or WebAssembly.
The MotionLoom DSL is the source of truth for playback.

[Try the playground](https://lovelyzombieyho.github.io/anica-landing-page/motionloom/) ·
[Browse examples](https://github.com/LOVELYZOMBIEYHO/motionloom-example) ·
[Read the docs](https://github.com/LOVELYZOMBIEYHO/motionloom/blob/main/docs/README.md)

<!-- Draft media slot: add a thumbnail linking to a 20–40 second demo reel.
Show 2D typography, S99 procedural architecture and S105 mesh-only rig binding.
Publish the selected showcase assets before adding their public media links. -->

## What you can build

| Capability | Examples |
| --- | --- |
| Motion graphics | Animated text, vector shapes, masks, layers and GPU effects |
| 3D scenes | Generated geometry, imported GLB/glTF, materials, lighting and camera cuts |
| Character authoring | Editable templates, semantic mesh edits, reference fitting and attachments |
| Humanoid animation | 65-node skeleton generation, reviewed skin weights, reusable Actions and rig diagnostics |
| Rendering | Native wgpu preview, browser WebGPU, image sequences and video export |
| Audio | Source trims, timed clips, gain and pan animation, and export mixing |

MotionLoom can power editors, renderers and AI tools. Applications own their UI
and authoring sessions; the engine provides parsing, typed operations,
validation and rendering. [Anica](https://github.com/LOVELYZOMBIEYHO/anica)
is one application built with MotionLoom.

## See it in motion

- [Interactive playground](https://lovelyzombieyho.github.io/anica-landing-page/motionloom/) — edit DSL and preview it in a browser.
- [NEXT IN LINE](https://github.com/LOVELYZOMBIEYHO/motionloom-example/blob/main/showcase/s-000081/preview.mp4) — a short film combining authored 3D geometry, camera cuts, screen graphics and audio.
- [Example library](https://github.com/LOVELYZOMBIEYHO/motionloom-example) — focused core examples and complete showcases with editable source.

## Install

This README describes the current `main` branch, which requires **Rust 1.88 or newer**.
Use the Git dependency to access the current authoring and rendering APIs:

```toml
[dependencies]
motionloom = { git = "https://github.com/LOVELYZOMBIEYHO/motionloom", branch = "main" }
```

For the published release:

```toml
[dependencies]
motionloom = "0.1"
```

Use the [published API documentation](https://docs.rs/motionloom) for that release.
Feature availability can differ from `main`; pin a Git revision when you need
a fixed development version.

## Quick start

Clone the repository and open its bundled 3D ink study:

```sh
git clone https://github.com/LOVELYZOMBIEYHO/motionloom.git
cd motionloom
cargo run --release --example wgpu_live_preview -- examples/ink_wash.motionloom
```

Run the remaining commands from this `motionloom/` directory. The example host
provides the preview window.

To create your own animation, save this as `hello.motionloom`:

```xml
<Graph fps={30} duration="3s" size={[640,360]}>
  <Background color="#101827" />
  <Scene id="hello">
    <Timeline>
      <Track id="foreground" space="screen">
        <Sequence from="0s" duration="3s" out="hold">
          <Layer>
            <Circle x="320" y="150" radius="80" color="#B6FF35" />
            <Text id="headline" x="320" y="300" value="MotionLoom"
                  fontSize="36" align="center" color="#FFFFFF" />
          </Layer>
        </Sequence>
      </Track>
    </Timeline>
  </Scene>
  <AnimationTarget node="headline" property="opacity">
    <Key time="0s" value="0" />
    <Key time="0.7s" value="1" ease="ease_out" />
  </AnimationTarget>
  <Present from="hello" />
</Graph>
```

Preview the animation:

```sh
cargo run --release --example wgpu_live_preview -- hello.motionloom
```

## Embed in Rust

Start with `motionloom::api`. A host can parse the same DSL and render a frame:

```rust
use motionloom::api::{SceneRenderProfile, parse_graph_script, render_scene_graph_frame};

async fn render(source: &str) -> Result<(), Box<dyn std::error::Error>> {
    let graph = parse_graph_script(source)?;
    let frame = render_scene_graph_frame(&graph, 0, SceneRenderProfile::Gpu).await?;
    frame.save("frame.png")?;
    Ok(())
}
```

See the [public API guide](https://github.com/LOVELYZOMBIEYHO/motionloom/blob/main/docs/PUBLIC_API.md)
for retained preview, asset resolvers, WASM integration and export.
GPU rendering requires a suitable adapter. Native video encoding uses
host-supplied FFmpeg; single frames and image sequences can be exported independently.

## Author with AI

An AI caller can discover the DSL schema, propose changes through typed APIs
and use inspection and verification reports to guide the next edit:

```text
Scene brief or reference images
          ↓
Author DSL or propose API edits
          ↓
Inspect → refine → verify → repeat
          ↓
Preview and export the accepted result
```

Character tools live in `motionloom::api::character_authoring`. Its `rig` module
inspects imported mesh geometry, generates the `humanoid65_v1` hierarchy and
verifies candidate bindings against anatomical evidence and sampled actions.
New mesh-only bindings ignore the input model's skeleton and skin weights.

Callers review landmarks and selections, refine weights, and commit verified
candidates. Reports retain unresolved anatomy and failed checks for further editing.
See the [binding workflow](https://github.com/LOVELYZOMBIEYHO/motionloom/blob/main/docs/HUMANOID_BINDING.md)
and [rig API](https://github.com/LOVELYZOMBIEYHO/motionloom/blob/main/docs/RIG_AUTHORING.md).

The [universal MotionLoom skill](https://github.com/LOVELYZOMBIEYHO/motionloom/blob/main/skills/motionloom/SKILL.md)
routes AI tasks to the relevant guides. API callers can use those guides directly.

## Documentation

The [documentation index](https://github.com/LOVELYZOMBIEYHO/motionloom/blob/main/docs/README.md)
provides task-based reading paths and the complete feature guides.

| Task | Start here |
| --- | --- |
| Integrate Rust or WASM | [Public API](https://github.com/LOVELYZOMBIEYHO/motionloom/blob/main/docs/PUBLIC_API.md) |
| Generate or repair DSL | [LLM authoring](https://github.com/LOVELYZOMBIEYHO/motionloom/blob/main/docs/LLM_AUTHORING.md) |
| Build geometry or fit image references | [Geometry assets](https://github.com/LOVELYZOMBIEYHO/motionloom/blob/main/docs/GEOMETRY_ASSETS.md) · [Image fitting](https://github.com/LOVELYZOMBIEYHO/motionloom/blob/main/docs/IMAGE_TO_MESHASSET.md) |
| Edit characters or bind humanoids | [Character authoring](https://github.com/LOVELYZOMBIEYHO/motionloom/blob/main/docs/CHARACTER_AUTHORING.md) · [Humanoid binding](https://github.com/LOVELYZOMBIEYHO/motionloom/blob/main/docs/HUMANOID_BINDING.md) |
| Configure rendering | [Render style](https://github.com/LOVELYZOMBIEYHO/motionloom/blob/main/docs/RENDER_STYLE.md) · [Weaver](https://github.com/LOVELYZOMBIEYHO/motionloom/blob/main/src/weaver/README.md) |
| Edit audio or use native tools | [Audio](https://github.com/LOVELYZOMBIEYHO/motionloom/blob/main/docs/AUDIO.md) · [CLI](https://github.com/LOVELYZOMBIEYHO/motionloom/blob/main/docs/CLI.md) |

## Development

The repository root is both the engine package and a Cargo workspace.
`crates/motionloom-action-tool/` contains the separate native FBX/glTF importer.
Required regression fixtures are bundled; optional showcase tests use the
separate `motionloom-example` checkout.

```sh
cargo check --workspace --all-targets --all-features --locked
cargo test --workspace --locked
cargo check -p motionloom --target wasm32-unknown-unknown --locked
```

Native GPU rendering uses Metal on macOS, DX12 on Windows and Vulkan where
available on Linux. Browser hosts use WASM and WebGPU.
The optional `weaver` feature enables the native offline renderer.

MotionLoom is under active development. Public API layers and stability policy
are described in the API guide. Focused issues and contributions are welcome;
include a minimal `.motionloom` reproduction and your native or browser target.

## License

MotionLoom is licensed under [Apache-2.0](https://github.com/LOVELYZOMBIEYHO/motionloom/blob/main/LICENSE).
Example assets retain their individual licenses and attribution.
