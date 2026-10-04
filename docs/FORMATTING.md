# DSL formatting

MotionLoom has one Rust formatter, shared by the native CLI, Rust API, WASM,
source editors and generated asset/proposal output. Formatting needs no GPU,
asset loading, renderer, FFmpeg or network service.

## CLI

From the Anica repository:

```sh
cargo run -p motionloom --bin motionloom -- fmt main.motionloom
cargo run -p motionloom --bin motionloom -- fmt --check ../motionloom-example/showcase/
```

Install the local executable:

```sh
cargo install --path . --bin motionloom
motionloom fmt main.motionloom
motionloom fmt showcase/
motionloom fmt --check showcase/
motionloom fmt --help
```

`fmt` accepts one or more files or directories. Directory traversal selects
`.motionloom` files recursively, skips symlinks and deduplicates overlapping
paths. Explicit symlink arguments are rejected. Output is sorted by file path.

Without `--check`, changed files are written in place. All files are read and
formatted before any replacement; malformed input leaves the entire batch
unchanged. Outputs are prepared in the same directories, retaining permissions.
Original revisions are checked again before replacement. A later filesystem
failure can leave earlier replacements completed; this is not a transaction
across multiple files.

Exit codes are `0` for success, `1` for formatting differences with `--check`,
and `2` for argument, source structure or I/O errors. The CLI writes no files
with `--check`.

## Rules

- Indent by two spaces; put structural child tags on separate lines.
- Align closing tags with their opening tags.
- Wrap tags exceeding 120 Unicode characters with one attribute per line.
  Keep `Vertex` and `Face` on one line each for topology inspection.
- Preserve attribute order and the complete original value of each attribute.
  Long values are never split internally.
- Keep at most one blank line in structural gaps; do not insert decorative
  blank lines by tag type.
- Use LF for structural line breaks and one final newline for nonempty output.
- Preserve comments, strings, numeric spelling and expressions byte for byte.
  Their internal line endings and whitespace are retained.
- Preserve mixed-content elements, CDATA, literal `Text` bodies without a `value`
  attribute, and elements marked
  `xml:space="preserve"` as complete source ranges.

Formatting recognizes structure, not the capability catalog: unknown tag names
can be formatted without adding them to the parser. Syntax and semantic analysis
remain separate. Incomplete tags, quotes, expressions or mismatched nesting
produce a typed error with line and column; no partial output is returned.

## Rust API

```rust
use motionloom::api::{FormatError, FormatResult, format_dsl};

fn prepare(source: &str) -> Result<FormatResult, FormatError> {
    format_dsl(source)
}
```

`FormatResult` contains `source`, `changed` and `edits`. Each edit contains byte
offsets for Rust, UTF-16 offsets for browser selections, and replacement text.
Edits are nonoverlapping and ordered against the original source. Apply them
from last to first, or replace the source with the returned `source`.

Formatting is idempotent: formatting its output again returns `changed=false`.
The formatter reads original spans directly instead of serializing a scene AST.

The parser uses the same source scanner to separate adjacent structural tags
into logical lines. Compact and formatted documents therefore read the same
authored children, including multiple animation keys or retarget maps on one
line. Parser diagnostics map back to the original source lines; the stored raw
script and its fingerprint remain the caller's original text.

## WASM and editors

```javascript
const result = JSON.parse(wasm.motionloom_format_dsl(source));
editor.value = result.source;
```

The WASM function returns JSON and throws on a formatting error. Its JSON edit
fields are `startByte`, `endByte`, `startUtf16`, `endUtf16` and `replacement`.
Native and WASM call the same implementation.

The main source editor offers **Format DSL**, **Download DSL**, and **Format on
save** (enabled by default). Shift+Alt+F formats; Command/Ctrl+S downloads DSL.
The Action Editor provides Format DSL and Apply DSL in its source dialog, and
formats saved Action files when Format on save is enabled. Formatting joins each
editor's existing undo history and retains source selection and scroll.

Formatting changes the source fingerprint. Pending mesh proposals must be
reevaluated against the returned source. Geometry emitters, mesh proposals,
topology proposals, explicit geometry baking, head fitting and Action edits
format output through this same API. Returned proposal fingerprints describe
the formatted revision.

## Verification

- 669 Rust library tests passed. One existing test is ignored; one unrelated
  HTTP resolver test was excluded because its local server cannot bind in the
  sandbox.
- Three native CLI integration tests passed, covering exit codes, unchanged
  `--check` input, batch preflight, recursive symlink handling and permissions.
- Eight frontend tests passed, including real WASM/native CLI output parity,
  compact-document parsing, Unicode selections, undo transactions, stale source
  rejection and existing mesh editing behavior.
- All 146 showcase scene and action-library files passed whitespace-only edit,
  parsed-semantic equality, edit replay and idempotence checks. Their final
  `fmt --check` reports zero differences. All 99 main schemas were regenerated:
  98 clean, one tessellation review warning, zero repair or unrenderable results.
- S98's three DSL files retain every Vertex/Face token: 25,355 in `main`,
  25,355 in `main2`, and 25,555 in `main3`.
- The WASM package, landing site and native examples with the Weaver feature
  compile successfully.

The reusable corpus check is:

```sh
cargo run -p motionloom --example verify_dsl_formatting -- ../motionloom-example/showcase/
```
