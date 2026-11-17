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
        Self::parse(
            &archive.read_entry(XYZ)?,
            &opt(E2D)?,
            &opt(GET)?,
            &opt(LIN)?,
        )
        .map(Some)
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
    for row in text.lines() {
        let mut words = row.split_whitespace().peekable();
        if words.peek() == Some(&"LI") {
            words.next();
            let Some(no) = words.next().and_then(|w| w.parse().ok()) else {
                continue;
            };
            out.push((no, Vec::new()));
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
        assert_eq!(m.edges().len(), 4);
    }
}
