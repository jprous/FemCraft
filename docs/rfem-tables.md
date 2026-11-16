# RFEM 6 `model.db` structure (as found in 6.16)

Reverse-engineered for interoperability from models saved by RFEM 6.16. The authoritative,
code-level mapping is `crates/femcraft-rfdb/src/mapping.rs`; this page records the general
layout and the tables not yet seen with data.

## General layout

- **Main table** per object type (`Node`, `Line`, `Member`, ...): `id` (db key), `version`,
  `userID` (the number shown in RFEM), `idForExportImport` (GUID), `impl_id` / `impl_table`.
  Loads also carry `parentModelObject_id` / `parentModelObject_table = LoadCase`.
- **Impl table** (`NodeImplStandard`, `MemberImplBeam`, `LineTypeLoadImplForce`, ...): the
  type-specific columns; its name gives the type. `parent_id` points back to the main row.
- **Part tables** with `owner_id` / `owner_table` = the impl row: `SpringConstants`,
  `SurfaceSpecificImplPart`, `SurfaceStiffnessStandard`, `MemberLoadTransfer`,
  `*SpecificImplPart`, `*VaryingParameters`, ... Part rows can own further part rows.
- **List tables** `<Owner>_<name>`: `id` = owner row id, `container_order`, value in one of
  `reference_id` + `reference_table`, `object_id` + `object_table`, `value_id`, or `value`.
  Key / value maps are `<Owner>_<map>_keys` + `_values`, paired by `id` + `container_order`.
- References between objects are **database ids**, never user numbers.
- The schema declares **foreign keys**; models saved by RFEM have none dangling
  (`rf6-catalog file.rf6 --fk`).
- Not stored (computed by RFEM): line / member length, surface area, solid volume.
- `-inf` means "not set"; `inf` means fixed / rigid.
- Settings live in object-like tables that must stay in any file: `StaticAnalysisSettings`,
  `Layer`, `CoordinateSystemModelObject`, `DesignSituation`, `AddonCase`, `Snap`,
  `LineGrid`, design configurations, notifiers, ...
- Sidecar `general_data.xml` holds object counts, a "has results" flag and the model GUID;
  results live under `!solver/` in the ZIP.

## Enumeration codes seen

- Nodal load `loadDirectionAxis`: 0 / 1 / 2 = global X / Y / Z.
- Surface load `loadDirection`: 11 / 12 / 13 = global X / Y / Z (walls 11 / 12, slabs 13).
- Member / line load `loadDirection`: 13 = global Z; 2 also common (likely local z).
- Global Z points down: "up" is negative Z.

## Recorded but not yet seen with data

Columns of the main / impl / list tables (bookkeeping columns omitted):

- `FreeConcentratedLoadImplForce` / `...ImplMoment`: coordinateSystem_id, loadProjection,
  loadDirection, actingRegionFrom, actingRegionTo, isAssignedToAll, locationFirst,
  locationSecond, magnitude; list `_assignedTo` (reference_id, reference_table).
- `FreeLineLoadImpl`: coordinateSystem_id, loadProjection, loadDirection, actingRegionFrom,
  actingRegionTo, isAssignedToAll, distribution, locationFirstX, locationFirstY,
  locationSecondX, locationSecondY, magnitudeFirst, magnitudeSecond, varyingLoadParameters_id;
  list `_assignedTo`; part `FreeLineLoadVaryingParameters` with list `_varyingLoadParameters`
  (note, distance, temperaturesOrMagnitudeFirstMagnitude, ...SecondMagnitude).
- `ImposedNodalDeformationImpl`: imposedDisplacement_x / _y / _z, imposedRotation_x / _y / _z;
  list `_assignedTo` (object_id, object_table, objectSupport_id, objectSupport_table).
- `ImposedLineDeformationImpl`: imposedDisplacementAtLineStart_x / _y / _z,
  imposedDisplacementAtLineEnd_x / _y / _z, imposedRotationLineStart, imposedRotationAtLineEnd;
  list `_assignedTo` (object_id, object_table, objectSupport_id, objectSupport_table).

## Building a model from scratch (`rf6-build`)

1. Template: a real `.rf6` with every model object purged (main, impl, owned part and list
   rows; list entries elsewhere that referenced them). Materials and sections are kept
   (library data). `!solver/` is dropped and `general_data.xml` updated.
2. Each new object clones a prototype row tree from a real file with fresh ids / GUIDs;
   references inside the tree are remapped, leftovers pointing outside are cleared using the
   foreign-key check.
3. Field values are then written like any edit.
