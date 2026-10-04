## MotionLoom Crate Rules

These instructions apply to MotionLoom parser, renderer, examples, tests, and
documentation in this crate. They are intended for both Codex and other LLM
coding agents.

## Documentation Source of Truth

- `README.md` is the user-facing overview for humans and LLMs.
- `docs/README.md` is the documentation index with task-based reading paths.
- `docs/PUBLIC_API.md` defines the intended public API layers and stability policy.
- Feature guides, task workflows and API contracts live in `docs/`.
  `skills/motionloom/SKILL.md` is the sole universal AI entry point and routes
  tasks to those documents. Keep request schemas and API examples in feature
  guides rather than copying them into the skill.
- `src/lib.rs` and `src/api.rs` provide the docs.rs/rustdoc entry points.
- This `AGENTS.md` is only for coding-agent workflow rules. Do not duplicate
  full user documentation here; link or update the source documents above.
- Keep `motionloom::api` as the recommended stable integration surface.
- Keep `motionloom::experimental` public for advanced/editor APIs that may
  change faster than the stable surface.
- Keep crate-root re-exports for compatibility unless there is an explicit
  migration plan and all Anica usages are updated.

## Skill Routing

- Read `skills/motionloom/SKILL.md` completely before taking MotionLoom task
  actions, then choose the relevant workflow and API documents from its table.
- Read each selected workflow document completely. Load linked API contracts
  as needed; do not load all feature guides or combine unrelated workflows.
- The entry skill and workflows supplement this file and parent instructions;
  they do not override mandatory repository rules. Explicit user instructions
  take precedence over optional skill guidance.
- Maintain one universal entry skill. Add specialized workflows to `docs/`
  and update the entry table and documentation index in the same change.

### Skill Registry

- `motionloom`: universal entry point for MotionLoom DSL and headless API tasks.
  Read [skills/motionloom/SKILL.md](skills/motionloom/SKILL.md); it routes image
  fitting to [docs/IMAGE_TO_MESHASSET.md](docs/IMAGE_TO_MESHASSET.md) and other
  tasks to their own workflow/API guides.

The entry point selects a workflow; it does not require running every workflow.
Choose image-to-MeshAsset fitting only for measured explicit-cage fitting,
not merely because a task includes an image.

## DSL Authoring Rules

- Keep MotionLoom scripts parseable by the current parser. Do not invent syntax
  because it looks natural in XML/JSX.
- `curve(...)` points must use numeric keyframe values:
  `curve("time:value[:ease], time:value[:ease]")`.
- Do not put function calls such as `random(...)`, `sin(...)`, `cos(...)`, or
  `floor(...)` inside the value field of a `curve(...)` point.
- Use standalone expressions for procedural motion, for example:
  `x="random(-26,26,floor($time.sec*2)+17) + 28*sin($time.sec*0.8)"`.
- Use `curve(...)` for deterministic smooth numeric interpolation only.
- Do not use `$index` in scene expressions unless the parser/runtime explicitly
  supports it. Prefer `Repeat` attributes such as `xStep`, `yStep`,
  `rotationStep`, and `opacityStep` for per-instance variation.
- Do not animate string attributes such as `Text.value` or `Path.d` with
  `curve(...)`. Use fixed strings/paths, opacity, transform, trim, or numeric
  properties instead.
- Do not duplicate attributes on one node. For example, do not write both
  `x="600"` and `x={curve(...)}` on the same `<Group>`.
- Keep `<Present ... />` as the final direct child of `<Graph>`.

## Parser Changes

- Do not broaden parser behavior just to accept one generated example. First
  rewrite the example into the existing DSL style.
- If a new DSL feature is genuinely needed, propose it first with:
  - intended syntax,
  - parser/runtime impact,
  - backward compatibility risk,
  - at least one before/after example.
- Any approved DSL change must update parser tests, renderer behavior, README,
  and examples together.

## Repository and Build Rules

- The repository root is the `motionloom` package and Cargo workspace.
- `crates/motionloom-action-tool` is a separate native authoring tool that
  depends on the engine. Keep FBX and subprocess dependencies in that crate.
- Source comments and documentation must be English. Explain the purpose of
  nontrivial additions with concise comments.
- Source file header paths are relative to this repository, for example
  `src/api.rs` and `crates/motionloom-action-tool/src/lib.rs`.
- Use typed errors in reusable Rust APIs; do not introduce `Result<T, String>`.
- Run native checks from this repository, not from the Anica workspace.
- Keep required test fixtures inside this repository. Sibling showcase tests
  must be explicitly optional or ignored and document the required checkout.
- The engine must build without Anica, GPUI, or the landing page.
