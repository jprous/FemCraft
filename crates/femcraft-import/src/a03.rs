//! Prokon Frame `.A03` reports: a text file of pipe-delimited tables (nodes, beams, shells,
//! supports, loads, materials, ...), read into the same [`StaadModel`] the STAAD importer
//! uses, so surfaces are rebuilt and supports / loads folded onto them the same way.
//!
//! Read: units, nodes (with `No. of` / `Node Inc` generation and increments or end values),
//! beam elements (`n1-n2`, section, beta) and the section table, 3 / 4 node shells (with
//! generation, thickness and material carried down), supports (`XYZxyz`, springs `S`), shell
//! pressures (`Udl` per shell list, global X / Y / Z or local L), load combinations (counted),
//! the self-weight case and the material table. Prokon is Y up: the model keeps its own axes
//! (`z_up` false) and the importer turns Y into RFEM's Z down. Everything else is counted in
//! [`StaadModel::skipped`].

use std::collections::BTreeMap;

use crate::staad::{
    ElementLoad, Material, MemberLoad, Pressure, PressureDir, Prismatic, StaadModel,
};

/// Shells share the number series of nothing else in the file; offset them so a shell and a
/// beam of the same number never share a material / thickness entry.
pub const SHELL_BASE: u32 = 1_000_000;

fn cells(line: &str) -> Vec<&str> {
    let t = line.trim_end();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    // Header rows also use a broken bar; data rows only `|`.
    t.split('|').map(str::trim).collect()
}

fn num(s: &str) -> Option<f64> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.parse().ok()).flatten()
}

fn int(s: &str) -> Option<u32> {
    s.trim().parse().ok()
}

fn is_border(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && t.chars().all(|c| c == '-' || c == ' ')
}

/// `1,2,4-12,18` -> numbers.
fn list(s: &str) -> Vec<u32> {
    let mut out = Vec::new();
    for part in s.split([',', ' ']).filter(|p| !p.is_empty()) {
        if let Some((a, b)) = part.split_once('-') {
            if let (Some(a), Some(b)) = (int(a), int(b)) {
                out.extend(a..=b);
            }
        } else if let Some(n) = int(part) {
            out.push(n);
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Table {
    None,
    Nodes,
    Beams,
    Sections,
    Shells,
    Supports,
    BeamLoads,
    ShellLoads,
    Combos,
    Materials,
    /// Tables whose rows are not converted: counted under this name.
    Skipped(&'static str),
}

pub fn parse(text: &str) -> Result<StaadModel, String> {
    if !text.contains("Frame Analysis") && !text.contains("Beam element definition") {
        return Err("not a Prokon Frame (.A03) report".into());
    }
    let mut m = StaadModel::default();
    let (mut l, mut f) = (1.0, 1.0);
    let mut table = Table::None;
    let mut sections: BTreeMap<String, (Prismatic, String)> = BTreeMap::new();
    let mut shell_no = 0u32;
    let (mut thick, mut material) = (0.0, String::new());
    // Load case names are matched without regard to case (`1in200` = `1IN200`).
    let mut lc_of: BTreeMap<String, u32> = BTreeMap::new();
    let mut lc_no = |m: &mut StaadModel, name: &str| -> u32 {
        let next = lc_of.len() as u32 + 1;
        let no = *lc_of.entry(name.to_uppercase()).or_insert(next);
        m.load_cases.entry(no).or_insert_with(|| name.to_owned());
        no
    };
    let mut shell_lc = String::new();
    let mut beam_lc = String::new();
    let mut by_pair: BTreeMap<(u32, u32), u32> = BTreeMap::new();
    let mut self_weight: Option<String> = None;
    let mut in_materials = false;

    for raw in text.lines() {
        let line = raw.trim_end();
        if let Some(rest) = line.trim().strip_prefix("Units:") {
            if rest.contains("Distance:mm") {
                l = 1e-3;
            } else if rest.contains("Distance:cm") {
                l = 1e-2;
            }
            if rest.contains("Force:N") && !rest.contains("Force:kN") {
                f = 1.0;
            } else if rest.contains("Force:kN") {
                f = 1e3;
            }
            continue;
        }
        if let Some(rest) = line.split("Self weight to be added to:").nth(1) {
            self_weight = Some(rest.trim().to_owned());
            continue;
        }
        if line.trim() == "Materials" {
            in_materials = true;
            table = Table::Materials;
            continue;
        }
        // Table headers (the first header line carries the table's own column names).
        if line.starts_with('|') || line.starts_with('¦') {
            let h = line;
            let new = if h.contains("X-inc") {
                Some(Table::Nodes)
            } else if h.contains("Beam element definition") && h.contains("Direction") {
                Some(Table::BeamLoads)
            } else if h.contains("Beam element definition") && h.contains("Releases") {
                Some(Table::Beams)
            } else if h.contains("Beam section designation") {
                Some(Table::Sections)
            } else if h.contains("Shell numbers") && h.contains("Direction") {
                Some(Table::ShellLoads)
            } else if h.contains("Shell numbers") && h.contains("Edge No") {
                Some(Table::Skipped("shell edge loads"))
            } else if h.contains("Fixity") {
                Some(Table::Supports)
            } else if h.contains("|Rigid|GN") {
                Some(Table::Shells)
            } else if h.contains("Cable element definition") {
                Some(Table::Skipped("cable elements"))
            } else if h.contains("Spring element") {
                Some(Table::Skipped("spring elements"))
            } else if h.contains("ULS") && h.contains("SLS") && h.contains("Load") {
                Some(Table::Combos)
            } else if h.starts_with("|Load") && h.contains("Node") && h.contains("Px") {
                Some(Table::Skipped("nodal loads"))
            } else {
                None
            };
            if let Some(t) = new {
                table = t;
                if t != Table::Materials {
                    in_materials = false;
                }
                continue;
            }
        }
        if !line.starts_with('|') {
            // A second header line starts with a broken bar (`¦`, or U+FFFD when read lossily).
            let t = line.trim_start();
            let text_line = t.is_empty() || t.chars().next().is_some_and(|ch| ch.is_alphanumeric());
            if !is_border(line) && !in_materials && text_line {
                table = Table::None;
            } else if !is_border(line) && in_materials && !line.trim().is_empty() {
                // Text after the material table (`Views`, ...).
                in_materials = false;
                table = Table::None;
            }
            continue;
        }
        let c = cells(line);
        // Blank rows, and the second / third header lines (`e.g. ...`, `(kN)`, `of extra`).
        if c.iter().all(|x| x.is_empty())
            || line.contains("e.g.")
            || c.iter()
                .any(|x| x.starts_with('(') || x.starts_with("of extra"))
        {
            continue;
        }
        match table {
            Table::Nodes => {
                let Some(no) = int(c[0]) else { continue };
                if c.len() < 4 {
                    continue;
                }
                let start = [
                    num(c[1]).unwrap_or(0.0) * l,
                    num(c[2]).unwrap_or(0.0) * l,
                    num(c[3]).unwrap_or(0.0) * l,
                ];
                m.joints.insert(no, start);
                let extra = c.get(4).and_then(|s| int(s)).unwrap_or(0);
                let inc = c.get(5).and_then(|s| int(s)).unwrap_or(1).max(1);
                let use_end = c.get(9).is_some_and(|s| s.eq_ignore_ascii_case("Y"));
                let step: [f64; 3] = std::array::from_fn(|i| {
                    let v = c.get(6 + i).and_then(|s| num(s)).unwrap_or(0.0) * l;
                    if use_end && extra > 0 {
                        (v - start[i]) / extra as f64
                    } else {
                        v
                    }
                });
                for k in 1..=extra {
                    let p = std::array::from_fn(|i| start[i] + step[i] * k as f64);
                    m.joints.insert(no + k * inc, p);
                }
            }
            Table::Sections => {
                if c.len() >= 9 && !c[0].is_empty() {
                    let p = Prismatic {
                        ax: num(c[2]).unwrap_or(0.0) * l * l,
                        ix: num(c[7]).unwrap_or(0.0) * l.powi(4),
                        iy: num(c[5]).unwrap_or(0.0) * l.powi(4),
                        iz: num(c[6]).unwrap_or(0.0) * l.powi(4),
                        ..Default::default()
                    };
                    sections.insert(c[0].to_owned(), (p, c[8].to_owned()));
                }
            }
            Table::Beams => {
                let Some((a, b)) = c[0].split_once('-') else {
                    continue;
                };
                let (Some(a), Some(b)) = (int(a), int(b.trim_start_matches('-'))) else {
                    continue;
                };
                let no = m.members.len() as u32 + 1;
                m.members.insert(no, (a, b));
                by_pair.insert((a, b), no);
                by_pair.insert((b, a), no);
                if let Some((p, mat)) = c.get(1).and_then(|s| sections.get(*s)) {
                    m.member_props.insert(no, *p);
                    if !mat.is_empty() {
                        m.element_material.insert(no, mat.clone());
                    }
                }
                if let Some(beta) = c.get(2).and_then(|s| num(s))
                    && beta != 0.0
                {
                    m.beta.insert(no, beta);
                }
            }
            Table::Shells => {
                // The second header line is `1 | 2 | 3 | 4 | of extra | ...`.
                if c.get(4)
                    .is_some_and(|s| s.chars().any(|ch| ch.is_alphabetic()))
                {
                    continue;
                }
                let n: Vec<u32> = c.iter().take(4).filter_map(|s| int(s)).collect();
                if n.len() < 3 {
                    continue;
                }
                if let Some(t) = c.get(6).and_then(|s| num(s)) {
                    thick = t * l;
                }
                if let Some(s) = c.get(7).filter(|s| !s.is_empty()) {
                    material = (*s).to_owned();
                }
                let extra = c.get(4).and_then(|s| int(s)).unwrap_or(0);
                let inc = c.get(5).and_then(|s| int(s)).unwrap_or(1).max(1);
                for k in 0..=extra {
                    shell_no += 1;
                    let id = SHELL_BASE + shell_no;
                    m.plates.insert(id, n.iter().map(|x| x + k * inc).collect());
                    if thick > 0.0 {
                        m.plate_thickness.insert(id, thick);
                    }
                    if !material.is_empty() {
                        m.element_material.insert(id, material.clone());
                    }
                }
            }
            Table::Supports => {
                let (Some(no), Some(fix)) = (int(c[0]), c.get(1)) else {
                    continue;
                };
                let flag = c.get(2).copied().unwrap_or("");
                if flag.eq_ignore_ascii_case("P") {
                    *m.skipped
                        .entry("prescribed displacements".into())
                        .or_default() += 1;
                    continue;
                }
                let mut d = [false; 6];
                for ch in fix.chars() {
                    if let Some(i) = "XYZxyz".find(ch) {
                        d[i] = true;
                    }
                }
                let mut springs = [0.0; 6];
                if flag.eq_ignore_ascii_case("S") {
                    for (i, s) in springs.iter_mut().enumerate() {
                        if let Some(v) = c.get(3 + i).and_then(|s| num(s)) {
                            d[i] = false;
                            *s = if i < 3 { v * f / l } else { v * f * l };
                        }
                    }
                }
                if !d.iter().any(|x| *x) && springs.iter().all(|s| *s == 0.0) {
                    continue;
                }
                let extra = c.get(9).and_then(|s| int(s)).unwrap_or(0);
                let inc = c.get(10).and_then(|s| int(s)).unwrap_or(1).max(1);
                for k in 0..=extra {
                    let n = no + k * inc;
                    m.supports.insert(n, d);
                    if springs.iter().any(|s| *s != 0.0) {
                        m.support_springs.insert(n, springs);
                    }
                }
            }
            Table::ShellLoads => {
                // The second header line: `Case | e.g. 1,2,4-12 | XYZ or L | (kN/m²)`.
                if c.get(1).is_some_and(|s| s.starts_with("e.g.")) {
                    continue;
                }
                if !c[0].is_empty() {
                    shell_lc = c[0].to_owned();
                }
                let (Some(dir), Some(udl)) = (
                    c.get(2).filter(|s| !s.is_empty()),
                    c.get(3).and_then(|s| num(s)),
                ) else {
                    if c.get(4).is_some_and(|s| !s.is_empty()) {
                        *m.skipped.entry("shell point loads".into()).or_default() += 1;
                    }
                    continue;
                };
                let dir = match dir.to_ascii_uppercase().as_str() {
                    "X" => PressureDir::Global(0),
                    "Y" => PressureDir::Global(1),
                    "Z" => PressureDir::Global(2),
                    "L" => PressureDir::Local(2),
                    _ => {
                        *m.skipped.entry("shell load direction".into()).or_default() += 1;
                        continue;
                    }
                };
                let lc = lc_no(&mut m, &shell_lc);
                let plates: Vec<u32> = list(c[1]).into_iter().map(|n| SHELL_BASE + n).collect();
                m.element_loads.push(ElementLoad {
                    lc,
                    plates,
                    dir,
                    value: Pressure::Uniform(udl * f / (l * l)),
                });
            }
            Table::BeamLoads => {
                let Some((a, b)) = c.get(1).and_then(|s| s.split_once('-')) else {
                    continue;
                };
                if !c[0].is_empty() {
                    beam_lc = c[0].to_owned();
                }
                let (Some(a), Some(b)) = (int(a), int(b)) else {
                    continue;
                };
                let lc = lc_no(&mut m, &beam_lc);
                let g = |i: usize| c.get(i).and_then(|s| num(s));
                let dir = match c.get(2).map(|s| s.to_ascii_uppercase()).as_deref() {
                    Some("X") => PressureDir::Global(0),
                    Some("Y") => PressureDir::Global(1),
                    Some("Z") => PressureDir::Global(2),
                    Some("L") => PressureDir::Local(2),
                    _ => {
                        *m.skipped
                            .entry("beam loads (other direction)".into())
                            .or_default() += 1;
                        continue;
                    }
                };
                let Some(&member) = by_pair.get(&(a, b)) else {
                    *m.skipped
                        .entry("beam loads on unknown beams".into())
                        .or_default() += 1;
                    continue;
                };
                let (low, high) = (g(5), g(6));
                let full = g(7).unwrap_or(0.0) == 0.0 && g(8).unwrap_or(0.0) == 0.0;
                match low {
                    Some(w)
                        if g(3).is_none() && full && high.is_none_or(|h| (h - w).abs() < 1e-12) =>
                    {
                        m.member_loads.push(MemberLoad {
                            lc,
                            member,
                            dir,
                            w: w * f / l,
                        });
                    }
                    _ => {
                        *m.skipped
                            .entry("beam loads (point, trapezoidal or partial)".into())
                            .or_default() += 1;
                    }
                }
            }
            Table::Combos => {
                if c.get(2).and_then(|s| num(s)).is_none() {
                    continue;
                }
                if !c[0].is_empty() {
                    m.combinations += 1;
                    m.combination_defs.push((c[0].to_owned(), Vec::new()));
                }
                if let Some(case) = c.get(1).filter(|s| !s.is_empty()) {
                    let lc = lc_no(&mut m, case);
                    if let Some(last) = m.combination_defs.last_mut() {
                        last.1.push((
                            lc,
                            c.get(2).and_then(|s| num(s)).unwrap_or(0.0),
                            c.get(3).and_then(|s| num(s)).unwrap_or(0.0),
                        ));
                    }
                }
            }
            Table::Materials => {
                // `| 1|Steel:350W | 200.0E6|0.3|77.0|11.7E-6|...`: E in kPa, density kN/m^3.
                let (Some(_), Some(name)) = (int(c[0]), c.get(1).filter(|s| !s.is_empty())) else {
                    continue;
                };
                let g = |i: usize| c.get(i).and_then(|s| num(s)).unwrap_or(0.0);
                m.materials.push(Material {
                    name: (*name).to_owned(),
                    e: g(2) * f / (l * l),
                    poisson: g(3),
                    density: g(4) * f / (l * l * l),
                    alpha: g(5),
                    ..Default::default()
                });
            }
            Table::Skipped(what) => {
                if !c[0].is_empty() {
                    *m.skipped.entry(what.into()).or_default() += 1;
                }
            }
            Table::None => {}
        }
    }
    if let Some(name) = self_weight.filter(|s| !s.is_empty()) {
        let no = lc_no(&mut m, &name);
        // Prokon is Y up: gravity along -Y.
        m.self_weight.push((no, [0.0, -1.0, 0.0]));
    }
    if m.joints.is_empty() {
        return Err("no nodes found in the Prokon report".into());
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_report() {
        let text = "Frame Analysis - Ver W3.0.06\n Units: Distance:m     Force:kN\n\
 ------ ----\n|Node  ¦X    ¦Y    ¦Z    ¦No.¦Node¦X-inc or¦Y-inc or¦Z-inc or¦Use end (Y)¦\n\
|Number|(m)  |(m)  |(m)  |of |Inc |X-end   |Y-end   |Z-end   |or inc [N] |\n ------\n\
|     1|    0|    6|    0|  2|   1|     0.9|        |        |           |\n\
|    10|    0|    6|  1.8|  2|   1|     0.9|        |        |           |\n\
 ------\n\n\
|Node¦Node¦Node¦Node¦Number  ¦Node No¦t    ¦Material       |Rigid|GN |Align local Y to:¦\n\
|1   |2   |3   |4   |of extra|inc.   |(m)  |               |     |   |                 |\n\
 ----\n|1   |   2|  11|  10|       1|      1|  0.5|Concrete:40 MPa|     |   |  |\n ----\n\n\
|Node                ¦Fixity¦[P]¦X  |Y  |Z  |x  |y  |z  |Number  ¦No\n\
|                  10|Zxyz  |   |   |   |   |   |   |   |       1|      |\n ----\n\n\
 Self weight to be added to:DL\n";
        let m = parse(text).unwrap();
        assert_eq!(m.joints.len(), 6);
        assert_eq!(m.joints[&3], [1.8, 6.0, 0.0]);
        assert_eq!(m.plates.len(), 2);
        assert_eq!(m.plates[&(SHELL_BASE + 2)], vec![2, 3, 12, 11]);
        assert_eq!(m.plate_thickness[&(SHELL_BASE + 1)], 0.5);
        assert_eq!(m.supports[&11], [false, false, true, true, true, true]);
        assert_eq!(m.supports.len(), 2);
        assert_eq!(m.self_weight, vec![(1, [0.0, -1.0, 0.0])]);
    }
}
