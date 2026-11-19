# FemCraft

A 3D structural modelling workspace for FEA models, written in Rust: build finite-element
models, calculate them and view the results, all in one application. FemCraft's native file
format today is RFEM 6 (`.rf6`), and the models are solved with RFEM's own solver.

Planned: importing models from other structural programs (Prokon Frame `.A03`, Autodesk Robot,
STAAD.Pro, Tekla Structural Designer `.tsmd` and Midas), and solving with Salome-Meca / code_aster.

Built with [egui](https://github.com/emilk/egui) for the interface and
[wgpu](https://wgpu.rs) for the GPU-rendered 3D viewport.

**Download:** the portable Windows x64 build is on the
[Releases](https://github.com/jprous/FemCraft/releases) page (unzip and run `femcraft.exe`).

## Features (v0.1.0)

- **Model files**: open, edit and save RFEM 6 `.rf6` files (RFEM opens the files FemCraft
  writes). File > New from a template; several models open at once, each in its own tab.
- **Modelling**: materials, sections (incl. parametric concrete rectangles / squares / circles in
  any size), thicknesses, nodes, lines, members, surfaces, openings, supports, load cases and
  nodal / line / member / surface loads, edited in tables with a Navigator tree.
- **3D view**: orbit / pan / zoom, construction grid with snapping, standard views, supports,
  loads per load case, optional FE mesh.
- **Solve with RFEM**: *Solve > Calculate with RFEM server* runs `RFEM6Server.exe` on the model
  (needs a licensed RFEM 6.16). *Mesh with RFEM* generates only the mesh; *Delete results* /
  *Delete mesh* remove them again. A progress dialog is shown while RFEM works and the results
  appear in the open model when it is done.
- **Results**: load cases, combinations and design situations; deformation and contour plots,
  member displacements as coloured lines, envelopes, contact stress.
- **Settings menu**: section database, material database, RFEM server (location, port, Dlubal
  login) and working directory. The Dlubal password is kept in the Windows Credential Manager.
- **MCP server**: FemCraft listens on `127.0.0.1:7980/mcp` with RFEM 6 API I style tools
  (`set_node`, `set_member`, `calculate_all`, `generate_mesh`, ...) so an AI assistant or script
  can build and calculate models.

See [CHANGELOG.md](CHANGELOG.md) for the details of each release.

## Development status

How far each milestone of the [roadmap](docs/ROADMAP.md) has come:

| Milestone | Progress | State |
| --- | --- | --- |
| M0 Foundations (workspace, CI) | `██████████` 100 % | done |
| M1 `.rf6` reader and viewer (native format) | `█████████░` 90 % | done; `.rfc` and a few rare load kinds still unseen |
| M2 Saving that RFEM accepts | `█████████░` 90 % | edits, new objects, library items confirmed in RFEM |
| M3 Object model and change tracking | `█████░░░░░` 50 % | typed objects and dirty tracking; undo / redo and delete missing |
| M4 Application layout | `███████░░░` 70 % | docks, ribbon, tables, settings; view cube and command palette missing |
| M5 Drawing and interaction | `███░░░░░░░` 30 % | depth-correct drawing, box selection; drawing tools and right-click menus missing |
| M6 Solving | `██████░░░░` 60 % | RFEM 6 solver connected; Salome-Meca / code_aster not started |
| M7 Post-processing | `████░░░░░░` 40 % | deformations, contours, member displacement lines; forces, diagrams, tables, export missing |

## Install (portable)

1. Download `FemCraft-0.1.0-win64.zip` from the Releases page and unzip it anywhere.
2. Run `femcraft.exe`. The section and material databases (`library/`), the new-model template
   (`template/`) and a few prototype models (`prototypes/`) are found next to the program.
3. To solve, open *Settings > RFEM server…* and enter the location of `RFEM6Server.exe` and your
   own Dlubal account. Nothing of the author's account is included.

Windows 10 / 11 x64. A GPU with DirectX 12, Vulkan or OpenGL support is needed.

## Controls

| Action | Mouse |
| --- | --- |
| Rotate | Right-drag |
| Pan | Middle-drag |
| Zoom | Wheel |

## Building

Requires [Rust](https://rustup.rs) (stable).

```bash
cargo run            # debug build
cargo run --release  # optimised build
cargo test
```

On Windows with the GNU toolchain (`x86_64-pc-windows-gnu`) you also need MinGW-w64 binutils
on `PATH` (for `dlltool`), e.g. `winget install BrechtSanders.WinLibs.POSIX.UCRT`.
The MSVC toolchain needs Visual Studio Build Tools instead.

## Project layout

```
apps/femcraft        the executable (window setup)
crates/femcraft-model  object kinds, fields and the editable model (no I/O)
crates/femcraft-rfdb   .rf6 reading / writing, libraries, mesh and results readers
crates/femcraft-render orbit camera, grid, wgpu renderer
crates/femcraft-ui     panels, tables, ribbon, dialogs, 3D scene
crates/femcraft-mcp    built-in MCP server (RFEM 6 API I style methods)
crates/femcraft-rfem   RFEM6Server connection (solve / mesh)
docs/                  roadmap and RFEM table notes
tools/                 library extraction scripts
```

## Licence

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in FemCraft by you, as defined in the Apache-2.0 licence, shall be dual licensed as above,
without any additional terms or conditions.

FemCraft is independent and is not affiliated with, sponsored by or endorsed by Dlubal Software.
RFEM is a trademark of its owner, named only to describe compatibility.
