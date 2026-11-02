# FemCraft

A 3D structural modelling workspace written in Rust, aimed at building finite-element models
(nodes, members, surfaces, loads) and handing them to analysis software such as RFEM 6.

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

- Depth buffer and shaded surfaces
- Model objects: nodes, lines, members, surfaces, supports
- Materials, sections and thicknesses
- Load cases and combinations
- Export to RFEM 6 through the Dlubal API
