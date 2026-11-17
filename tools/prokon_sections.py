"""Reads a Prokon section database export (`Sections.dat`, "PROKON DATABASE OF STRUCTURAL STEEL
SECTIONS", file version 2) and proposes RFEM 6 library names for its sections.

The file is a text table per section block ("I-sections (Parallel flange)", ...), each split
by country marker rows (`|>South Afri|`, `|>Euro|`). Only rows with a numeric second column
are sections; the others are group labels ("WIDE FLANGE", "EXAMPLES").

    python -I tools/prokon_sections.py Sections.dat [--region ZA|EU] [--names OUT.txt]

lists the sections (block, country, designation, area) or writes RFEM candidate names for
`rfem_library_extract.py sections --prokon`.
"""
import argparse
import re

REGIONS = {"South Afri": "ZA", "Euro": "EU"}


def num(s):
    s = s.strip()
    try:
        return float(s)
    except ValueError:
        return None


def parse(path):
    """Sections as dicts: block, region, designation, values {column: (value, unit)}."""
    lines = open(path, encoding="latin-1").read().splitlines()
    out = []
    block = cols = units = None
    region = None
    for i, line in enumerate(lines):
        if line and not line.startswith(("|", " ", "-")) and i > 16:
            block, cols, units, region = line.strip(), None, None, None
            continue
        if not line.startswith("|") or block is None:
            continue
        cells = [c.strip() for c in re.split(r"[|!]", line)[1:-1]]
        if cols is None:  # first table row of a block: column names, then units
            cols = cells
            continue
        if cols and units is None:
            units = cells
            continue
        m = re.match(r"\|>([^|!]*)", line)
        if m:
            name = m.group(1).strip()
            region = next((r for k, r in REGIONS.items() if name.startswith(k)), None)
            continue
        if region is None or len(cells) < 3 or num(cells[1]) is None:
            continue
        values = {}
        for c, u, v in zip(cols[1:], units[1:], cells[1:]):
            x = num(v)
            if c and x is not None:
                values.setdefault(c, (x, u))
        out.append({"block": block, "region": region, "designation": cells[0],
                    "values": values})
    return out


def area_m2(sec):
    """Prokon's area in m² (column `A`, unit `E3` = 10³ mm²), or None."""
    v = sec["values"].get("A")
    if not v:
        return None
    x, unit = v
    scale = {"E3": 1e3, "E6": 1e6, "": 1.0, "mm²": 1.0}.get(unit.replace(" ", ""), None)
    return None if scale is None else x * scale * 1e-6


# RFEM naming per Prokon block, South Africa (Macsteel library; confirmed examples in comments).
def _sa(block, d):
    d = d.replace(" ", "")
    if block == "I-sections (Parallel flange)":
        m = re.match(r"IPE(-AA)?(\d+)$", d)
        if m:  # IPE 100 | DIN 1025:1965 | Macsteel
            return [f"IPE {'AA ' if m.group(1) else ''}{m.group(2)}",
                    f"IPE {m.group(2)}{' AA' if m.group(1) else ''}"]
        return [f"UB {d}", f"I {d}"]  # UB 203x133x25 | BS 4-1:1993 | Macsteel
    if block == "H-sections (Parallel flange)":
        return [f"UC {d}", f"H {d}"]  # UC 152x152x23 | BS 4-1:1993 | Macsteel
    if block == "I-sections (Taper flange)":
        return [f"TFB {d}", f"UB {d}", f"I {d}", f"J {d}"]
    if block == "Channels (Parallel flange)":
        return [f"PFC {d.removeprefix('PC')}"]  # PFC 100x50 | EN 10279 | Macsteel
    if block == "Channels (Taper flange)":
        return [f"TFC {d.removeprefix('C')}"]  # TFC 76x38 | BS 4-1:1993 | Macsteel
    if block.startswith("Angles"):
        return [f"L {d}"]  # L 65x50x6 | EN 10056 | Macsteel
    if block == "Circular hollow sections":
        return [f"CHS {d}"]  # CHS 48.4x2.5 | -- | Macsteel
    if block == "Rectangular hollow sections":
        return [f"RHS {d}"]
    if block == "Square hollow sections":
        return [f"SHS {d}"]
    if block == "Cold-formed lipped channels":
        return [f"CFC {d}", f"CFLC {d}", f"LC {d}"]  # CFC 75x50x20x2.0 | -- | Macsteel
    if block == "Cold-formed lipped angles":
        return [f"CFLA {d}", f"CFA {d}", f"LA {d}"]
    if block == "Cold-formed lipped zeds":
        return [f"CFZ {d}", f"CFLZ {d}", f"LZ {d}", f"Z {d}"]
    return []


def _eu(block, d):
    d = d.replace(" ", "")
    m2 = re.match(r"(HD|HP|HL|HLZ|W|IPE|HE)(\d+)x(.+)$", d)
    if m2:
        return [f"{m2.group(1)} {m2.group(2)}x{m2.group(3)}", f"{m2.group(1)} {m2.group(2)} x {m2.group(3)}"]
    m = re.match(r"([A-Z]+?)(\d+)([A-Z]*)$", d)
    if block.startswith(("I-sections", "H-sections")) and m:
        fam, size, suf = m.groups()
        if fam == "HE":  # HE 200 A
            return [f"HE {size} {suf}".strip(), f"HE{suf} {size}"]
        if fam == "IPE" and suf:  # IPE A 100, IPE O 100
            return [f"IPE {suf} {size}", f"IPE {size} {suf}"]
        return [f"{fam} {size} {suf}".strip(), f"{fam} {size}{suf}"]
    if block.startswith("Channels") and m:
        return [f"{m.group(1)} {m.group(2)}{m.group(3)}"]
    if block.startswith("Angles"):
        return [f"L {d}"]
    return []


def candidates(sec):
    """Designations to try in RFEM, best first."""
    f = _sa if sec["region"] == "ZA" else _eu
    return f(sec["block"], sec["designation"])


# RFEM library suffixes ("standard | manufacturer") per designation prefix, confirmed on
# RFEM 6.16. South Africa: Macsteel's catalogue; Europe: ArcelorMittal's EN 10365 catalogue.
SUFFIX = {
    "ZA": {"IPE": "DIN 1025:1965 | Macsteel", "UB": "BS 4-1:1993 | Macsteel",
           "UC": "BS 4-1:1993 | Macsteel", "TFC": "BS 4-1:1993 | Macsteel",
           "I": "EN 10024 | Macsteel", "PFC": "EN 10279 | Macsteel", "L": "EN 10056 | Macsteel",
           "CHS": "-- | Macsteel", "RHS": "-- | Macsteel", "SHS": "-- | Macsteel",
           "CFC": "-- | Macsteel", "CFZ": "-- | Macsteel"},
    "EU": {p: "EN 10365:2017 | ArcelorMittal (2018)"
           for p in ("IPE", "HE", "HEA", "HEB", "HEM", "HD", "HP", "HL", "HLZ", "IPN", "UPN",
                     "UPE", "UAP", "PFC")},
}


def rfem_names(sec):
    """Full RFEM names to try: the regional catalogue first, then the bare designation
    (RFEM picks a manufacturer; the caller checks the area)."""
    designations = candidates(sec)
    table = SUFFIX.get(sec["region"], {})
    full = [f"{d} | {table[d.split()[0]]}" for d in designations if d.split()[0] in table]
    return list(dict.fromkeys(full + designations))


# Concrete blocks: RFEM parametric massive sections, named in metres.
def concrete_names(secs):
    out = []
    for s in secs:
        d = s["designation"].replace(" ", "")
        if s["block"] == "Rectangular solid concrete sections":
            m = re.match(r"(\d+)x(\d+)$", d)
            if m:
                b, h = int(m.group(1)) / 1000, int(m.group(2)) / 1000
                out.append(f"SQ_M1 {b:g}" if b == h else f"R_M1 {b:g}/{h:g}")
        elif s["block"] == "Circular solid concrete sections":
            m = re.match(r"Dia(\d+)$", d)
            if m:
                out.append(f"CIRCLE_M1 {int(m.group(1)) / 1000:g}")
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("file")
    ap.add_argument("--region", choices=["ZA", "EU"])
    a = ap.parse_args()
    for s in parse(a.file):
        if a.region and s["region"] != a.region:
            continue
        print(f"{s['region']} | {s['block']} | {s['designation']} | A={area_m2(s)} | "
              f"{candidates(s)}")


if __name__ == "__main__":
    main()
