//! Robot Structural Analysis text files (`.str`, "ROBOT97" format; Tekla Structural Designer
//! and others export it). Parsed into the same [`StaadModel`] the STAAD and Prokon readers fill.
//!
//! Read: units, nodes, supports (no label = fixed, labels = free degrees of freedom), bars,
//! triangular / quadrangular plates, material + section / thickness groups (`PROperties`), load
//! cases with nodal forces / moments and uniform global pressures (`PX/PY/PZ`) and bar loads,
//! combinations. Z is up. Numbers use a decimal comma when the exporting PC did. Rigid links,
//! offsets, spring supports and varying element loads are only counted in
//! [`StaadModel::skipped`].


use crate::staad::{
    ElementLoad, JointLoad, Material, MemberLoad, Pressure, PressureDir, Prismatic, StaadModel,
};

fn num(t: &str) -> Option<f64> {
    t.parse().ok().or_else(|| t.replace(',', ".").parse().ok())
}

/// `key=value` pairs of a record (`E=3,3E+10 NAMe="Concrete C30"`), keys upper-cased.
fn pairs(rec: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let b: Vec<char> = rec.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if let Some(eq) = b[i..].iter().position(|c| *c == '=') {
            let eq = i + eq;
            let ks = b[i..eq]
                .iter()
                .rposition(|c| c.is_whitespace())
                .map_or(i, |p| i + p + 1);
            let key: String = b[ks..eq].iter().collect::<String>().to_ascii_uppercase();
            let mut j = eq + 1;
            let v: String;
            if b.get(j) == Some(&'"') {
                j += 1;
                let end = b[j..].iter().position(|c| *c == '"').map_or(b.len(), |p| j + p);
                v = b[j..end].iter().collect();
                i = (end + 1).min(b.len());
            } else {
                let end = b[j..]
                    .iter()
                    .position(|c| c.is_whitespace())
                    .map_or(b.len(), |p| j + p);
                v = b[j..end].iter().collect();
                i = end;
            }
            out.push((key, v));
            if i >= b.len() {
                break;
            }
        } else {
            break;
        }
    }
    out
}

/// Numbers and `a TO b [BY s]` ranges at the start of `rec`, and the rest (from the first
/// token that is not part of the list).
fn id_list(rec: &str) -> (Vec<u32>, &str) {
    let mut ids = Vec::new();
    let mut rest = rec.trim_start();
    let mut last: Option<u32> = None;
    loop {
        let (tok, after) = match rest.find(char::is_whitespace) {
            Some(p) => (&rest[..p], rest[p..].trim_start()),
            None => (rest, ""),
        };
        if tok.is_empty() {
            break;
        }
        if let Ok(n) = tok.parse::<u32>() {
            ids.push(n);
            last = Some(n);
        } else if tok.eq_ignore_ascii_case("TO") {
            // `a TO b [BY s]`
            let (end_tok, after2) = match after.find(char::is_whitespace) {
                Some(p) => (&after[..p], after[p..].trim_start()),
                None => (after, ""),
            };
            let (Some(a), Ok(b)) = (last, end_tok.parse::<u32>()) else {
                break;
            };
            let mut step = 1;
            let mut after3 = after2;
            if let Some(r) = after2.strip_prefix_ci("BY") {
                let r = r.trim_start();
                let (s, a3) = match r.find(char::is_whitespace) {
                    Some(p) => (&r[..p], r[p..].trim_start()),
                    None => (r, ""),
                };
                if let Ok(s) = s.parse::<u32>() {
                    step = s.max(1);
                    after3 = a3;
                }
            }
            let mut n = a + step;
            while n <= b {
                ids.push(n);
                n += step;
            }
            last = Some(b);
            rest = after3;
            continue;
        } else {
            break;
        }
        rest = after;
    }
    (ids, rest)
}

trait StripCi {
    fn strip_prefix_ci(&self, p: &str) -> Option<&str>;
}
impl StripCi for str {
    fn strip_prefix_ci(&self, p: &str) -> Option<&str> {
        let h = self.get(..p.len())?;
        (h.eq_ignore_ascii_case(p) && self[p.len()..].starts_with(|c: char| c.is_whitespace()))
            .then(|| &self[p.len()..])
    }
}

/// Length / force factors to SI from `LENgth=m FORces=N`.
fn unit_factors(rec: &str) -> (Option<f64>, Option<f64>) {
    let (mut l, mut f) = (None, None);
    for (k, v) in pairs(rec) {
        let v = v.to_ascii_lowercase();
        if k.starts_with("LEN") {
            l = match v.as_str() {
                "m" => Some(1.0),
                "cm" => Some(1e-2),
                "mm" => Some(1e-3),
                "km" => Some(1e3),
                _ => None,
            };
        } else if k.starts_with("FOR") {
            f = match v.as_str() {
                "n" => Some(1.0),
                "kn" => Some(1e3),
                "mn" => Some(1e6),
                "dan" => Some(10.0),
                _ => None,
            };
        }
    }
    (l, f)
}

/// `21to24` -> `21 TO 24` (Robot writes ranges without spaces).
fn spaced_ranges(l: &str) -> String {
    let c: Vec<char> = l.chars().collect();
    let mut out = String::with_capacity(l.len() + 8);
    let mut i = 0;
    while i < c.len() {
        if i > 0
            && i + 2 < c.len()
            && c[i - 1].is_ascii_digit()
            && (c[i] == 't' || c[i] == 'T')
            && (c[i + 1] == 'o' || c[i + 1] == 'O')
            && c[i + 2].is_ascii_digit()
        {
            out.push_str(" TO ");
            i += 2;
            continue;
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

#[derive(Clone, Copy, PartialEq)]
enum Sec {
    Top,
    Nodes,
    Supports,
    Bars,
    Plates,
    Properties,
    RigidLinks,
    Offsets,
    Loads,
    LoadNodes,
    LoadElems,
    Combination,
    LoadSelf,
    Skip,
}

/// Parses a Robot `.str` text file.
pub fn parse(text: &str) -> Result<StaadModel, String> {
    let mut m = StaadModel {
        z_up: true,
        ..StaadModel::default()
    };
    // Logical lines: drop comments, join `_` continuations.
    let mut lines: Vec<String> = Vec::new();
    let mut carry = String::new();
    for raw in text.lines() {
        let l = raw.trim();
        if l.starts_with('\'') || l.starts_with(';') || l.is_empty() {
            continue;
        }
        if let Some(h) = l.strip_suffix('_') {
            carry.push_str(h);
            carry.push(' ');
            continue;
        }
        carry.push_str(l);
        lines.push(spaced_ranges(&std::mem::take(&mut carry)));
    }
    if !lines.iter().any(|l| l.to_ascii_uppercase().starts_with("ROBOT97")) {
        return Err("not a Robot text file (no ROBOT97 line)".into());
    }
    // Tekla's export says `Label - free`; Robot's own files list the restrained degrees.
    let labels_free = text.contains("Label        - free");
    let mut pending_name = String::new();
    let (mut ulen, mut uforce) = (1.0f64, 1.0f64);
    let mut sec = Sec::Top;
    let mut seen_nodes_header = false;
    let mut cur_lc = 0u32;
    let mut cur_mat: Option<String> = None;
    let mut mat_no = 0;
    let skip = |m: &mut StaadModel, what: &str| *m.skipped.entry(what.to_owned()).or_default() += 1;
    for l in &lines {
        let up = l.to_ascii_uppercase();
        let first = up.split_whitespace().next().unwrap_or("");
        let numeric = first.starts_with(|c: char| c.is_ascii_digit());
        if sec == Sec::Skip {
            if up.starts_with("BOU") && up.contains("END") {
                sec = Sec::Top;
            }
            continue;
        }
        if up.starts_with("BOU") {
            sec = Sec::Skip;
            continue;
        }
        if up.starts_with("SEL") {
            sec = Sec::LoadSelf;
            continue;
        }
        if sec == Sec::Properties && l.starts_with('"') {
            pending_name = l.trim_matches('"').to_owned();
            continue;
        }
        // Section keywords (3-letter prefix is significant in Robot's language).
        if !numeric {
            let kw = |k: &str| up.starts_with(k);
            if kw("UNI") {
                continue;
            } else if kw("LEN") || kw("FOR") {
                let (a, b) = unit_factors(l);
                ulen = a.unwrap_or(ulen);
                uforce = b.unwrap_or(uforce);
                continue;
            } else if kw("NOD") {
                if sec == Sec::Loads || sec == Sec::LoadNodes || sec == Sec::LoadElems {
                    sec = Sec::LoadNodes;
                } else if !seen_nodes_header {
                    // `NODes 1333  ELEments 2478` (counts) then the NODes table.
                    seen_nodes_header = true;
                } else {
                    sec = Sec::Nodes;
                }
                continue;
            } else if kw("SUP") {
                sec = Sec::Supports;
                continue;
            } else if kw("ELE") {
                if matches!(sec, Sec::Loads | Sec::LoadNodes | Sec::LoadElems) {
                    sec = Sec::LoadElems;
                }
                continue;
            } else if kw("BAR") {
                sec = Sec::Bars;
                continue;
            } else if kw("TRI") || kw("QUA") {
                sec = Sec::Plates;
                continue;
            } else if kw("PRO") {
                sec = Sec::Properties;
                continue;
            } else if kw("RIG") {
                sec = Sec::RigidLinks;
                continue;
            } else if kw("OFF") {
                sec = Sec::Offsets;
                continue;
            } else if kw("LOA") {
                sec = Sec::Loads;
                continue;
            } else if kw("ANA") {
                continue;
            } else if kw("CAS") {
                // `CASe #3 3 Dead`
                let mut toks: Vec<&str> = l
                    .split_whitespace()
                    .skip(1)
                    .filter(|t| *t != "#")
                    .collect();
                let no = toks
                    .first()
                    .and_then(|t| t.trim_start_matches('#').parse::<u32>().ok())
                    .unwrap_or(cur_lc + 1);
                if !toks.is_empty() {
                    toks.remove(0);
                }
                // `#3 3 Dead` carries a nature number before the name, `# 5 DL5` does not.
                if toks.len() > 1 && toks[0].chars().all(|c| c.is_ascii_digit()) {
                    toks.remove(0);
                }
                let name = toks.join(" ");
                cur_lc = no;
                m.load_cases
                    .insert(no, if name.is_empty() { format!("LC{no}") } else { name });
                sec = Sec::Loads;
                continue;
            } else if kw("COM") {
                m.combinations += 1;
                let mut it = l.split_whitespace().skip(1);
                let _no = it.next();
                let _idx = it.next();
                let name = it.collect::<Vec<_>>().join(" ");
                m.combination_defs.push((name, Vec::new()));
                sec = Sec::Combination;
                continue;
            } else if kw("END") {
                break;
            } else if matches!(sec, Sec::Top) {
                continue; // header lines (SHEll, NUMbering ...)
            }
        }
        match sec {
            Sec::Nodes => {
                let t: Vec<&str> = l.split_whitespace().collect();
                if t.len() >= 3 {
                    if let (Ok(n), Some(x), Some(y)) =
                        (t[0].parse::<u32>(), num(t[1]), num(t[2]))
                    {
                        // `PLAte` files give X and Y only.
                        let z = t.get(3).and_then(|v| num(v)).unwrap_or(0.0);
                        m.joints.insert(n, [x * ulen, y * ulen, z * ulen]);
                    }
                }
            }
            Sec::Supports => {
                let (ids, rest) = id_list(l);
                let label = rest.trim().to_ascii_uppercase();
                if label.contains('=') {
                    skip(&mut m, "spring supports");
                    continue;
                }
                let named: Vec<usize> = label
                    .split_whitespace()
                    .filter_map(|t| match t {
                        "UX" => Some(0),
                        "UY" => Some(1),
                        "UZ" => Some(2),
                        "RX" => Some(3),
                        "RY" => Some(4),
                        "RZ" => Some(5),
                        _ => None,
                    })
                    .collect();
                let mut fixed = if label.is_empty() {
                    [true; 6]
                } else if label.starts_with("PIN") {
                    [true, true, true, false, false, false]
                } else if label.starts_with("FIX") {
                    [true; 6]
                } else if labels_free {
                    [true; 6]
                } else {
                    [false; 6]
                };
                for i in named {
                    fixed[i] = !labels_free;
                }
                for n in ids {
                    m.supports.insert(n, fixed);
                }
            }
            Sec::Bars => {
                let t: Vec<u32> = l
                    .split_whitespace()
                    .map_while(|t| t.parse().ok())
                    .collect();
                if t.len() >= 3 {
                    m.members.insert(t[0], (t[1], t[2]));
                }
            }
            Sec::Plates => {
                let t: Vec<u32> = l
                    .split_whitespace()
                    .map_while(|t| t.parse().ok())
                    .collect();
                if t.len() >= 4 {
                    m.plates.insert(t[0], t[1..].to_vec());
                }
            }
            Sec::Properties => {
                let kv = pairs(l);
                let get = |k: &str| {
                    kv.iter()
                        .find(|(a, _)| a == k)
                        .and_then(|(_, v)| num(v))
                };
                let name = kv
                    .iter()
                    .find(|(a, _)| a == "NAME")
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default();
                if numeric {
                    let (ids, _) = id_list(l);
                    if let Some(th) = get("TH") {
                        for id in &ids {
                            m.plate_thickness.insert(*id, th * ulen);
                        }
                    } else if get("AX").is_some() {
                        let mut p = Prismatic {
                            ax: get("AX").unwrap_or(0.0) * ulen * ulen,
                            ix: get("IX").unwrap_or(0.0) * ulen.powi(4),
                            iy: get("IY").unwrap_or(0.0) * ulen.powi(4),
                            iz: get("IZ").unwrap_or(0.0) * ulen.powi(4),
                            ..Prismatic::default()
                        };
                        // Names like `400x800` (mm, width x depth) give the section size.
                        if let Some((b, h)) = name.split_once(['x', 'X']) {
                            if let (Ok(b), Ok(h)) = (b.trim().parse::<f64>(), h.trim().parse::<f64>())
                            {
                                if b > 0.0 && h > 0.0 {
                                    p.zd = b / 1e3;
                                    p.yd = h / 1e3;
                                }
                            }
                        }
                        for id in &ids {
                            m.member_props.insert(*id, p);
                        }
                    } else {
                        skip(&mut m, "property groups (other)");
                        continue;
                    }
                    if let Some(mn) = &cur_mat {
                        for id in ids {
                            m.element_material.insert(id, mn.clone());
                        }
                    }
                } else if get("E").is_some() {
                    mat_no += 1;
                    // Stresses and weight densities are force / length^n.
                    let sf = uforce / (ulen * ulen);
                    let e = get("E").unwrap_or(0.0) * sf;
                    let g = get("G").map(|g| g * sf);
                    let nu = get("NI")
                        .or_else(|| g.filter(|g| *g > 0.0).map(|g| e / (2.0 * g) - 1.0))
                        .unwrap_or(0.2);
                    let name = if !name.is_empty() {
                        name
                    } else if !pending_name.is_empty() {
                        std::mem::take(&mut pending_name)
                    } else {
                        format!("Material {mat_no}")
                    };
                    m.materials.push(Material {
                        name: name.clone(),
                        e,
                        g: g.unwrap_or(e / (2.0 * (1.0 + nu))),
                        poisson: nu,
                        density: get("RO").unwrap_or(0.0) * uforce / (ulen * ulen * ulen),
                        alpha: get("LX").unwrap_or(1.2e-5),
                    });
                    cur_mat = Some(name);
                }
            }
            Sec::RigidLinks => skip(&mut m, "rigid links"),
            Sec::Offsets => skip(&mut m, "member offsets"),
            Sec::LoadNodes => {
                let (ids, rest) = id_list(l);
                let kv = pairs(rest);
                if ids.is_empty() || kv.is_empty() {
                    continue;
                }
                let (mut f, mut mo) = ([0.0; 3], [0.0; 3]);
                let mut other = false;
                for (k, v) in &kv {
                    let Some(v) = num(v) else { continue };
                    match k.as_str() {
                        "FX" => f[0] = v * uforce,
                        "FY" => f[1] = v * uforce,
                        "FZ" => f[2] = v * uforce,
                        "CX" => mo[0] = v * uforce * ulen,
                        "CY" => mo[1] = v * uforce * ulen,
                        "CZ" => mo[2] = v * uforce * ulen,
                        _ => other = true,
                    }
                }
                if other {
                    skip(&mut m, "nodal load (other)");
                }
                m.joint_loads.push(JointLoad {
                    lc: cur_lc,
                    joints: ids,
                    force: f,
                    moment: mo,
                });
            }
            Sec::LoadElems => {
                let (ids, rest) = id_list(l);
                let kv = pairs(rest);
                if ids.is_empty() || kv.is_empty() {
                    continue;
                }
                // Uniform global pressure / line load only: exactly one P? component, no
                // `TILl` (varying) part, no local axes.
                let up_rest = rest.to_ascii_uppercase();
                let varying = up_rest.contains("TIL") || kv.iter().any(|(k, _)| k == "X");
                let local = up_rest.contains("LOC");
                let comp: Vec<(usize, f64)> = kv
                    .iter()
                    .filter_map(|(k, v)| {
                        let a = match k.as_str() {
                            "PX" => 0,
                            "PY" => 1,
                            "PZ" => 2,
                            _ => return None,
                        };
                        num(v).map(|v| (a, v))
                    })
                    .collect();
                if varying || local || comp.is_empty() {
                    skip(
                        &mut m,
                        "element loads (varying / local / other)",
                    );
                    continue;
                }
                let (bars, plates): (Vec<u32>, Vec<u32>) =
                    ids.into_iter().partition(|i| m.members.contains_key(i));
                for (a, v) in comp {
                    for b in &bars {
                        m.member_loads.push(MemberLoad {
                            lc: cur_lc,
                            member: *b,
                            dir: PressureDir::Global(a),
                            w: v * uforce / ulen,
                        });
                    }
                    if !plates.is_empty() {
                        m.element_loads.push(ElementLoad {
                            lc: cur_lc,
                            plates: plates.clone(),
                            dir: PressureDir::Global(a),
                            value: Pressure::Uniform(v * uforce / (ulen * ulen)),
                        });
                    }
                }
            }
            Sec::Combination => {
                // `case factor [case factor ...]`
                let t: Vec<&str> = l.split_whitespace().collect();
                for pair in t.chunks_exact(2) {
                    if let (Ok(lc), Some(f)) = (pair[0].parse::<u32>(), num(pair[1])) {
                        if let Some(c) = m.combination_defs.last_mut() {
                            c.1.push((lc, f, f));
                        }
                    }
                }
            }
            Sec::LoadSelf => {
                // `11to18942 PZ MINus`: self-weight along -Z (Robot's Z is up).
                let up = l.to_ascii_uppercase();
                if up.contains("PZ") && up.contains("MIN") {
                    if !m.self_weight.iter().any(|(lc, _)| *lc == cur_lc) {
                        m.self_weight.push((cur_lc, [0.0, 0.0, -1.0]));
                    }
                } else {
                    skip(&mut m, "self-weight (other direction)");
                }
            }
            Sec::Skip => {}
            Sec::Loads | Sec::Top => {}
        }
    }
    if m.joints.is_empty() {
        return Err("no nodes found in the Robot file".into());
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "ROBOT97\nSHEll\nNODes 3  ELEments 3\nUNIts\nLENgth=m    FORces=N\nUNIts END\n\
NODes\n' x\n 1 0,0E+0 0,0E+0 0,0E+0\n 2 5,0E+0 0,0E+0 0,0E+0\n 3 5,0E+0 5,0E+0 0,0E+0\n 4 0,0E+0 5,0E+0 0,0E+0\n 5 0 0 3\n\
SUPports\n1\n2 RY RX\nELEments\nBARs 1\n' h\n 1 1 5\nQUAdrangular 4\n 2 1 2 3 4\nPROperties\n\
E=3,3E+10 NI=2,0E-1 RO=0,0E+0 LX=1,0E-5 G=1,375E+10 NAMe=\"C30\"\n1_\nAx=3,2E-1 Iy=3,4E-3 Iz=8,5E-4 Ix=1E-4 NAMe=\"400x800\"\n2 TO 2_\nTH=2,5E-1\n\
LOAds\nANAlysis LINear\nCASe #1 1 Dead\nNODes\n5 FZ=-3,0E+3 CX=1,0E+1\nELEments\n2 PZ=-5,0E+3\n1 PZ=-1,0E+2\n\
CASe #2 2 Wind\nELEments\n2 X=0 PZ=1 TILl X=1 PZ=2 RELative\n\
COMbination #1 1 ULS\n 1 1,35E+0\n 2 1,5E+0\nEND\n";

    #[test]
    fn reads_small_model() {
        let m = parse(SAMPLE).unwrap();
        assert_eq!(m.joints.len(), 5);
        assert_eq!(m.joints[&2], [5.0, 0.0, 0.0]);
        assert_eq!(m.supports[&1], [true; 6]);
        assert_eq!(m.supports[&2], [true, true, true, false, false, true]);
        assert_eq!(m.members[&1], (1, 5));
        assert_eq!(m.plates[&2], vec![1, 2, 3, 4]);
        assert_eq!(m.materials.len(), 1);
        assert_eq!(m.materials[0].name, "C30");
        let p = m.member_props[&1];
        assert!((p.zd - 0.4).abs() < 1e-12 && (p.yd - 0.8).abs() < 1e-12);
        assert_eq!(m.plate_thickness[&2], 0.25);
        assert_eq!(m.element_material[&2], "C30");
        assert_eq!(m.load_cases[&1], "Dead");
        assert_eq!(m.joint_loads[0].force, [0.0, 0.0, -3000.0]);
        assert_eq!(m.joint_loads[0].moment, [10.0, 0.0, 0.0]);
        assert_eq!(m.element_loads.len(), 1);
        assert_eq!(m.member_loads.len(), 1);
        assert_eq!(m.combination_defs[0].1, vec![(1, 1.35, 1.35), (2, 1.5, 1.5)]);
        assert_eq!(m.skipped.values().sum::<usize>(), 1);
    }

    #[test]
    fn rejects_other_text() {
        assert!(parse("STAAD SPACE\n").is_err());
    }
}
