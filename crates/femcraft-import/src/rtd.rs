//! Robot Structural Analysis binary files (`.rtd`, an OLE compound file of record streams).
//!
//! Only part of the format is decoded (found by inspecting "New Sludge Tank Rev 0.rtd",
//! Robot 2023 / file version 36):
//!
//! - `RV6STR/OFDAT/OBCOR`: header of 8 i32 (`[2]` = records), then per object 12 f64 = the
//!   first 4 corner points (x, y, z; Z up) of a panel. Panels with more corners or curved
//!   contours (the base slab) are stored elsewhere and read as all zero or partial here.
//! - `RV6STR/OFDAT/ObPrtDtREC`: per object (part number, 1) i32 pairs; 0 = not a panel.
//! - `RV6STR.003/SSDAT` + `SSNAM`: panel thickness table (f64, 48-byte names).
//!
//! Not decoded: which panel has which thickness, nodes / FE mesh (`ONDAT`, `OEDAT`, bit packed),
//! supports and loads. Every panel gets [`DEFAULT_THICKNESS`] and a note says so.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};

use crate::staad::{Material, StaadModel};

/// Compound-file signature (also used by `.rtd`).
pub const MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// Thickness given to every panel until the file's panel -> thickness link is decoded (m).
pub const DEFAULT_THICKNESS: f64 = 0.4;

type Cf<'a> = cfb::CompoundFile<Cursor<&'a [u8]>>;

fn stream(cf: &mut Cf, path: &str) -> Result<Vec<u8>, String> {
    let mut s = cf
        .open_stream(path)
        .map_err(|e| format!("stream {path}: {e}"))?;
    let mut v = Vec::new();
    s.read_to_end(&mut v).map_err(|e| e.to_string())?;
    Ok(v)
}

fn i32s(b: &[u8]) -> Vec<i32> {
    b.chunks_exact(4)
        .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn f64s(b: &[u8]) -> Vec<f64> {
    b.chunks_exact(8)
        .map(|c| f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]))
        .collect()
}

/// Header record count and the payload after the 32-byte header.
fn payload(b: &[u8]) -> Result<(usize, &[u8]), String> {
    if b.len() < 32 {
        return Err("stream too short".into());
    }
    let n = i32::from_le_bytes([b[8], b[9], b[10], b[11]]);
    Ok((n.max(0) as usize, &b[32..]))
}

pub fn parse(bytes: &[u8]) -> Result<StaadModel, String> {
    if bytes.len() < 8 || bytes[..8] != MAGIC {
        return Err("not a Robot .rtd file (no compound-file signature)".into());
    }
    let mut cf = cfb::CompoundFile::open(Cursor::new(bytes))
        .map_err(|e| format!("not a readable compound file: {e}"))?;
    let cor = stream(&mut cf, "/RV6STR/OFDAT/OBCOR").map_err(|_| {
        "no panel geometry (RV6STR/OFDAT/OBCOR): only Robot shell models are supported".to_string()
    })?;
    let part = stream(&mut cf, "/RV6STR/OFDAT/ObPrtDtREC").unwrap_or_default();
    let mut m = StaadModel {
        z_up: true,
        ..StaadModel::default()
    };

    let (n, pl) = payload(&cor)?;
    let usable = (n * 96).min(pl.len() / 96 * 96);
    let coords = f64s(&pl[..usable]);
    let parts: Vec<i32> = payload(&part)
        .map(|(_, p)| i32s(p).chunks_exact(2).map(|c| c[0]).collect())
        .unwrap_or_default();

    let mut key_to_joint: BTreeMap<[i64; 3], u32> = BTreeMap::new();
    let (mut zero, mut degenerate, mut stray) = (0usize, 0usize, 0usize);
    for rec in 0..coords.len() / 12 {
        let c = &coords[rec * 12..rec * 12 + 12];
        let is_panel = parts.get(rec).copied().unwrap_or(0) != 0;
        if c.iter().all(|v| *v == 0.0) {
            if is_panel {
                zero += 1; // a panel whose contour is stored elsewhere (base slab)
            }
            continue;
        }
        if !is_panel {
            stray += 1; // data of a deleted / unnumbered object
            continue;
        }
        let mut ids: Vec<u32> = Vec::new();
        for k in 0..4 {
            let p = [c[k * 3] + 0.0, c[k * 3 + 1] + 0.0, c[k * 3 + 2] + 0.0];
            let key = [
                (p[0] * 1e4).round() as i64,
                (p[1] * 1e4).round() as i64,
                (p[2] * 1e4).round() as i64,
            ];
            let next = key_to_joint.len() as u32 + 1;
            let id = *key_to_joint.entry(key).or_insert(next);
            m.joints.entry(id).or_insert(p);
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        if ids.len() < 3 {
            degenerate += 1;
            continue;
        }
        m.plates.insert(rec as u32, ids);
    }
    if m.plates.is_empty() {
        return Err("no panels with readable corner points were found".into());
    }

    // Thickness table, for the note only.
    let mut table = Vec::new();
    if let (Ok(dat), Ok(nam)) = (
        stream(&mut cf, "/RV6STR.003/SSDAT"),
        stream(&mut cf, "/RV6STR.003/SSNAM"),
    ) {
        if let (Ok((_, d)), Ok((_, nm))) = (payload(&dat), payload(&nam)) {
            for (t, name) in f64s(d).iter().zip(nm.chunks_exact(48)) {
                let name = String::from_utf8_lossy(name);
                let name = name.trim_end_matches('\0').trim();
                if !name.is_empty() && *t > 0.0 {
                    table.push(format!("{name} = {t} m"));
                }
            }
        }
    }
    for id in m.plates.keys().copied().collect::<Vec<_>>() {
        m.plate_thickness.insert(id, DEFAULT_THICKNESS);
    }
    m.materials.push(Material {
        name: "Concrete (default)".into(),
        e: 3.2e10,
        g: 3.2e10 / 2.4,
        poisson: 0.2,
        density: 25_000.0,
        alpha: 1e-5,
    });
    m.notes.push(format!(
        "Robot .rtd is only partly decoded: {} panels read from their corner points; no \
         supports, loads or load cases. The panel -> thickness link is not read, so all panels \
         have {DEFAULT_THICKNESS} m (the file's thicknesses: {}). Material is a default \
         concrete (E 32 GPa). Export a .str from Robot for the full model.",
        m.plates.len(),
        if table.is_empty() {
            "n/a".to_string()
        } else {
            table.join(", ")
        }
    ));
    if zero > 0 {
        m.notes.push(format!(
            "{zero} panels (complex contours, e.g. the base slab) have no readable corner points"
        ));
    }
    if degenerate + stray > 0 {
        m.notes.push(format!(
            "{} panel records skipped (degenerate or not numbered)",
            degenerate + stray
        ));
    }
    Ok(m)
}
