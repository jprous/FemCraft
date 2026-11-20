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
- `crates/femcraft-ui`: multi-document. `document.rs` one open model (data, display settings, camera, selection, 3D view and its input; GPU buffers per document id); `app.rs` list of documents + active one, menu bar (File with Open Recent / Close, View, Windows, Help), minimisable ribbon (Home / View / Model / Loads tabs); layout, recent files, ribbon state and display settings (toggles, grid, axes; View > Reset Display) saved between sessions via eframe `persistence` (`%APPDATA%\FemCraft\data\app.ron`, key `femcraft.ui.v1`), docks and viewport interaction; `dock.rs` dockable layout on `egui_tiles` (a tab per model titled with its file name, a Start pane marking the model space, Display / Navigator / Tables panes following the active model as draggable tabs, resizable splits, closable panes reopened from View > Panels; `egui_dock` would add floating windows but targets egui 0.36); `panels.rs` Navigator tree and object Tables (`egui_extras`), both sharing the viewport selection; `scene.rs` turns a `Model`
  into GPU lines / triangles plus 2D marks (nodes, support symbols, load labels). RFEM's Z
  points down: model `(x, y, z)` is drawn at world `(x, -y, -z)`. Open a model with
  `cargo run --release -- file.rf6`, File > Open... (native dialog, `rfd`) or drag and drop.
- `crates/femcraft-render` orbit camera (Z up), grid and snapping, wgpu renderer (line list +
  translucent triangle list, no depth buffer yet), `shaders/`.
- `crates/femcraft-mcp` built-in MCP server. Methods mimic RFEM 6 API I (go-by:
  github.com/jprous/Rfem-API1-MCP-beta), one MCP tool each: `new_model`, `open_model`, `save`,
  `save_as`, `get_model_list`, `get_all_object_numbers` / `get_first_free_number` (`type` =
  `E_OBJECT_TYPE_*`), `set_<kind>({value: {no, ...}})` / `get_<kind>({no})` for material,
  section, thickness, node, line, member, surface, opening, nodal / line / surface support, load case,
  nodal / line / surface load (loads also take `load_case_no`). Fields are `kinds.rs` keys;
  enums are RFEM's stored codes. `api.rs` (methods, `Workspace` trait), `lib.rs` (JSON-RPC),
  `http.rs` (the app listens on `127.0.0.1:7980/mcp`, `FEMCRAFT_MCP_PORT`, 0 = off; requests
  are answered on the UI thread, so edits show live), `stdio.rs` + `headless.rs`
  (`femcraft --mcp`: each message is forwarded to the window on the HTTP port when one
  listens, so Claude Desktop drives the open window; else handled windowless). Add a kind by adding it to `api::KINDS`. Not yet: delete.
  New objects of a kind the model lacks need a prototype file holding one (line loads: e.g.
  `Al Qou_Rev6.rf6`); supports are assigned to the template's predefined ones (Hinged, Rigid).
  `tests/build_models.rs` builds a portal frame + RC pit over JSON-RPC.
- Libraries (`femcraft-rfdb/src/library.rs`): RFEM's `material_database.db` etc. are encrypted
  and API I cannot list them, so `tools/rfem_library_extract.py materials|sections OUT [--names
  FILE]` creates items by name in a scratch RFEM model (via `tools/rfem_soap.py`, RFEM API on
  8081) and keeps exact-name hits only (RFEM matches loosely). `OUT/<kind>.db` = that model's
  `model.db` (RFEM layout; library link `MaterialImpl.databaseMaterialId` /
  `DatabaseSection.databaseSectionId`) + index tables `femcraft_library`,
  `femcraft_library_value`. Looked up in `FEMCRAFT_LIBRARY`, `%APPDATA%\FemCraft\data\library`,
  `library/` next to the exe. MCP: `get_library_items`, `get_library_item`. UI: "…" in the Name column of
  Materials / Sections opens `library_dialog.rs`; Select calls
  `RfDocument::assign_library_item` (writes pending edits, `build::capture_library_item` +
  `Builder::replace_with` copy the item's impl tree into the working db and repoint the
  main row, so member refs stay; a section keeps its material; reload; `is_modified()`).
  `tests/library_assign.rs`. MCP `set_section` / `set_material`: `name` is a library name
  (full RFEM name or unique designation), assigned after the other fields
  (`femcraft-mcp/tests/library_names.rs`).
  **RFEM calls a file corrupted** when a library section lacks its `value_id` lists
  (`DatabaseSection_sectionMaterialCharacteristics`, `SectionTopology_outlines`): their
  targets come from the declared SQLite foreign keys (`Schema::fk`). Replaced library rows
  are deleted unless shared (copies within a model share `DatabaseSection` rows). Name
  column shows the library name via `database_section_id` / `database_material_id`.
  `rf6-catalog --assign OUT section|material NO NAME` makes test files (confirmed in RFEM). RFEM 6.16 has
  no SANS concrete / reinforcement; SA sections are manufacturer `Macsteel`; SA steel is
  `S355JR | SANS 50025-2`. API I deletes with `delete_object(type, no)`; a rejected
  `set_material` still leaves a user-defined object behind. The translator files map
  deprecated -> new RFEM library ids (tab separated).
- Section names come from a Prokon `Sections.dat` export (`tools/prokon_sections.py`; `--prokon`
  on the extractor): South Africa -> Macsteel catalogue with per-family suffixes
  (`prokon_sections.SUFFIX`), Europe -> ArcelorMittal EN 10365, else the bare designation
  (RFEM picks a manufacturer) accepted only if the area is within 3 % of Prokon's. Prokon's SA
  concrete rectangles / squares / circles become `R_M1` / `SQ_M1` / `CIRCLE_M1` items (category
  `concrete`). Left out: structural tees, timber, solid steel, custom sections. A rejected
  section leaves an object RFEM will not delete: its number is reused.
- RFEM's API `save` returns before the file is written (closing then loses it) and RFEM does not
  write into `%APPDATA%`: the extractor saves to `Documents\FemCraft\library` (`--rf6-dir`),
  waits for the file, then builds the `.db`. `--replay LOG` re-tries only names a log found;
  `--from-rf6` builds `materials.db` from a model saved by hand.
- Concrete sections are parametric, not library: `SectionImplParametricMassive` ->
  `ParametricSection.parametrizationShape` 49 = `R_M1` rectangle, 154 = `SQ_M1` square,
  53 = `CIRCLE_M1` circle (`mapping::parametric_shape`); dimensions `b` / `h` / `d` in
  `SectionData_parameterValues`. RFEM's UI (and the library) name them in mm: `R_M1 300/600`, `SQ_M1 400`,
  `CIRCLE_M1 400` (API I uses metres; `Library::get` accepts both). Section fields `parametrization_type`, `b`, `h`, `d` are read-only.
- Settings / Solve menus (`app.rs`, `AppSettings`, saved with the UI state): section / material
  database files (`femcraft_rfdb::set_db_override`, wins over the library folder search), RFEM
  server exe + web-service port + Dlubal e-mail / licence, working directory (dialogs start there,
  the solve scratch folder is `<working dir>\solved`; when it finishes `App::adopt_solved` copies
  the calculated file over the model's file (old one kept as `<name>.rf6bak`), deletes the scratch
  folder and `Document::reload`s the open tab in place, so results just appear; a model edited
  meanwhile opens the calculated copy as a new tab instead). Solve > "Mesh with RFEM" /
  MCP `generate_mesh` run `generate_mesh` instead of `calculate_all` (`femcraft_rfem::Job`):
  the file gets `!solver/M/` (FE mesh) but no result folders (tested, ~69 s). Picking a load case in the Results
  panel also sets the Display panel's load case. `crates/femcraft-rfem`: `solve()` copies the
  model, starts `RFEM6Server.exe` when nothing answers on the port (needs `--email` /
  `--password` / `--license`; entered in Settings > RFEM server… (dialog), password kept in the
  Windows Credential Manager via `keyring` (env `FEMCRAFT_RFEM_PASSWORD` overrides); without login the
  server logs "Missing login credentials" and exits), then API I over SOAP: `open_model` (app,
  port 8081) returns the model's own URL **on another port (8083...)**; `calculate_all`
  (result `succeeded` / `errors_and_warnings`), `save` (async: wait for the file), `close_model`
  by the new index. MCP `calculate_all`. An RFEM window already answering on the port is used
  instead. Tested end to end with the server (5 span beam, 66 s incl. start-up): it needs
  `--start-soap-server <port>` (else "No valid port specified for WS server", exit 1) and keeps
  HTTP connections open, so read by Content-Length (not to EOF).
- Which layout a results file has is read from its header, not the folder name (frames also use
  `RN_` folders): `i32[5]` > 0 and `i32[155]` > 0 = element-end block of 1D elements (decoded
  as below; nodes no 1D element touches keep the plausible values of the nodal block at
  `i32[48]`); otherwise (`i32[5]` = 0, `i32[155]` = -1, surface / solid meshes) the nodal block at
  `i32[48]`. Reading the nodal block of a frame gives garbage (1e30). Mixed 1D + 2D models (`i32[6]` > 0): the nodal block at `i32[48]` is `count` x 6 **f32**
  (24 bytes, global axes, members and surfaces alike), verified against RFEM's API; the
  per-element-corner block at `i32[163]` is NOT displacement. Envelopes (design situation / result combination) of members draw the Max and / or Min
  deformed shape (Both = both in one view); surfaces keep the undeformed envelope contour.
- **Surface internal forces are not stored**; RFEM builds them from the block at `i32[163]`.
  That block is per 2D element (`Data.E2D` order) 4 corners x 8 f64 of **generalised strains**,
  in the order of the basic forces: `[κx, κy, κxy, γxz, γyz, εx, εy, εxy]` (curvatures 1/m,
  transverse shear and membrane strains). Confirmed against RFEM's xlsx result export for
  `Chamber 4x4x6.rf6` (LC1): slot c correlates with basic force c, and fitting on one surface's
  interior nodes recovers the plate stiffness on the diagonal (coeff of κx in m_x = D =
  E·d³/12/(1-ν²); of κxy in m_xy = D(1-ν)/2; of εx in n_x = C = E·d/(1-ν²)). Build-up:
  force = isotropic plate constitutive law · generalised strain at each corner, then **smooth per
  surface** (average at each node over that surface's elements only; junction nodes keep a
  separate value per surface — node 5 differs across slab + two walls). **The law is CONFIRMED**
  (`surface_moments::basic_forces` / `PlateStiffness::isotropic`): deriving the basic forces from
  the export's OWN face strains (`κ = (ε_top − ε_bot)/d`, membrane `= (ε_top + ε_bot)/2`, both
  smoothed the same way so the map survives) gives, for the Chamber slab (C30/37, E 33 GPa, ν 0.2,
  d 0.4 m), `n_x`/`n_y`/`n_xy` to < 0.01 kN/m and `m_xy/κ_xy = D(1-ν)/2 = 73333` EXACTLY. So:
  `m_x = -D(κx + ν κy)`, `m_y = -D(κy + ν κx)`, `m_xy = -D(1-ν)/2·κxy`, `v = S·γ`,
  `n = C(ε + ν ε)` with D=E·d³/12/(1-ν²), C=E·d/(1-ν²), S≈5/6·G·d. (An earlier note here claimed
  a ~5.8× twist error / MITC covariant transform — that was WRONG; it came from reading the block's
  slot2 against the export's smoothed κxy. The block's membrane slots 5/6 match true membrane strain
  to ratio ≈1.0; the curvature slots 0/1/2 match with scatter only because the block is per-element
  while the export κ is smoothed, and because κ = small difference of face strains amplifies the
  export's per-mille rounding.) **Membrane n and shear v CAN be driven from the block** (slots 5/6
  match true membrane strain to ratio ≈1.0): build `PlateStiffness` per surface and call
  `basic_forces` — good to < 1 kN/m. **Moments from the block are still BLOCKED**: the block's
  bending slots 0/1/2 are NOT the Cartesian corner curvatures. Decisive test (parallel diagnostic):
  at interior FE node 10 the four adjacent elements' raw `m_x = -D(κx+ν κy)` from the block are
  [3.6, 3.8, 11.2, 11.6] but the export is -8.265 — outside that range and wrong-signed, so no nodal
  averaging or axis rotation recovers it; the per-corner curvature from the block is itself wrong.
  The block's 4 bending entries behave like 2×2 Gauss-point values needing the MITC4 covariant→
  Cartesian recovery (√3 corner extrapolation alone leaves ~23 kNm/m error). So `surface_moments::
  basic_forces` is the correct law (it reproduces forces from the export's face strains), but the
  input curvatures must come from the proper MITC recovery of the stored strains, not the raw block
  slots — that recovery is the open piece. Numbering is correct (NOT a blocker): the block is indexed in
  `Data.E2D` row order = the export's element numbers (0 mismatch), corner k ↔ `Data.E2D` node k,
  and `Data.GET` is a per-element tile counter (not the surface number — `mesh.rs` uses `Data.MTO`).
  RFEM API grid route was a dead end (`get_results_for_surfaces_basic_internal_forces` needs a
  surface **result grid** the model lacks; enabling one via `set_surface` fails `finish_modification`
  with "surface grid origin is ... defined incorrectly"). **Gotcha: `set_surface`'s `boundary_lines`
  must be the space-separated STRING ("5 6 7 13 8"); a JSON array stores only the first line and
  corrupts the surface (NaN centroid) — restore by re-setting the string.**
  Basic→principal→design formulas are in `surface_moments.rs`, all checked
  against the LC1 export to < 2e-5 (the manual's `m_c,D+` branch for `m_x > m_y, m_y < -|m_xy|`
  is `m_y - m_xy²/|m_y|`; `m_y,D±` / `n_y,D` are the x/y swap of the documented `m_x,D±` / `n_x,D`).
  The 92608/27200 offsets are for this 532-node / 511-element model; always read the header
  (`i32[163]`). Research scripts and the comparison workbooks are in the session scratchpad and
  `testdata/private/*_check_LC1.xlsx` (git-ignored).
- Results of 1D members live in `!solver/RO_<id>_<case> - SA1/Data.RO<id>` (surfaces / solids:
  `RN_*`). Same 601 header; the displacement block is at the byte offset in `i32[155]`: per
  element (`Data.E1D` pairs of FE nodes) 2 ends x 6 f64 `[ux,uy,uz,φx,φy,φz]` (member local axes;
  = global for members along X). Block at `i32[156]` (same size) looks like generalised strains
  (N, Vz, My per end), not decoded yet. Member results are drawn as thick colour-banded member
  lines (`scene::add_result_members`). Test: `femcraft-rfdb/tests/member_results.rs`.
- Member / line load `load_direction` is an INTEGER column: 0 / 1 / 2 = local x / y / z, 11 / 12 /
  13 = global X / Y / Z (13 = Z down, positive magnitude = downwards). MCP accepts the API names
  (`LOAD_DIRECTION_GLOBAL_Z_OR_USER_DEFINED_W_TRUE` -> 13); text written into the column made RFEM
  use the local axes.
- Support tables list only the support definitions (the `nodes` / `lines` / `members` / `surfaces`
  assignment fields stay in the model but are not columns); the Nodes / Lines / Members / Surfaces
  tables end in a virtual editable "Support" column (`panels::assign_support` moves the object
  between the supports' lists).
- **RFEM calls a file corrupted when a numeric column holds text** (an API enum name where it
  stores a code): `save` refuses it (`numeric_affinity` in `document.rs`; numbers in text are
  stored as numbers). Load case `actionCategoryId` is an integer: 11 = permanent G (confirmed);
  19 earth, 36 / 42 imposed, 61 wind, 71 temperature seen, names unconfirmed. Load
  `loadDistribution` 0 = uniform. MCP converts the confirmed names (`api::ACTION_CATEGORIES`,
  `LOAD_DISTRIBUTIONS`), refuses other names.
- Imposed-load cases (36, 42) need a `LoadCaseStandardDependentOptions` row linked by
  `LoadCaseImplStatic.loadCaseStandardDependentOptions_id`, or RFEM's server asserts
  `m_loadCaseStandardDependentOptions` and crashes on opening (client sees os error 10054).
  `RfDocument::complete_load_cases` (on save) adds it and sets a new load case's
  `activeSelfWeightOnFoundations` / `activeEarthCoveringOnFoundations` to its own
  `selfWeightActive` (copies of the self-weight case kept them on). Test:
  `tests/new_objects.rs::imposed_load_cases_complete`.
- Nodal support spring / restraint "rigid" is `inf` (JSON string `"inf"`; `null` writes NULL and
  RFEM refuses to calculate).
- `crates/femcraft-import` (File > Import, STAAD `.std`): `staad.rs` parser (joint / element loads kept), `surfaces.rs` plate mesh -> surfaces, `lib.rs` `import_staad` + `merge_onto_surfaces`: supports on every node of a surface -> one surface support (equal spring per node or per tributary area, 5 %; bands of different stiffness stay nodal); plate pressures equal on a whole surface, and nodal forces of equal pressure per area, -> uniform surface loads; the rest lumps to nodal loads (linear / triangular surface loads cannot be written yet: `SurfaceLoad` varying fields are read-only in rfdb). Private tests: `staad_private.rs`, `staad_merge_private.rs` (mock, prints notes). Prokon Frame `.A03` (`a03.rs`, text tables, Y up, shells offset by `SHELL_BASE`) is parsed into the same `StaadModel`, so it shares `import_model` and the merging; beams and uniform full-length beam loads (node pair -> member) are imported, point / trapezoidal / partial beam loads are counted, combinations are listed in the notes (no combination kind in FemCraft yet); beam sections are not (the report's section table is empty: members get section 1); tests `a03_*` (`PKW Wall_with shells.a03`, `C:\Temp\Model1.A03` from the A03RF6STEEL skill).
- Robot import (File > Import > Robot, `.str` or `.rtd`, picked by the compound-file signature; text may be UTF-16): `robot.rs` reads the ROBOT97 text, both Tekla's export and Robot's own (`PLAte`, 2D nodes, `21to24` ranges, `BOUndaries` skipped, `SELf-weight`, quoted material names; supports labels = restrained DOFs in Robot's files, free DOFs in Tekla's (`Label - free` comment)) into `StaadModel` (`z_up`). `rtd.rs` (`cfb` crate), layout found by diffing `Punching test model.rtd` against its `.str` (documented in its header): meshed models read nodes (`NDNUM`/`NDCOR`), tri / quad elements (`ELNUM`/`ELTYP`/`ELDAT`/`ELREC`, node indices), thickness (`ELA01` -> `SSDAT`), materials (`MADAT`/`MANAM`), support nodes (`SUREC`; DOF flags NOT found, imported as UZ), load cases and loads (`LNNAM`/`LNATR`, `LOATR` -> `LOREC` bitmask -> `LDREC`/`LDTYP` values, `LVREC` per-object bitmask of applied records; nodal-load targets inferred from one model); combinations not decoded. Unmeshed models (the sludge tank) only give panel corners (`OFDAT/OBCOR`) and a default 0.4 m thickness: the panel -> thickness link is still unknown. `import*`/`check` take bytes. Tests: `tests/robot_private.rs` (`TSD1 3D.str`, `Punching test model.str/.rtd` agree, `New Sludge Tank Rev 0.rtd`; files in `testdata/private`).
- `crates/femcraft-aster`, `-results` placeholders.
- egui / eframe 0.37 (panels are `egui::Panel`, `App::ui` instead of `update`).
- CI: `.github/workflows/ci.yml` (Windows build + test).
- `crates/femcraft-model` - no I/O. `kinds.rs`: every object kind (materials, sections,
  thicknesses, nodes, lines, members, surfaces, solids, openings, 4 support kinds, load cases,
  11 load kinds) with field descriptors (key, label, type/SI unit) that drive tables and dialogs.
  `model.rs`: `Model` of `ObjectId { kind, load_case, no }` -> `Object` (field values),
  typed `set` with dirty / removed tracking, geometry helpers. `geometry.rs`: line tessellation
  (arc, circle, ellipse, elliptical arc, parabola, spline). References are object numbers.
- `crates/femcraft-rfdb` - `.rf6` I/O. `archive.rs` ZIP / bare db / folder, raw-copies
  sidecars on save. `mapping.rs`: **all RFEM table and column names** (layout rules in its
  header; entries marked "guess" are unconfirmed). `document.rs`: `RfDocument::open` loads
  every kind generically (main row -> impl row -> list / part tables -> derived maps), resolves
  db ids to object numbers, reports missing fields; `save` writes changed main / impl / part
  columns and list tables. Read-only: type, library section / material properties, fields with
  no stored column (e.g. thickness of a surface without thickness). New objects (no binding) are
  inserted on save as copies of a similar object (own db, else `prototype_files`), then the
  model is reloaded and typed values re-applied (untyped refs cleared). Not yet: deleting.
  `tests/editable_fields.rs` edits every UI-editable field on the private models;
  `tests/new_objects.rs` builds a model from `blank_model` (`FEMCRAFT_KEEP=<path>` keeps it).
- File > New: `build::blank_model` (template emptied, materials / sections kept, LC1 + 200 mm
  thickness) cached as `%APPDATA%\FemCraft\data\new model.rf6`; prototypes from
  `%APPDATA%\FemCraft\data\prototypes\*.rf6`. Tables in `panels::NEW_ROW_KINDS` end in an
  empty row; typing adds the object (loads go to the load case shown in the 3D view).
- UI editing (temporary test set, `panels::is_editable`): double-click a blue-headed table cell (comments in every table)
  (Enter commits, Escape cancels); File > Save (Ctrl+S) / Save As (Ctrl+Shift+S), `.rf6` only.
  No unsaved-changes prompt on close yet.
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
  Surface thickness via `SurfaceStiffnessStandard.thickness_id`; surface load magnitude via
  `loadParameters_id` -> `<Impl>_NodeAndMagnitudeParameters_loadParameters`; member / surface
  support springs are `springConstant_N` on the impl row.
- Surface stiffness type from `SurfaceSpecificImplPart.stiffness_table`
  (`SurfaceStiffnessWithoutThickness` -> `TYPE_WITHOUT_THICKNESS`, no thickness).
- Verified models (local only): `Prokon Model1.rf6` (frame), `Al Qou_Rev6.rf6` (reservoir:
  surfaces, thicknesses, member supports, nodal / line / surface / free loads),
  `Solids_check_RBeam.rf6` (solids). Not yet seen: solid loads, free concentrated / line
  loads, imposed deformations, nodal load components.
- **RFEM opens FemCraft-edited files** (impl column and part table edits confirmed in RFEM).
- Layout reference: `docs/rfem-tables.md`. RFEM 6 has no solid loads in practice (owner):
  solids are closed sets of `TYPE_WITHOUT_THICKNESS` surfaces, loaded via surfaces / lines /
  nodes.
- New models: `build.rs` (template purge + prototype cloning, foreign-key cleanup) and
  `rf6-build <out-dir> <template.rf6> [donor.rf6 ...]` (portal frame + pit test models).
- Solver data and results are under `!solver/` in the ZIP (`Data.RFemX`, `RN_*` folders).
- FE mesh (after meshing / calculating in RFEM) only in `!solver/M/`: `Data.XYZ` (3 x f64 per
  FE node), `Data.E2D` (4 x i32 per 2D element), `Data.GET` (surface no per element),
  `Data.LIN` (text, FE nodes along each line); read by `femcraft-rfdb/src/mesh.rs`
  (`RfDocument::fe_mesh`), shown in the 3D view (Display > Mesh) until the model is edited,
  MCP `get_mesh`. Saving an edited model drops `!solver/`. Triangle encoding not seen yet.
  Test: `tests/fe_mesh.rs` on `FemCraft pit 1 geometry-meshed.rf6`.
- Saving strategy: start from a database RFEM saved, change only edited rows, keep all
  unknown tables and sidecars byte-identical.

## Rules

- Never commit `.rf6` / `.rfc` or anything in `testdata/private/` (client data).
- Licence is MIT OR Apache-2.0. SolidCraft and BIMCraft code may be reused with attribution in
  `NOTICE`; CadCraft and Lapin for ideas; **Stabileo is AGPL - never copy its code.**
- Prefer small diffs; run `cargo build` and `cargo test` before committing.
