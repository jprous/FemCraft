# FemCraft roadmap

FemCraft aims to be a familiar FEM pre-processor, solver front-end and post-processor in one
application:

1. Open and save RFEM 6 models: `.rf6` (zipped) and `.rfc` (unzipped database), keeping the
   same table structure as RFEM.
2. Solve models with Salome-Meca / code_aster.
3. View results in the same application.

The strategy is to tackle the riskiest part first - reading and writing `.rf6` - and build the
interface on top of a working file layer rather than ahead of it.

## Reference projects

| Project | Stack | Licence | What we take |
|---|---|---|---|
| [SolidCraft](https://github.com/bherbruck/solidcraft) | Rust, egui + wgpu, crate workspace (`engine`, `render`, `gpu`, `ui-egui`, `io`, `derive`, `mcp`) | MIT / Apache-2.0 | Code may be reused with attribution in `NOTICE`: workspace layout, renderer, view handling, property-editor derive macros |
| [BIMCraft](https://github.com/IPedrax/BIMCraft) | Rust, egui (`core`, `ui-egui`, `rvt`, `dwg`) | MIT / Apache-2.0 | Code may be reused with attribution: object model, tree/property panels, reading another vendor's format |
| [CadCraft](https://github.com/getcadcraft/cadcraft) | JavaScript | Apache-2.0 | UX patterns: ribbon controls, panes, command line, undo, property panel |
| [Lapin](https://github.com/Ciuntuc-BIM-Studio/lapin) | C# / TypeScript | MIT | UX patterns: classic modeller layout and interaction |
| [Stabileo](https://github.com/lambdaclass/stabileo) | TypeScript + Rust engine | **AGPL-3.0** | **Study only - never copy code.** Visual conventions for supports, hinges, loads and result diagrams |
| Dlubal RFEM 6 API | Dlubal's published API (client libraries, documentation) | Dlubal terms - check | Interface design only: object/attribute naming and operation vocabulary for `femcraft-api`. No copying of Dlubal code or docs |

### Ground rules

- **No client or third-party models in the repository.** Real project files are used only
  locally, in the git-ignored `testdata/private/` folder. Public test models are built
  specifically for FemCraft.
- **RFEM file support is reverse engineering for interoperability.** Check Dlubal's licence
  terms before publishing that part, and never commit files produced by RFEM unless they were
  made for this purpose and their terms allow it.
- **No AGPL code** in FemCraft, which is MIT OR Apache-2.0.

## What we know about `.rf6`

- An `.rf6` file is a ZIP archive containing a SQLite database `model.db` plus XML sidecars
  (`general_data.xml`, `unit.xml`, `views.xml`, `mesh.xml`, ...), previews (`preview.glb`,
  `preview.jpg`) and `format.txt` (program and format version).
- A real project model has about 5,900 tables. Objects follow a consistent pattern:
  a main table (`Node`, `Line`, `Member`, ...) with `impl_id` / `impl_table` pointing to a
  detail table (`NodeImplStandard`, `LineImplPolyline`, `MemberImplBeam`, ...), which in turn
  owns child lists (`*_items`, `*_keys` / `*_values`, rows with `owner_id`).
- Still to confirm: what `.rfc` is exactly, and where calculated results are stored.

## Command API and MCP server (cross-cutting, from M0)

Goal: let an AI assistant (Claude, DeepSeek, any MCP client) drive FemCraft the same way
scripts drive RFEM 6, so existing Dlubal API knowledge, scripts and examples transfer with as
few changes as possible.

### Design

- **One API, many front-ends.** All model changes and queries go through `femcraft-api`. The
  GUI, the MCP server, and later a scripting/remote interface are thin layers over it. The GUI
  must not have a private path to the model that the API lacks; this keeps the API complete
  and testable.
- **Dlubal compatibility.** Mirror the RFEM 6 API's concepts and names: the application /
  model split, object types and their attribute names (`Node`, `Line`, `Member`, `Surface`,
  `Material`, `Section`, `LoadCase`, ...), numbered objects, and the operation vocabulary
  (get / set / delete object, get object list, begin/finish modification for batching,
  calculate, get results). Use the same attribute names and units so payloads are largely
  interchangeable. Where RFEM semantics are unclear or not supported, return an explicit
  "not supported" error rather than silently differing.
- **Same command layer as editing.** API write operations are the M3 edit commands (apply /
  undo, grouped). One API batch = one undo step, and it feeds the same change log, so "results
  out of date" and saving behave identically for GUI and AI edits.
- **Transport.** Core is transport-neutral. The MCP server (`femcraft-mcp`) speaks MCP over
  stdio first (works with Claude Desktop / Claude Code and DeepSeek-capable MCP clients), with
  local HTTP/SSE later. A Dlubal-style remote endpoint (gRPC or SOAP, whichever the current
  RFEM 6 API uses) is optional and comes after MCP.
- **MCP tools.** Generic, schema-driven tools rather than hundreds of hand-written ones:
  `list_object_types`, `get_object_schema`, `get_objects`, `set_objects`, `delete_objects`,
  `begin_modification` / `finish_modification`, `open_model` / `save_model`, `calculate`,
  `get_results`, plus view helpers (screenshot, select, isolate). Schemas are generated from
  the same field descriptions used for the edit dialogs (M4) and the typed objects (M3).
- **Safety.** AI edits go through the undo stack; destructive operations (delete, overwrite on
  save) can require confirmation in the GUI; a read-only mode exposes queries only; the server
  binds to localhost and needs no network access.

### Open questions (resolve in M1, before committing to names)

- Confirm the current RFEM 6 API: which protocol it uses (gRPC vs. the older SOAP web
  services), its exact object/attribute naming, and its licence terms. Check Dlubal's
  documentation and licence for reusing names and structures, as with the `.rf6` format
  ("Ground rules"). Do not copy Dlubal's proprietary client libraries or documentation text;
  reimplement from observed behaviour and published interface descriptions.
- Whether Dlubal already ships an MCP server or AI integration whose tool naming we should
  align with.
- How far attribute names in the `.rf6` database (table columns) match API attribute names; if
  they match closely, schemas can be derived from the catalogue tool (M1).

### Milestone touchpoints

| Milestone | API / MCP work |
|---|---|
| M0 | `femcraft-api` + `femcraft-mcp` crates, stdio server, info/ping tools, CI test |
| M1 | Read-only tools: open model, list object types, get objects; compare naming to Dlubal API; schema derived from catalogue |
| M2 | `save_model`; "RFEM accepts the file" tests also run via the API |
| M3 | Write tools mapped onto edit commands; batching; undo; validation errors in Dlubal style |
| M4 | UI indicator for connected MCP clients, activity log, confirm/read-only toggles |
| M5 | View tools: screenshot, select, isolate, zoom |
| M6 | `calculate` tool, solver progress, errors |
| M7 | `get_results` tools, result tables as JSON |

## Milestones

### M0 - Foundations

- Upgrade to the current egui / eframe / wgpu to stay level with SolidCraft and BIMCraft.
- Split into a crate workspace:
  - `femcraft-model` - domain objects and edit commands, no UI
  - `femcraft-rfdb` - `.rf6` / `.rfc` reading and writing
  - `femcraft-render` - 3D drawing
  - `femcraft-ui` - panels, ribbon, tables, dialogs
  - `femcraft-aster` - solver connection
  - `femcraft-results` - results storage and display
  - `femcraft-api` - the command API (Dlubal-compatible object model and operations), no UI
  - `femcraft-mcp` - MCP server exposing `femcraft-api` to Claude, DeepSeek and other clients
  - `apps/femcraft` - the executable
- GitHub Actions CI building and testing on Windows.
- API/MCP skeleton from day one (see "Command API and MCP server"): an empty `femcraft-api`
  trait with `get_application_info`, `get_model_info` and `ping`-style calls, and a
  `femcraft-mcp` stdio server that lists them as tools. Every later milestone adds its
  operations to this API as it adds the feature.

### M1 - File format research and read-only viewer

1. Open the ZIP (`zip`), read `model.db` (`rusqlite`), keep every sidecar file unchanged.
2. Schema catalogue tool: dump all tables and columns and recognise the object / impl / child
   pattern generically.
3. Learn the format by comparison: generate controlled models in RFEM (empty, 1 node,
   2 nodes, 1 member, ...) - ideally through the Dlubal API - and diff the databases to see
   exactly which rows change.
4. Confirm what `.rfc` is by saving a model as `.rfc` from RFEM.
5. Goal: open a real model and show nodes, lines, members and surfaces in the viewport, with a
   read-only navigator tree and tables.

### M2 - Saving that RFEM accepts

- Never build a database from scratch. Start from a database RFEM saved, keep every table
  FemCraft does not understand exactly as it was, and change only edited rows.
- Test 1: open and save without changes gives an identical database, and RFEM opens it.
- Then one kind of edit at a time, always confirming RFEM opens the result.
- Check `format.txt` and warn on untested versions.
- Mark results as out of date after any edit.

### M3 - Object model and change tracking

- Typed objects: Node, Line, Member, Surface, Opening, Material, Section, Thickness, Support,
  Hinge, LoadCase, LoadCombination, loads. Each keeps the database columns FemCraft does not
  edit, so saving loses nothing.
- Storage in `slotmap` collections with lookup by object number.
- Every edit is a command with apply / undo; commands can be grouped so one user action is
  one undo step.
- A change log of dirty objects drives saving, UI refresh and "results out of date".
- Dependency rules (for example deleting a node still used by lines).
- Selection and visibility live in view state, not in the model (as RFEM's `views.xml`).

### M4 - Application layout (can run alongside M1-M3)

- Dockable panels (`egui_dock`), layout remembered between sessions:
  - Navigator on the left with tabs: Data, Display, Views, Results
  - Tables at the bottom, one tab per object type (`egui_table`, virtualised)
  - Properties panel
- Ribbon alternative: compact tabbed toolbar (Home | Model | Loads | Mesh | Calculate |
  Results | View) with grouped buttons, collapsible to one row; classic line toolbars remain
  available as a toggle.
- Single action registry: each action (name, icon, shortcut, enabled-when) is defined once and
  feeds the menu bar, ribbon, toolbars, shortcuts, right-click menus and a command palette.
- Menu bar: File (New, Open, Save as `.rf6` / `.rfc`, recent files), Edit, Insert, View,
  Calculate, Results, Help.
- Navigator tree (`egui_ltreeview`) with visibility checkboxes in the Display tab.
- View cube in the top-right corner: 26 clickable faces, edges and corners, home button,
  perspective / orthographic toggle.
- Edit dialogs generated from field descriptions on each object type.
- Status bar with snap toggles (Grid, Node, Ortho) and units.

### M5 - Model space drawing and interaction

- Depth buffer and batched drawing per object type:
  - nodes as points
  - members as lines or as their real cross-section shape
  - surfaces as filled triangles, with holes for openings
  - supports and hinges as symbols
  - loads as arrows
  - local axes and number labels
- Picking by rendering object IDs to an off-screen target; box and crossing selection.
- Right-click menu on any object: Edit, Properties, Hide, Isolate, Select same type, Delete.
- Visibility by object, type, load case or saved visibility sets.
- Drawing tools: node, line, polyline, member, surface from boundary lines; snapping to grid,
  nodes, midpoints and intersections (`rstar` spatial index).

### M6 - Solver: Salome-Meca / code_aster

- Meshing: FemCraft writes a Salome Python script (GEOM + SMESH) that builds geometry and
  mesh, with one named group per FemCraft object (`M12`, `S3`, `SUP5`), exported as MED.
  Salome and code_aster run as separate programs, so their GPL / LGPL licences do not affect
  FemCraft.
- Command file (`.comm`) generation:

  | FemCraft | code_aster |
  |---|---|
  | members | beam elements (`POU_D_T`), sections via `AFFE_CARA_ELEM` |
  | surfaces | shell elements (`DKT`), thickness via `COQUE` |
  | supports | `DDL_IMPO` or spring elements |
  | materials | `DEFI_MATERIAU` |
  | self-weight, surface loads | `PESANTEUR`, `FORCE_COQUE` / `PRES_REP` |
  | modal / spectral | `CALC_MODES` / `COMB_SISM_MODAL` |

  Each load case is solved once; FemCraft builds combinations itself.
- Running: Salome-Meca is mainly a Linux program; on Windows run it in WSL2 or a container,
  behind a `SolverRunner` interface (local / WSL / container / remote).
- Results import: a small Salome Python script (MEDCoupling) converts MED results into
  FemCraft's own results file, avoiding an HDF5 build on Windows.
- Validation against hand calculations and against RFEM results for the same models.

### M7 - Post-processing

- Results stored in a separate `results.db`, keyed by load case / combination and object.
- In the model view: deformed shape with scale, contour colours on surfaces with legend,
  member diagrams (N, Vy, Vz, Mt, My, Mz), reaction arrows, envelopes, animation, section cuts.
- Results tab in the navigator and result tables in the bottom panel.
- Export to CSV / Excel and screenshots.

## Order

M0 → **M1 read-only viewer** → M4 layout (in parallel) → M2 saving → M3 + M5 editing →
M6 solver → M7 results.

The command API and MCP server start in M0 and grow with every milestone (see "Command API
and MCP server"); they are not a separate late milestone.

M2 decides whether the project is viable: if RFEM rejects edited files we learn it early,
before everything else is built around it.

## Credit budget (Claude Code)

Budget: **one third of the weekly Claude Pro limit** (about 33% of the weekly bar).

### Unit of work

Work is planned in **sessions**: one fresh Claude Code session per task, ending in a commit.
A focused session (one feature, short conversation) is estimated at **3-6% of the weekly
limit**. That leaves room for **about 6-8 sessions per week**.

The estimate is rough: usage depends on the model, conversation length and how much code is
read. It is corrected by measuring (see the tracking table below).

### Estimate per milestone

| Milestone | Sessions | % of a weekly limit | Weeks at 33% / week | Suggested model |
|---|---|---|---|---|
| M0 Foundations (incl. API / MCP skeleton) | 3-4 | 12-20% | 0.5-1 | Sonnet |
| M1 File research + read-only viewer (+ read-only MCP tools) | 7-10 | 30-50% | 1-1.5 | Opus (format research, API naming), Sonnet (viewer) |
| M2 Saving RFEM accepts | 5-8 | 20-40% | 1-1.5 | Opus |
| M3 Object model, undo, change log (+ MCP write tools) | 5-8 | 20-40% | 0.5-1.5 | Opus (design), Sonnet (object types) |
| M4 Application layout | 7-10 | 25-50% | 1-1.5 | Sonnet |
| M5 Drawing, picking, editing tools | 8-12 | 30-60% | 1-2 | Sonnet, Opus for picking / renderer |
| M6 Salome-Meca / code_aster | 9-14 | 35-70% | 1-2 | Opus |
| M7 Post-processing | 6-9 | 25-45% | 1-1.5 | Sonnet |
| **Total** | **51-77** | **200-380%** | **6-12 weeks** | |

**First usable result:** the read-only viewer (end of M1), after roughly 2 weeks.
**Go / no-go point:** M2. If RFEM rejects edited files, stop and reconsider (for example
exporting through the Dlubal API or SAF instead) before spending the remaining ~70% of the
budget.

### Rules for spending credits sparingly

1. **One fresh session per task.** Long conversations are the biggest cost: every message
   re-sends the whole history. Start a new session for each task and close it after the commit.
2. **Use Sonnet by default**, and Opus only where the table says so (format reverse
   engineering, solver mapping, architecture). Keep reasoning effort at low / medium for
   routine work.
3. **`CLAUDE.md` holds the project context**, so sessions do not re-research the project,
   the RFEM format or the reference repos.
4. **Small, precise tasks** ("add the view cube in `femcraft-render`, clicking faces sets the
   view") rather than open-ended ones ("improve the UI").
5. **Run builds and the app yourself** and paste only the error lines. Avoid asking for
   screenshots, web research or reading whole reference repositories; point to the exact file.
6. **Batch small fixes** into one session instead of one session each.
7. **Stop at the milestone boundary**, check the weekly bar, and re-plan if a milestone ran
   over its estimate.

### Tracking

Record the weekly-limit percentage before and after each session, so the estimates can be
corrected after M0 and M1.

| Date | Milestone / task | Model | Weekly % before | Weekly % after | Used |
|---|---|---|---|---|---|
| 2026-11-02 | Setup, first viewport, licence, roadmap (one long session) | Opus | - | 25% (whole week so far) | not measured separately |

## Candidate crates

| Purpose | Crate |
|---|---|
| UI / rendering | `egui`, `eframe`, `egui-wgpu`, `wgpu` |
| Docking | `egui_dock` (alternative: `egui_tiles`) |
| Tables | `egui_table` / `egui_extras` |
| Tree view | `egui_ltreeview` |
| Database / archive / XML | `rusqlite` (bundled), `zip`, `quick-xml` |
| Object storage | `slotmap` |
| Spatial index | `rstar` |
| Maths | `glam`, `faer` (for any in-house solving) |
| MCP server | `rmcp` (official Rust MCP SDK; verify current status) |
| API schemas / serialisation | `serde`, `serde_json`, `schemars` |
| Async / transport | `tokio` (MCP stdio / HTTP), `tonic` + `prost` if a gRPC endpoint is added |
