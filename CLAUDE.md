# FemCraft - notes for Claude Code

Read this instead of re-researching the project. Keep sessions short and focused: the owner
has a limited credit budget (see "Credit budget" in `docs/ROADMAP.md`).

## Project

Rust desktop FEM application: open / save RFEM 6 `.rf6` (ZIP) and `.rfc` files with RFEM's
table structure, model in a familiar FEM UI, solve with Salome-Meca / code_aster, view results.
Plan and milestone status: `docs/ROADMAP.md`.

## Build

- Windows, Rust stable, GNU toolchain (`x86_64-pc-windows-gnu`); MinGW binutils (WinLibs) on
  PATH provide `dlltool`.
- `cargo build`, `cargo test`, `cargo run`.
- Stack: eframe / egui with the wgpu renderer, `glam` maths, WGSL shaders in `src/shaders/`.

## Code layout

Cargo workspace: the app is still the root package; library crates live in `crates/`.

- `src/main.rs` window setup, `src/app.rs` UI and viewport interaction,
  `src/camera.rs` orbit camera (Z up), `src/grid.rs` grid and snapping,
  `src/viewport.rs` wgpu line renderer (egui paint callback).
- `crates/femcraft-model` - no I/O. `kinds.rs`: every object kind (materials, sections,
  thicknesses, nodes, lines, members, surfaces, solids, 4 support kinds, load cases, 11 load
  kinds) with field descriptors (key, label, type/SI unit) that drive tables and dialogs.
  `model.rs`: `Model` of `ObjectId { kind, load_case, no }` -> `Object` (field values),
  typed `set` with dirty / removed tracking, geometry helpers. References are object numbers.
- `crates/femcraft-rfdb` - `.rf6` I/O. `archive.rs` ZIP / bare db / folder, raw-copies
  sidecars on save. `mapping.rs`: **all RFEM table and column names (guesses until
  verified)**. `document.rs`: `RfDocument::open` loads every kind generically (main row ->
  impl row -> `{Table}_{field}` child tables), auto-detects whether refs store db ids or
  numbers, reports missing fields / unmapped columns; `save` writes only changed scalar fields.
  Not yet: editing list fields held in child tables, changing type, adding / deleting objects.
- `rf6-catalog` research tool: `cargo run -p femcraft-rfdb --bin rf6-catalog -- file.rf6`
  (load report), `--tables [FILTER..]`, `--rows TABLE [N]`.

## RFEM `.rf6` facts

- ZIP containing `model.db` (SQLite) + XML sidecars (`general_data.xml`, `unit.xml`,
  `views.xml`, `mesh.xml`, ...), `preview.glb`, `format.txt` (program / format version).
- About 5,900 tables. Pattern: main table (`Node`, `Line`, `Member`, ...) with
  `impl_id` / `impl_table` -> detail table (`NodeImplStandard`, `LineImplPolyline`,
  `MemberImplBeam`, ...) -> child rows (`*_items`, `*_keys` / `*_values`, `owner_id`).
- Saving strategy: start from a database RFEM saved, change only edited rows, keep all
  unknown tables and sidecars byte-identical.

## Rules

- Never commit `.rf6` / `.rfc` or anything in `testdata/private/` (client data).
- Licence is MIT OR Apache-2.0. SolidCraft and BIMCraft code may be reused with attribution in
  `NOTICE`; CadCraft and Lapin for ideas; **Stabileo is AGPL - never copy its code.**
- Prefer small diffs; run `cargo build` and `cargo test` before committing.
