# Changelog

## 0.1.0 - 2026-11-19

First release.

### Models
- Open, edit and save RFEM 6 `.rf6` files; RFEM opens the files FemCraft writes.
- File > New from a template model; new objects are cloned from similar objects.
- Table editing for the main object kinds, with a Navigator tree sharing the viewport selection.
- Support tables list only the support definitions; the Nodes, Lines, Members and Surfaces tables
  end in an editable "Support" column that says which support line an object refers to.
- Multiple models open at once, each in its own tab; dockable panels; recent files.
- Parametric concrete sections (rectangle, square, circle) in any size.

### 3D view
- Orbit / pan / zoom viewport, construction grid with snapping, standard views.
- Nodes, lines, members, surfaces, supports and loads per load case.

### Solving with RFEM
- Solve > Calculate with RFEM server: runs RFEM 6's own solver through RFEM6Server.exe.
- Solve > Mesh with RFEM: generates only the FE mesh.
- Solve > Delete results / Delete mesh.
- Results of frames (1D elements only) are read from the right data block, whichever way RFEM
  names the result folders; cases RFEM lists but never calculated are left out.
- Progress dialog while RFEM works; results appear in the open model when it is done.
- Dlubal account password is kept in the Windows Credential Manager, never in a file.

### Results
- Results navigator with load cases, combinations and design situations.
- Deformation and contour plots for surfaces and solids; member displacements drawn as
  colour-banded lines; contact stress.
- The plotted shape follows the component: u_x, u_y and u_z move the shape along that axis only;
  |u| and the rotations show the full translation.
- Design situations (explicit or generated combinations) envelope their combinations; for members
  Max and Min draw the deformed max / min shape and Both draws both in one view.
- Display > "Colour results": off gives plain thin yellow lines (members: a line plot; surfaces:
  the displaced mesh net without contours).
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
