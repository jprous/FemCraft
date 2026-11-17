# FemCraft short-term shopping list

Work top to bottom. Tick an item when it is merged; move finished groups to the bottom.
Longer-term plan: `ROADMAP.md`.

## Now: line types

- [ ] Read and draw all RFEM line types: polyline, arc, circle, ellipse, spline (confirm
      tables and columns on real files, add them to `mapping.rs`).
- [ ] Show the type in the Lines table; edit arc / circle parameters.
- [ ] Create arc / circle lines in the modeler (and over MCP).

## Then: openings

- [ ] Read openings (boundary lines, surfaces they cut) from `.rf6`.
- [ ] Draw openings in the 3D view (surfaces with holes).
- [ ] Openings table + Navigator entry; create / edit openings in the modeler.
- [ ] Openings over MCP (`set_opening` / `get_opening`).

## Later (parking)

- [ ] Cold-formed lipped angles in the section library (needs an RFEM name example).
- [ ] Structural tees and IPE R sections in the library.
- [ ] Deleting objects.
- [ ] Unsaved-changes prompt on close.

## Done

- [x] Material / section libraries extracted from RFEM; MCP `get_library_items` / `get_library_item`.
- [x] Concrete rectangle / square / circle sections recognised.
- [x] FE mesh display.
- [x] Library dialog ("…" in the Materials / Sections Name column).
- [x] Library picker: Select fills the table cell (library rows copied into the model,
      members / surfaces keep pointing at it); saved files confirmed to open in RFEM.
- [x] Library dialog: rows select with one click (no text cursor).
- [x] MCP `set_section` / `set_material` take a library name.
