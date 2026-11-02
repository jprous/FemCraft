# FemCraft

A 3D structural modelling workspace written in Rust: build finite-element models (nodes,
members, surfaces, loads), open and save RFEM 6 files, solve with Salome-Meca / code_aster and
view the results, all in one application.

Built with [egui](https://github.com/emilk/egui) for the interface and
[wgpu](https://wgpu.rs) for the GPU-rendered 3D viewport.

## Current features

- 3D workspace with orbit / pan / zoom navigation (global Z up)
- Construction grid with metric spacing, placed on the **XY**, **XZ** or **YZ** plane
  - adjustable spacing (with presets), extent, major-line interval and level along the plane normal
- Cursor snapping to grid intersections, with live X / Y / Z coordinates in the status bar
- Standard views (Iso, Top, Front, Right), perspective / orthographic projection
- Global axes and an orientation triad

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
src/
  main.rs          window setup
  app.rs           UI: menu, settings panel, status bar, viewport interaction
  camera.rs        orbit camera, projections, picking rays
  grid.rs          construction grid geometry and snapping
  viewport.rs      wgpu line renderer used inside the egui viewport
  shaders/lines.wgsl
```

## Roadmap

The full implementation strategy is in [docs/ROADMAP.md](docs/ROADMAP.md). In short:

- **M0** Foundations: crate workspace, current egui / wgpu, CI
- **M1** Read RFEM 6 `.rf6` / `.rfc` files and show them (read-only viewer)
- **M2** Save files RFEM accepts
- **M3** Object model with undo / redo and change tracking
- **M4** Application layout: dockable navigator and tables, ribbon and toolbars, view cube
- **M5** Drawing, picking, right-click menus, visibility
- **M6** Solving with Salome-Meca / code_aster
- **M7** Post-processing: deformations, contours, member diagrams, result tables

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
