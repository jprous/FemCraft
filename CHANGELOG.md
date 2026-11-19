# Changelog

## 0.1.0 - 2026-11-19

First release.

### Models
- Open, edit and save RFEM 6 `.rf6` files; RFEM opens the files FemCraft writes.
- File > New from a template model; new objects are cloned from similar objects.
- Table editing for the main object kinds, with a Navigator tree sharing the viewport selection.
- Multiple models open at once, each in its own tab; dockable panels; recent files.
- Parametric concrete sections (rectangle, square, circle) in any size.

### 3D view
- Orbit / pan / zoom viewport, construction grid with snapping, standard views.
- Nodes, lines, members, surfaces, supports and loads per load case.

### Solving with RFEM
- Solve > Calculate with RFEM server: runs RFEM 6's own solver through RFEM6Server.exe.
- Solve > Mesh with RFEM: generates only the FE mesh.
- Solve > Delete results / Delete mesh.
- Progress dialog while RFEM works; results appear in the open model when it is done.
- Dlubal account password is kept in the Windows Credential Manager, never in a file.

### Results
- Results navigator with load cases, combinations and design situations.
- Deformation and contour plots for surfaces and solids; member displacements drawn as
  colour-banded lines; envelopes (Min / Max / Both); contact stress.
- The load case chosen in Results also selects the loads shown in Display.

### Settings
- Settings menu: section database, material database, RFEM server (location, port, login),
  working directory.

### MCP server
- Built-in MCP server on 127.0.0.1:7980 with RFEM 6 API I style methods
  (`set_node`, `set_member`, `calculate_all`, `generate_mesh`, ...).
- Load directions accept RFEM API names (global Z = code 13); concrete sizes are created on request.

### Known limits
- Member internal forces and surface forces are not displayed yet.
- No undo / redo and no delete of model objects yet.
- Solving needs a licensed RFEM 6.16 installation (RFEM6Server.exe).
