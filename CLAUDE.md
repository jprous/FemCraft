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
- `cargo build`, `cargo test --workspace`, `cargo run` (default member is `apps/femcraft`).
- Stack: eframe / egui with the wgpu renderer, `glam` maths, WGSL shaders in `src/shaders/`.

## Code layout (Cargo workspace)

- `apps/femcraft` executable (window setup only).
- `crates/femcraft-ui` panels and viewport interaction (`app.rs`).
- `crates/femcraft-render` orbit camera (Z up), grid and snapping, wgpu line renderer, `shaders/`.
- `crates/femcraft-aster`, `-results` placeholders.
- egui / eframe 0.37 (panels are `egui::Panel`, `App::ui` instead of `update`).
- CI: `.github/workflows/ci.yml` (Windows build + test).
- `crates/femcraft-model` - no I/O. `kinds.rs`: every object kind (materials, sections,
  thicknesses, nodes, lines, members, surfaces, solids, 4 support kinds, load cases, 11 load
  kinds) with field descriptors (key, label, type/SI unit) that drive tables and dialogs.
  `model.rs`: `Model` of `ObjectId { kind, load_case, no }` -> `Object` (field values),
  typed `set` with dirty / removed tracking, geometry helpers. References are object numbers.
- `crates/femcraft-rfdb` - `.rf6` I/O. `archive.rs` ZIP / bare db / folder, raw-copies
  sidecars on save. `mapping.rs`: **all RFEM table and column names** (layout rules in its
  header; entries marked "guess" are unconfirmed). `document.rs`: `RfDocument::open` loads
  every kind generically (main row -> impl row -> list / part tables -> derived maps), resolves
  db ids to object numbers, reports missing fields; `save` writes changed main / impl / part
  columns. Read-only for now: list fields, type, library section / material properties.
  Not yet: adding / deleting objects.
- `rf6-catalog` research tool: `cargo run -p femcraft-rfdb --bin rf6-catalog -- file.rf6`
  (load report), `--kinds [KIND..]`, `--tables [FILTER..]`, `--rows TABLE [N]`,
  `--sql "SELECT .."`, `--objects KIND [N]`, `--edit OUT KIND NO[@LC] FIELD VALUE`.
- Tests: `tests/roundtrip.rs` (synthetic RFEM-shaped db), `tests/private_model.rs` (real model
  from `FEMCRAFT_TEST_MODEL` or `testdata/private/test.rf6`; skipped if absent).
- On the owner's PC antivirus quarantines the `femcraft_rfdb` lib unit-test exe
  ("Access is denied"); run `cargo test -p femcraft-rfdb --test roundtrip --test private_model`.

## RFEM `.rf6` facts

- ZIP containing `model.db` (SQLite) + XML sidecars (`general_data.xml`, `unit.xml`,
  `views.xml`, `mesh.xml`, ...), `preview.glb`, `format.txt` (program / format version).
- About 5,900 tables. Pattern: main table (`Node`, `Line`, `Member`, ...) with
  `impl_id` / `impl_table` -> detail table (`NodeImplStandard`, `LineImplPolyline`,
  `MemberImplBeam`, ...) -> child rows (`*_items`, `*_keys` / `*_values`, `owner_id`).
- Confirmed on RFEM 6.16 (details in `mapping.rs`): camelCase columns; object number is
  `userID` (`id` is the db key); refs are `<name>_id` holding db ids; loads -> load case via
  `parentModelObject_id`; list tables `<Impl>_<field>` keyed by `id` + `container_order`;
  springs in `SpringConstants` (owner = support impl); member load magnitudes in
  `<Impl>_magnitudes`; section values via `librarySection` -> `SectionData_parameterValues`;
  material values via `MaterialProperties_propertiesMap`. Library sections / materials store
  no name (RFEM generates it). Line / member lengths are not stored. `-inf` = "not set".
- Solver data and results are under `!solver/` in the ZIP (`Data.RFemX`, `RN_*` folders).
- Saving strategy: start from a database RFEM saved, change only edited rows, keep all
  unknown tables and sidecars byte-identical.

## Rules

- Never commit `.rf6` / `.rfc` or anything in `testdata/private/` (client data).
- Licence is MIT OR Apache-2.0. SolidCraft and BIMCraft code may be reused with attribution in
  `NOTICE`; CadCraft and Lapin for ideas; **Stabileo is AGPL - never copy its code.**
- Prefer small diffs; run `cargo build` and `cargo test` before committing.
