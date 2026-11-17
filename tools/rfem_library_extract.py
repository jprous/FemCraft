"""Builds FemCraft's own material / section library databases through a running RFEM 6.

RFEM's library databases are encrypted, and API I cannot list them, so each library item is
created *by name* in a scratch model; names RFEM does not know are rejected. The scratch model
is saved, and its `model.db` (RFEM's own table layout) becomes the library: FemCraft clones
row trees out of it exactly like it clones prototypes, and RFEM recognises the copies through
`MaterialImpl.databaseMaterialId` / `DatabaseSection.databaseSectionId`.

Two index tables are added for fast lookup (see `femcraft_rfdb::library`):

    femcraft_library(kind, user_id, name, designation, standard, manufacturer, region, family,
                     category, rfem_library_id, rfem_version)
    femcraft_library_value(kind, user_id, key, idx, value_si, value_text, thickness_si)

Usage (RFEM running with its API enabled on port 8081):

    python -I tools/rfem_library_extract.py materials OUT_DIR [--names FILE]
    python -I tools/rfem_library_extract.py sections OUT_DIR --names FILE

`--names` files hold one RFEM name per line (`#` comments). Material candidates are generated
when no file is given (Eurocode + South African concrete, reinforcement, structural steel).
Writes OUT_DIR/<kind>.rf6 (RFEM's file), OUT_DIR/<kind>.db and OUT_DIR/<kind>.log.
"""
import argparse
import itertools
import os
import re
import sqlite3
import sys
import time
import zipfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from rfem_soap import Client, SoapFault  # noqa: E402

APP_URL = "http://127.0.0.1:8081"

# --- material candidates -------------------------------------------------------------------

CONCRETE = ["C12/15", "C16/20", "C20/25", "C25/30", "C30/37", "C35/45", "C40/50", "C45/55",
            "C50/60", "C55/67", "C60/75", "C70/85", "C80/95", "C90/105", "C100/115"]
CONCRETE_STD = ["EN 1992-1-1:2004/A1:2014", "EN 1992-1-1:2023"]
REBAR = ["B500A", "B500B", "B500C", "B500S(A)", "B500S(B)", "B500S(C)", "B450C", "B550B",
         "B600B", "B400B", "B420B"]
REBAR_STD = CONCRETE_STD + ["EN 10080:2005"]
STEEL_BASE = ["S235", "S275", "S355", "S420", "S450", "S460", "S500", "S550", "S620", "S690"]
STEEL_QUALITY = ["", "JR", "J0", "J2", "K2", "N", "NL", "M", "ML", "Q", "QL", "QL1", "W",
                 "JRH", "J0H", "J2H", "K2H", "NH", "NLH", "MH", "MLH", "J2W", "J0W", "J2+N",
                 "J2C+N"]
STEEL_STD = [
    "EN 1993-1-1:2005-05", "EN 1993-1-1:2022", "EN 1993-1-1:2022-11",
    "EN 10025-2:2004-11", "EN 10025-2:2019-08", "EN 10025-3:2004-11", "EN 10025-3:2019-08",
    "EN 10025-4:2004-11", "EN 10025-4:2019-08", "EN 10025-5:2004-11", "EN 10025-5:2019-08",
    "EN 10025-6:2004-11", "EN 10025-6:2019-08", "EN 10210-1:2006-04", "EN 10219-1:2006-04",
    "SANS 50025-2", "SANS 50025-3", "SANS 50025-4", "SANS 50025-5", "SANS 50025-6",
    "SANS 50210-1", "SANS 50219-1", "SANS 1431",
]
COLD = ([f"S{g}GD" for g in (220, 250, 280, 320, 350, 390, 420, 450, 550)]
        + [f"S{g}MC" for g in (315, 355, 420, 460, 500, 550, 600, 650, 700)]
        + ["DC01", "DC03", "DC04", "DC05", "DC06", "DX51D", "DX52D", "DX53D", "DX54D"])
COLD_STD = ["EN 10346:2015", "EN 10346:2015-10", "EN 10346:2009", "EN 10149-2:2013-12",
            "EN 10149-2:2013", "EN 10130:2006", "EN 1993-1-3:2006", "EN 1993-1-3:2024",
            "SANS 10346", "SANS 3575", "SANS 4998"]
# SA designations RFEM may hold under its own names (probed bare and with SANS standards).
SA_EXTRA = ["300WA", "350WA", "300W", "350W", "25", "30", "35", "40", "Y450", "R250", "450"]
SA_STD = ["SANS 10100-1", "SANS 10100-1:2000", "SANS 920", "SANS 920:2011", "SANS 1431",
          "SANS 10162-1", "SANS 10162-2"]


def material_candidates():
    names = []
    for g in CONCRETE:
        names += [g] + [f"{g} | {s}" for s in CONCRETE_STD]
    for g in REBAR:
        names += [g] + [f"{g} | {s}" for s in REBAR_STD]
    for b, q in itertools.product(STEEL_BASE, STEEL_QUALITY):
        names += [b + q] + [f"{b}{q} | {s}" for s in STEEL_STD]
    for g in COLD:
        names += [g] + [f"{g} | {s}" for s in COLD_STD]
    for g in SA_EXTRA:
        names += [f"{g} | {s}" for s in SA_STD]
    return list(dict.fromkeys(names))


MATERIAL_TYPES = {"TYPE_CONCRETE": "concrete", "TYPE_REINFORCING_STEEL": "reinforcement",
                  "TYPE_STEEL": "steel"}
COLD_RE = re.compile(r"GD|MC$|^DC0|^DX5|EN 10346|EN 10149|EN 10130|EN 1993-1-3|EN 10219")


def region_of(standard, manufacturer):
    s = f"{standard} {manufacturer}"
    if "SANS" in s or "Macsteel" in s or "SAISC" in s:
        return "ZA"
    if re.search(r"\bEN\b|BS EN|DIN EN", s) or standard.startswith("EN"):
        return "EU"
    return ""


def split_name(name):
    parts = [p.strip() for p in name.split("|")]
    parts += ["--"] * (3 - len(parts))
    clean = lambda p: "" if p == "--" else p  # noqa: E731
    return parts[0], clean(parts[1]), clean(parts[2])


# --- RFEM probing ---------------------------------------------------------------------------

class Probe:
    def __init__(self, kind, log):
        self.kind = kind  # "material" / "cross_section"
        self.log = log
        self.app = Client(APP_URL)
        self.model_name = f"FemCraft {kind} library {time.strftime('%Y%m%d-%H%M%S')}"
        url = self.app.call("new_model", model_name=self.model_name)["value"]
        self.model = Client(url)
        self.found = {}  # resolved name -> (no, api object, index overrides)
        self.no = itertools.count(1)
        # RFEM cannot delete a half-created object left by a rejected name: its number is
        # reused (setting a valid name over it replaces it).
        self.slot = None

    def delete(self, no):
        object_type = {"material": "E_OBJECT_TYPE_MATERIAL",
                       "cross_section": "E_OBJECT_TYPE_CROSS_SECTION"}[self.kind]
        try:
            self.model.call("delete_object", type=object_type, no=no)
        except SoapFault:
            self.slot = no

    def try_name(self, name, extra=None, check=None, meta=None):
        """Creates `name`; keeps it when RFEM knows it and `check(obj)` (if given) agrees."""
        no, self.slot = (self.slot, None) if self.slot else (next(self.no), None)
        value = {"no": no, "name": name, **(extra or {})}
        try:
            self.model.call(f"set_{self.kind}", value=value)
            obj = self.model.call(f"get_{self.kind}", no=no)["value"]
        except SoapFault as e:
            # A rejected name can still leave an object behind.
            self.delete(no)
            self.log(f"miss  {name!r}: {str(e)[:100]}")
            return None
        got = obj["name"]
        # RFEM matches loosely ("C30/37 | SANS 10100" -> the EN grade): with a standard given,
        # only an exact answer counts.
        why = ("not exact" if "|" in name and got != name else "duplicate" if got in self.found
               else "rejected" if not self.accept(obj) else check(obj) if check else None)
        if why:
            self.delete(no)
            self.log(f"skip  {name!r} -> {got!r}: {why}")
            return None
        self.found[got] = (no, obj, meta or {})
        self.log(f"found {name!r} -> {got!r} (no {no})")
        return obj

    def accept(self, obj):
        if self.kind != "material":
            return True
        if obj.get("material_type") not in MATERIAL_TYPES or obj.get("user_defined"):
            return False
        _, std, man = split_name(obj["name"])
        return region_of(std, man) in ("EU", "ZA")

    def save_and_close(self, path):
        # RFEM refuses to overwrite an existing file.
        for p in (path, path + "bak"):
            if os.path.exists(p):
                os.remove(p)
        # Saving renames the model after the file: find its index first.
        models = self.app.call("get_model_list_with_indexes")["value"]["model_name_and_index"]
        index = [m["index"] for m in (models if isinstance(models, list) else [models])
                 if m["name"] == self.model_name]
        self.model.call("save", file_path=path)
        # `save` returns before RFEM has written the file, and closing the model then drops
        # it: wait until the file exists and its size has settled.
        last, stable, t0 = -1, 0, time.time()
        while stable < 3:
            if time.time() - t0 > 600:
                raise RuntimeError(f"RFEM did not write {path}; the model is left open")
            time.sleep(1)
            size = os.path.getsize(path) if os.path.exists(path) else -1
            stable = stable + 1 if size == last and size > 0 else 0
            last = size
        if index:
            self.app.call("close_model", index=index[0], save_changes=False)


# --- library database ----------------------------------------------------------------------

INDEX_SQL = """
DROP TABLE IF EXISTS femcraft_library;
DROP TABLE IF EXISTS femcraft_library_value;
CREATE TABLE femcraft_library (
    kind TEXT NOT NULL, user_id INTEGER NOT NULL, name TEXT NOT NULL, designation TEXT NOT NULL,
    standard TEXT NOT NULL, manufacturer TEXT NOT NULL, region TEXT NOT NULL,
    family TEXT NOT NULL, category TEXT NOT NULL, rfem_library_id INTEGER,
    rfem_version TEXT NOT NULL, PRIMARY KEY (kind, user_id));
CREATE INDEX femcraft_library_name ON femcraft_library(name);
CREATE TABLE femcraft_library_value (
    kind TEXT NOT NULL, user_id INTEGER NOT NULL, key TEXT NOT NULL, idx INTEGER NOT NULL,
    value_si REAL, value_text TEXT, thickness_si REAL, PRIMARY KEY (kind, user_id, key, idx));
"""


def family_of(designation):
    m = re.match(r"[A-Za-z]+", designation)
    return m.group(0).upper() if m else designation.split()[0]


def material_rows(db, user_id):
    """(key, idx, value_si, value_text, thickness_si) of a material's property map."""
    data = db.execute(
        "SELECT d.properties_id FROM Material m JOIN MaterialImpl_materialDataVector v "
        "ON v.id = m.impl_id JOIN MaterialData d ON d.id = v.reference_id "
        "WHERE m.userID = ? ORDER BY v.container_order LIMIT 1", (user_id,)).fetchone()
    if not data:
        return []
    out = []
    entries = db.execute(
        "SELECT k.value, v.reference_id, v.reference_table FROM "
        "MaterialProperties_propertiesMap_keys k JOIN MaterialProperties_propertiesMap_values v "
        "ON v.id = k.id AND v.container_order = k.container_order WHERE k.id = ? "
        "ORDER BY k.container_order", (data[0],)).fetchall()
    for key, rid, rtable in entries:
        if rtable == "MaterialPropertyDouble":
            (v,) = db.execute("SELECT valueSI FROM MaterialPropertyDouble WHERE id=?", (rid,)).fetchone()
            out.append((key, 0, v, None, None))
        elif rtable in ("MaterialPropertyInt", "MaterialPropertyBool"):
            (v,) = db.execute(f"SELECT value FROM {rtable} WHERE id=?", (rid,)).fetchone()
            out.append((key, 0, float(v), None, None))
        elif rtable == "MaterialPropertyString":
            (v,) = db.execute("SELECT value FROM MaterialPropertyString WHERE id=?", (rid,)).fetchone()
            out.append((key, 0, None, v, None))
        elif rtable == "MaterialPropertyRange":
            rows = db.execute(
                "SELECT mv.valueSI, mv.thicknessSI FROM MaterialPropertyRange_values r JOIN "
                "MaterialPropertyMaterialValue mv ON mv.id = r.reference_id WHERE r.id = ? "
                "ORDER BY r.container_order", (rid,)).fetchall()
            out += [(key, i, v, None, t) for i, (v, t) in enumerate(rows)]
    return out


def material_library_id(db, user_id):
    r = db.execute("SELECT i.databaseMaterialId FROM Material m JOIN MaterialImpl i "
                   "ON i.id = m.impl_id WHERE m.userID = ?", (user_id,)).fetchone()
    return r[0] if r else None


def section_data(db, user_id):
    """(databaseSectionId, sectionData id) of a library section."""
    (impl_table, impl_id) = db.execute(
        "SELECT impl_table, impl_id FROM Section WHERE userID = ?", (user_id,)).fetchone()
    lib = db.execute(f'SELECT librarySection_table, librarySection_id FROM "{impl_table}" '
                     "WHERE id = ?", (impl_id,)).fetchone()
    if not lib or not lib[0]:
        return None, None
    cols = [r[1] for r in db.execute(f'PRAGMA table_info("{lib[0]}")')]
    lib_id = "databaseSectionId" if "databaseSectionId" in cols else "NULL"
    return db.execute(f'SELECT {lib_id}, sectionData_id FROM "{lib[0]}" WHERE id = ?',
                      (lib[1],)).fetchone()


def section_rows(db, data_id):
    rows = db.execute(
        "SELECT k.value, v.valueSI FROM SectionData_parameterValues_keys k JOIN "
        "SectionData_parameterValues_values v ON v.id = k.id AND v.container_order = "
        "k.container_order WHERE k.id = ? ORDER BY k.container_order", (data_id,)).fetchall()
    return [(k, 0, v, None, None) for k, v in rows]


def build_db(kind, rf6, out_db, found, version):
    with zipfile.ZipFile(rf6) as z, open(out_db, "wb") as f:
        f.write(z.read("model.db"))
    db = sqlite3.connect(out_db)
    db.executescript(INDEX_SQL)
    lib_kind = "material" if kind == "material" else "section"
    for name, (no, obj, meta) in sorted(found.items(), key=lambda x: x[1][0]):
        designation, standard, manufacturer = split_name(name)
        if kind == "material":
            category = MATERIAL_TYPES[obj["material_type"]]
            if category == "steel":
                category = "steel_cold_formed" if COLD_RE.search(name) else "steel_hot_rolled"
            lib_id, values = material_library_id(db, no), material_rows(db, no)
        else:
            category = {"MANUFACTURING_TYPE_HOT_ROLLED": "hot_rolled",
                        "MANUFACTURING_TYPE_COLD_FORMED": "cold_formed",
                        "MANUFACTURING_TYPE_WELDED": "welded"}.get(
                obj.get("manufacturing_type"), "other")
            if obj.get("type", "").startswith("TYPE_PARAMETRIC_MASSIVE"):
                category = "concrete"
            lib_id, data_id = section_data(db, no)
            values = section_rows(db, data_id) if data_id else []
        region = meta.get("region") or region_of(standard, manufacturer)
        db.execute("INSERT INTO femcraft_library VALUES (?,?,?,?,?,?,?,?,?,?,?)",
                   (lib_kind, no, name, designation, standard, manufacturer, region,
                    family_of(designation), category, lib_id, version))
        db.executemany("INSERT OR REPLACE INTO femcraft_library_value VALUES (?,?,?,?,?,?,?)",
                       [(lib_kind, no, *v) for v in values])
    db.commit()
    db.execute("VACUUM")
    db.close()


# MaterialImpl.materialType codes (RFEM 6.16).
MATERIAL_TYPE_CODES = {1: "TYPE_CONCRETE", 9: "TYPE_REINFORCING_STEEL", 10: "TYPE_STEEL"}


def build_from_rf6(rf6, out_db, names):
    """Index for a saved extraction model whose material `i` is `names[i - 1]` (a replay of
    exact names, e.g. the `found` lines of an earlier log, saved from RFEM by hand)."""
    tmp = out_db + ".tmp"
    with zipfile.ZipFile(rf6) as z, open(tmp, "wb") as f:
        f.write(z.read("model.db"))
    db = sqlite3.connect(tmp)
    rows = dict(db.execute("SELECT m.userID, i.materialType FROM Material m JOIN MaterialImpl i "
                           "ON i.id = m.impl_id").fetchall())
    db.close()
    if sorted(rows) != list(range(1, len(names) + 1)):
        sys.exit(f"{rf6} holds materials {sorted(rows)[:5]}..., not 1..{len(names)}")
    found = {n: (i, {"material_type": MATERIAL_TYPE_CODES.get(rows[i], "?")}, {})
             for i, n in enumerate(names, 1)}
    with zipfile.ZipFile(rf6) as z:
        info = z.read("format.txt").decode("utf-8", "replace") if "format.txt" in z.namelist() else ""
    version = (re.search(r"\d+\.\d+\.\d+", info) or [""])[0]
    os.remove(tmp)
    build_db("material", rf6, out_db, found, version)
    print(f"{len(found)} materials -> {out_db}")


def prokon_jobs(path, steel, concrete, log):
    """South African and European steel sections of a Prokon export, plus its concrete
    rectangles / squares / circles as RFEM parametric sections. Structural tees, timber, solid
    steel and custom sections are left out (no RFEM library equivalent found)."""
    import prokon_sections as P
    secs = P.parse(path)
    jobs = []
    for s in secs:
        names = P.rfem_names(s)
        if not names or s["block"] == "Structural tees":
            continue
        area = P.area_m2(s)

        def check(obj, area=area, d=s["designation"]):
            a = obj.get("A")
            if area and a and abs(a - area) > 0.03 * area:
                return f"area {a:.6g} differs from Prokon {d} {area:.6g}"
            return None
        jobs.append((names, steel, check, {"region": s["region"]}))
    za = [s for s in secs if s["region"] == "ZA"]
    jobs += [([n], concrete, None, {"region": "ZA"}) for n in P.concrete_names(za)]
    log(f"{len(jobs)} Prokon sections to look up")
    return jobs


def read_names(path):
    with open(path, encoding="utf-8") as f:
        return [l.strip() for l in f if l.strip() and not l.lstrip().startswith("#")]


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("kind", choices=["materials", "sections"])
    ap.add_argument("out_dir")
    ap.add_argument("--names", help="one RFEM name per line")
    ap.add_argument("--prokon", help="sections: a Prokon Sections.dat export (see "
                    "prokon_sections.py); its South African and European sections")
    ap.add_argument("--replay", help="with --prokon: an earlier log; only the names it "
                    "found are tried again (same order, regions and checks)")
    ap.add_argument("--rf6-dir", default=os.path.join(os.path.expanduser("~"), "Documents",
                    "FemCraft", "library"),
                    help="where RFEM saves the extraction model (RFEM does not write into "
                    "%%APPDATA%%)")
    ap.add_argument("--from-rf6", help="materials: build the .db from this saved extraction "
                    "model instead of probing RFEM; --names lists its materials in order")
    ap.add_argument("--concrete-material", default="C30/37 | EN 1992-1-1:2023",
                    help="material the probed concrete sections refer to")
    ap.add_argument("--section-material", default="S355JR | EN 10025-2:2019-08",
                    help="material the probed sections refer to")
    a = ap.parse_args()
    os.makedirs(a.out_dir, exist_ok=True)
    kind = "material" if a.kind == "materials" else "cross_section"
    stem = os.path.join(os.path.abspath(a.out_dir), a.kind)
    logf = open(stem + ".log", "w", encoding="utf-8")

    def log(msg):
        logf.write(msg + "\n")
        logf.flush()

    if a.from_rf6:
        if kind != "material" or not a.names:
            sys.exit("--from-rf6 needs materials and --names")
        build_from_rf6(a.from_rf6, stem + ".db", read_names(a.names))
        return
    # Each job: (candidate names, first accepted wins; set_ extras; check; index overrides).
    steel, concrete = {"material": 1}, {"material": 2}
    if a.names:
        jobs = [([n], steel if kind == "cross_section" else None, None, None)
                for n in read_names(a.names)]
    elif kind == "material":
        jobs = [([n], None, None, None) for n in material_candidates()]
    elif a.prokon:
        jobs = prokon_jobs(a.prokon, steel, concrete, log)
        if a.replay:
            with open(a.replay, encoding="utf-8") as f:
                hits = set(re.findall(r"^found '(.*)' -> ", f.read(), re.M))
            jobs = [([n for n in names if n in hits], *rest) for names, *rest in jobs]
            jobs = [j for j in jobs if j[0]]
    else:
        sys.exit("sections need --names or --prokon")
    probe = Probe(kind, log)
    version = probe.app.call("get_information")["value"].get("version", "")
    if kind == "cross_section":
        # The materials belong to the library too: sections refer to them.
        probe.model.call("set_material", value={"no": 1, "name": a.section_material})
        probe.model.call("set_material", value={"no": 2, "name": a.concrete_material})
    t0 = time.time()
    for i, (names, extra, check, meta) in enumerate(jobs, 1):
        if not any(probe.try_name(n, extra, check, meta) for n in names):
            log(f"NONE  {names}")
        if i % 100 == 0:
            print(f"{i}/{len(jobs)} probed, {len(probe.found)} found, "
                  f"{time.time() - t0:.0f} s", flush=True)
    if probe.slot:
        probe.log(f"note: object {probe.slot} is a rejected leftover RFEM would not delete")
    os.makedirs(a.rf6_dir, exist_ok=True)
    rf6 = os.path.join(os.path.abspath(a.rf6_dir), a.kind + ".rf6")
    probe.save_and_close(rf6)
    build_db(kind, rf6, stem + ".db", probe.found, version)
    print(f"{len(probe.found)} {a.kind} -> {stem}.db")


if __name__ == "__main__":
    main()
