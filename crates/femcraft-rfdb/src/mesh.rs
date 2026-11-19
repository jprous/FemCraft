//! FE mesh RFEM stores under `!solver/M/` once a model has been meshed or calculated.
//!
//! Layout (confirmed on RFEM 6.16, solver 6.10, a 5-surface quad mesh; little-endian, no
//! headers, all numbers 1-based):
//! - `Data.XYZ`: per FE node 3 x f64 (model x, y, z in m).
//! - `Data.E2D`: per 2D element 4 x i32 FE node numbers. Triangles not seen yet (guess: 4th
//!   node 0 or equal to the 3rd).
//! - `Data.GET`: per 2D element i32, the RFEM surface number.
//! - `Data.LIN` (text): `LI <line no> <fe nodes..>` with continuation lines, the FE node chain
//!   along each model line.
//! - `Data.MTO`, `Data.GEO`, `Data.DIV` (text): element ranges per surface, solver copy of the
//!   geometry, mesh parameters. `Data.E2DL`, `Data.GBS`, `Data.FBI`, `TR/Data.ETRI` (element
//!   axes, 9 x f64) not used here.
//!
//! The mesh belongs to the geometry it was made from: [`crate::RfDocument::save`] drops
//! `!solver/` once the model has been edited.

use crate::{Archive, Error};

pub const XYZ: &str = "!solver/M/Data.XYZ";
pub const E2D: &str = "!solver/M/Data.E2D";
pub const GET: &str = "!solver/M/Data.GET";
pub const MTO: &str = "!solver/M/Data.MTO";
pub const LIN: &str = "!solver/M/Data.LIN";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FeMesh {
    /// FE node coordinates; FE node `n` is `nodes[n - 1]`.
    pub nodes: Vec<[f64; 3]>,
    /// 2D elements: FE node numbers, `d == c` (or 0) for a triangle.
    pub elements: Vec<[u32; 4]>,
    /// RFEM surface number per element (same length as `elements`, 0 when unknown).
    pub element_surface: Vec<u32>,
    /// FE node chain along each model line: (line no, FE node numbers).
    pub lines: Vec<(u32, Vec<u32>)>,
}

impl FeMesh {
    /// Reads the mesh from an opened archive; `Ok(None)` when the file holds no mesh.
    pub fn read(archive: &Archive) -> Result<Option<Self>, Error> {
        if !archive.entries.iter().any(|e| e == XYZ) {
            return Ok(None);
        }
        let opt = |name: &str| -> Result<Vec<u8>, Error> {
            if archive.entries.iter().any(|e| e == name) {
                archive.read_entry(name)
            } else {
                Ok(Vec::new())
            }
        };
        let mut mesh = Self::parse(
            &archive.read_entry(XYZ)?,
            &opt(E2D)?,
            &opt(GET)?,
            &opt(LIN)?,
        )?;
        mesh.apply_mto(&opt(MTO)?);
        Ok(Some(mesh))
    }

    /// Sets `element_surface` from `Data.MTO`, the mesh's object table. Its text rows
    /// `<surface> 0 0 <first element> <last element> G ...` give each surface's contiguous element
    /// range; `Data.GET` is not the surface number (on a slab it counts 1, 2, 3, ... per element).
    pub fn apply_mto(&mut self, mto: &[u8]) {
        let text = String::from_utf8_lossy(mto);
        let mut ranges = Vec::new();
        for row in text.lines() {
            let w: Vec<&str> = row.split_whitespace().collect();
            if w.len() < 6 || w[5] != "G" {
                continue;
            }
            let num = |i: usize| w[i].parse::<u32>().ok();
            if let (Some(surface), Some(first), Some(last)) = (num(0), num(3), num(4)) {
                ranges.push((surface, first, last));
            }
        }
        if ranges.is_empty() {
            return;
        }
        self.element_surface = vec![0; self.elements.len()];
        for (surface, first, last) in ranges {
            for el in first.max(1)..=last.min(self.elements.len() as u32) {
                self.element_surface[el as usize - 1] = surface;
            }
        }
    }

    pub fn parse(xyz: &[u8], e2d: &[u8], get: &[u8], lin: &[u8]) -> Result<Self, Error> {
        if xyz.len() % 24 != 0 || e2d.len() % 16 != 0 || get.len() % 4 != 0 {
            return Err(Error::Format("unexpected FE mesh file size".into()));
        }
        let f64_at = |b: &[u8], i: usize| f64::from_le_bytes(b[i..i + 8].try_into().unwrap());
        let i32_at = |b: &[u8], i: usize| i32::from_le_bytes(b[i..i + 4].try_into().unwrap());
        let nodes: Vec<[f64; 3]> = xyz
            .chunks_exact(24)
            .map(|c| [f64_at(c, 0), f64_at(c, 8), f64_at(c, 16)])
            .collect();
        let n = nodes.len() as i64;
        let mut elements = Vec::with_capacity(e2d.len() / 16);
        for c in e2d.chunks_exact(16) {
            let mut e = [0u32; 4];
            for (k, v) in e.iter_mut().enumerate() {
                let x = i32_at(c, 4 * k) as i64;
                if x < 0 || x > n || (x == 0 && k < 3) {
                    return Err(Error::Format(format!("FE element node {x} out of range")));
                }
                *v = x as u32;
            }
            elements.push(e);
        }
        let mut element_surface: Vec<u32> = get
            .chunks_exact(4)
            .map(|c| i32_at(c, 0).max(0) as u32)
            .collect();
        element_surface.resize(elements.len(), 0);
        Ok(Self {
            nodes,
            elements,
            element_surface,
            lines: parse_lin(lin),
        })
    }

    /// Element edges as FE node pairs (each shared edge once), for drawing.
    pub fn edges(&self) -> Vec<(u32, u32)> {
        let mut set = std::collections::BTreeSet::new();
        for e in &self.elements {
            let k = if e[3] == 0 || e[3] == e[2] { 3 } else { 4 };
            for i in 0..k {
                let (a, b) = (e[i], e[(i + 1) % k]);
                set.insert((a.min(b), a.max(b)));
            }
        }
        set.into_iter().collect()
    }

    pub fn node(&self, no: u32) -> Option<[f64; 3]> {
        self.nodes.get((no as usize).checked_sub(1)?).copied()
    }
}

/// `LI <line> <nodes..>` records; continuation lines carry only numbers.
fn parse_lin(bytes: &[u8]) -> Vec<(u32, Vec<u32>)> {
    let text = String::from_utf8_lossy(bytes);
    let mut out: Vec<(u32, Vec<u32>)> = Vec::new();
    // False after a record of another kind (`PO <line> <position> <node>` rows follow the `LI`
    // records on beam meshes): its numbers are not part of the node chain.
    let mut in_chain = false;
    for row in text.lines() {
        let mut words = row.split_whitespace().peekable();
        match words.peek() {
            Some(&"LI") => {
                words.next();
                let Some(no) = words.next().and_then(|w| w.parse().ok()) else {
                    in_chain = false;
                    continue;
                };
                out.push((no, Vec::new()));
                in_chain = true;
            }
            Some(w) if w.parse::<u32>().is_err() => {
                in_chain = false;
                continue;
            }
            _ => {}
        }
        if !in_chain {
            continue;
        }
        if let Some((_, nodes)) = out.last_mut() {
            nodes.extend(words.filter_map(|w| w.parse::<u32>().ok()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mto_gives_surface_element_ranges() {
        let mut mesh = FeMesh {
            nodes: vec![],
            elements: vec![[1, 2, 3, 4]; 5],
            element_surface: vec![9; 5],
            lines: vec![],
        };
        let mto = b"         1         0         0         1         2         G         1
         2         0         0         3         5         G         0
       511         0       532        14         0         S       532
";
        mesh.apply_mto(mto);
        assert_eq!(mesh.element_surface, vec![1, 1, 2, 2, 2]);
    }

    #[test]
    fn parses_quad_and_lines() {
        let mut xyz = Vec::new();
        for p in [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0f64],
        ] {
            for v in p {
                xyz.extend(v.to_le_bytes());
            }
        }
        let e2d: Vec<u8> = [1i32, 2, 3, 4]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let get = 7i32.to_le_bytes();
        let lin = b"LI       1         1         2\r\n                   3\r\n";
        let m = FeMesh::parse(&xyz, &e2d, &get, lin).unwrap();
        assert_eq!(m.nodes.len(), 4);
        assert_eq!(m.element_surface, vec![7]);
        assert_eq!(m.lines, vec![(1, vec![1, 2, 3])]);
        // `PO` records after the chains are not nodes of the last line.
        let lin = b"LI       1         1         2\nPO       1   0.50000         3\n";
        let m = FeMesh::parse(&xyz, &e2d, &get, lin).unwrap();
        assert_eq!(m.lines, vec![(1, vec![1, 2])]);
        assert_eq!(m.edges().len(), 4);
    }
}
