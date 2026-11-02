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
- `crates/femcraft-model`, `-rfdb`, `-aster`, `-results` placeholders, filled from M1 on.
- egui / eframe 0.37 (panels are `egui::Panel`, `App::ui` instead of `update`).
- CI: `.github/workflows/ci.yml` (Windows build + test).

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
