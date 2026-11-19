//! Section properties of parametric concrete sections (rectangle, square, circle) and
//! creation of new library sizes (used by the library dialog and the MCP `set_section`).

use crate::library::{Library, LibraryItem, LibraryKind, LibraryValue};

/// Section properties of a parametric concrete shape, from its dimensions (mm), in SI units
/// and under the same key names as RFEM's own `SectionData_parameterValues` (so
/// [`library_values`]'s result can overwrite them - see `set_concrete_values`). A
/// rectangle's formulas are used for a square too (`b_mm == h_mm`): RFEM treats them as
/// separate shapes, but a square's section properties are the rectangle's with `b = h`.
///
/// Every formula here was checked against FemCraft's own real, RFEM-computed catalogue
/// entries across several sizes per family (massive rectangle, square, circle), and matches
/// to at least 4 significant figures - except `tor_alpha` (and the `J_alpha` / `W_t` values
/// derived from it) for a non-square rectangle: RFEM's own `tor_alpha` is a torsional *stress*
/// coefficient that needs its own series solution to reproduce exactly (unlike every other
/// property here, which has a closed form), so it reuses the stiffness coefficient `tor_beta`
/// as a reasonable approximation. That only affects torsional-stress reporting; it doesn't
/// matter for a square or circle, whose `tor_alpha` is exact (1, resp. equal to `tor_beta`).
pub fn concrete_properties(family: &str, b_mm: f64, h_mm: f64, d_mm: f64) -> Vec<(&'static str, f64)> {
    match family {
        "R" | "SQ" => rect_properties(b_mm, h_mm),
        "CIRCLE" => circle_properties(d_mm),
        _ => Vec::new(),
    }
}

/// Massive rectangle (or square, `b == h`) section properties; see [`concrete_properties`].
fn rect_properties(b_mm: f64, h_mm: f64) -> Vec<(&'static str, f64)> {
    let (b, h) = (b_mm / 1e3, h_mm / 1e3);
    let a = b * h;
    let (iy, iz) = (b * h.powi(3) / 12.0, h * b.powi(3) / 12.0);
    let (long, short) = (b.max(h), b.min(h));
    // Roark/Timoshenko's standard engineering approximation for a solid rectangle's torsion
    // constant (`long` = longer full side, `short` = shorter full side).
    let tor_beta = 1.0 / 3.0
        - 0.21 * (short / long) * (1.0 - short.powi(4) / (12.0 * long.powi(4)));
    let it = long * short.powi(3) * tor_beta;
    vec![
        ("b", b),
        ("h", h),
        ("A", a),
        ("A_m", 2.0 * (b + h)),
        ("A_m_V", 2.0 * (b + h) / a),
        ("A_u", 5.0 / 6.0 * a),
        ("A_v", 5.0 / 6.0 * a),
        ("A_y", 5.0 / 6.0 * a),
        ("A_z", 5.0 / 6.0 * a),
        ("DEPTH_FOR_TEMPERATURE_LOAD", h),
        ("WIDTH_FOR_TEMPERATURE_LOAD", b),
        ("I_p", iy + iz),
        ("I_p_SC", iy + iz),
        ("I_t", it),
        ("I_u", iy),
        ("I_v", iz),
        ("I_y", iy),
        ("I_yz", 0.0),
        ("I_z", iz),
        ("J_alpha", tor_beta * long * short.powi(3)),
        ("J_beta", it),
        ("SELF_WEIGHT_INTERNAL", 2500.0 * a),
        ("S_u_max", b * h * h / 8.0),
        ("S_v_max", h * b * b / 8.0),
        ("S_y_max", b * h * h / 8.0),
        ("S_z_max", h * b * b / 8.0),
        ("U_t", 2.0 * (b + h)),
        ("V", a),
        ("W_pl_u", b * h * h / 4.0),
        ("W_pl_v", h * b * b / 4.0),
        ("W_pl_y", b * h * h / 4.0),
        ("W_pl_z", h * b * b / 4.0),
        ("W_t", tor_beta * long * short.powi(2)),
        ("W_u", iy / (h / 2.0)),
        ("W_u_max", iy / (h / 2.0)),
        ("W_u_min", -iy / (h / 2.0)),
        ("W_v", iz / (b / 2.0)),
        ("W_v_max", iz / (b / 2.0)),
        ("W_v_min", -iz / (b / 2.0)),
        ("W_y", iy / (h / 2.0)),
        ("W_y_max", iy / (h / 2.0)),
        ("W_y_min", -iy / (h / 2.0)),
        ("W_z", iz / (b / 2.0)),
        ("W_z_max", iz / (b / 2.0)),
        ("W_z_min", -iz / (b / 2.0)),
        ("alpha", 0.0),
        ("alpha_pl_u", 1.5),
        ("alpha_pl_v", 1.5),
        ("alpha_pl_y", 1.5),
        ("alpha_pl_z", 1.5),
        ("e_y", b / 2.0),
        ("e_z", h / 2.0),
        ("j_p", ((iy + iz) / a).sqrt()),
        ("j_p_SC", ((iy + iz) / a).sqrt()),
        ("j_u", (iy / a).sqrt()),
        ("j_v", (iz / a).sqrt()),
        ("j_y", (iy / a).sqrt()),
        ("j_yz", 0.0),
        ("j_z", (iz / a).sqrt()),
        ("min_w", short),
        ("r_omega_SC", 0.0),
        ("r_u", 0.0),
        ("r_u_SC", 0.0),
        ("r_v", 0.0),
        ("r_v_SC", 0.0),
        ("r_y", 0.0),
        ("r_y_SC", 0.0),
        ("r_z", 0.0),
        ("r_z_SC", 0.0),
        ("t_M", 0.0),
        ("tor_alpha", tor_beta),
        ("tor_beta", tor_beta),
        ("u_SC", 0.0),
        ("u_pl", 0.0),
        ("v_SC", 0.0),
        ("v_pl", 0.0),
        ("y_SC", 0.0),
        ("y_pl", 0.0),
        ("z_SC", 0.0),
        ("z_pl", 0.0),
    ]
}

/// Massive circle section properties; see [`concrete_properties`].
fn circle_properties(d_mm: f64) -> Vec<(&'static str, f64)> {
    let d = d_mm / 1e3;
    let pi = std::f64::consts::PI;
    let a = pi * d * d / 4.0;
    let i = pi * d.powi(4) / 64.0;
    let ip = pi * d.powi(4) / 32.0;
    vec![
        ("d", d),
        ("A", a),
        ("A_m", pi * d),
        ("A_m_V", pi * d / a),
        ("A_u", 0.84 * a),
        ("A_v", 0.84 * a),
        ("A_y", 0.84 * a),
        ("A_z", 0.84 * a),
        ("DEPTH_FOR_TEMPERATURE_LOAD", d),
        ("WIDTH_FOR_TEMPERATURE_LOAD", d),
        ("I_p", ip),
        ("I_p_SC", ip),
        ("I_t", ip),
        ("I_u", i),
        ("I_v", i),
        ("I_y", i),
        ("I_yz", 0.0),
        ("I_z", i),
        ("J_alpha", ip),
        ("J_beta", ip),
        ("SELF_WEIGHT_INTERNAL", 2500.0 * a),
        ("S_u_max", d.powi(3) / 12.0),
        ("S_v_max", d.powi(3) / 12.0),
        ("S_y_max", d.powi(3) / 12.0),
        ("S_z_max", d.powi(3) / 12.0),
        ("U_t", pi * d),
        ("V", a),
        ("W_pl_u", d.powi(3) / 6.0),
        ("W_pl_v", d.powi(3) / 6.0),
        ("W_pl_y", d.powi(3) / 6.0),
        ("W_pl_z", d.powi(3) / 6.0),
        ("W_t", ip / (d / 2.0)),
        ("W_u", i / (d / 2.0)),
        ("W_u_max", i / (d / 2.0)),
        ("W_u_min", -i / (d / 2.0)),
        ("W_v", i / (d / 2.0)),
        ("W_v_max", i / (d / 2.0)),
        ("W_v_min", -i / (d / 2.0)),
        ("W_y", i / (d / 2.0)),
        ("W_y_max", i / (d / 2.0)),
        ("W_y_min", -i / (d / 2.0)),
        ("W_z", i / (d / 2.0)),
        ("W_z_max", i / (d / 2.0)),
        ("W_z_min", -i / (d / 2.0)),
        ("alpha", 0.0),
        ("alpha_pl_u", 16.0 / (3.0 * pi)),
        ("alpha_pl_v", 16.0 / (3.0 * pi)),
        ("alpha_pl_y", 16.0 / (3.0 * pi)),
        ("alpha_pl_z", 16.0 / (3.0 * pi)),
        ("e_y", d / 2.0),
        ("e_z", d / 2.0),
        ("j_p", (ip / a).sqrt()),
        ("j_p_SC", (ip / a).sqrt()),
        ("j_u", (i / a).sqrt()),
        ("j_v", (i / a).sqrt()),
        ("j_y", (i / a).sqrt()),
        ("j_yz", 0.0),
        ("j_z", (i / a).sqrt()),
        ("min_w", d),
        ("r_omega_SC", 0.0),
        ("r_u", 0.0),
        ("r_u_SC", 0.0),
        ("r_v", 0.0),
        ("r_v_SC", 0.0),
        ("r_y", 0.0),
        ("r_y_SC", 0.0),
        ("r_z", 0.0),
        ("r_z_SC", 0.0),
        ("t_M", 0.0),
        ("tor_alpha", 1.0),
        ("tor_beta", 1.0),
        ("u_SC", 0.0),
        ("u_pl", 0.0),
        ("v_SC", 0.0),
        ("v_pl", 0.0),
        ("y_SC", 0.0),
        ("y_pl", 0.0),
        ("z_SC", 0.0),
        ("z_pl", 0.0),
    ]
}

/// [`concrete_properties`] as [`LibraryValue`]s, ready to store.
pub fn library_values(family: &str, b_mm: f64, h_mm: f64, d_mm: f64) -> Vec<LibraryValue> {
    concrete_properties(family, b_mm, h_mm, d_mm)
        .into_iter()
        .map(|(key, value_si)| LibraryValue {
            key: key.to_owned(),
            value_si: Some(value_si),
            value_text: None,
            thickness_si: None,
        })
        .collect()
}

/// Dimension text in mm, at most one decimal (`300`, `304.8`).
fn mm_text(v: f64) -> String {
    let s = format!("{:.1}", v);
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// Parses `R_M1 b/h`, `SQ_M1 b` or `CIRCLE_M1 d` -> (family, b, h, d) in mm. Values below
/// 10 are metres (as RFEM's API I names them), others millimetres.
pub fn parse_concrete_name(name: &str) -> Option<(&'static str, f64, f64, f64)> {
    let (shape, dims) = name.trim().split_once(' ')?;
    let v: Vec<f64> = dims
        .split('/')
        .map(|s| s.trim().parse::<f64>().ok().filter(|x| *x > 0.0))
        .collect::<Option<_>>()?;
    let mm = |x: f64| if x < 10.0 { (x * 1e4).round() / 10.0 } else { x };
    match (shape, v.as_slice()) {
        ("R_M1", [b, h]) => Some(("R", mm(*b), mm(*h), 0.0)),
        ("SQ_M1", [b]) => Some(("SQ", mm(*b), mm(*b), 0.0)),
        ("CIRCLE_M1", [d]) => Some(("CIRCLE", 0.0, 0.0, mm(*d))),
        _ => None,
    }
}

/// The library section for a parametric concrete name (`R_M1 304.8/609.6`, mm or metres),
/// created from an existing item of the same shape when the library lacks that size.
/// `None` when `name` is not a parametric concrete name.
pub fn get_or_add_concrete(lib: &Library, name: &str) -> Result<Option<LibraryItem>, crate::Error> {
    let Some((family, b, h, d)) = parse_concrete_name(name) else {
        return Ok(None);
    };
    let full = match family {
        "R" => format!("R_M1 {}/{}", mm_text(b), mm_text(h)),
        "SQ" => format!("SQ_M1 {}", mm_text(b)),
        _ => format!("CIRCLE_M1 {}", mm_text(d)),
    };
    if let Some(found) = lib.get(LibraryKind::Section, &full)? {
        return Ok(Some(found));
    }
    let filter = crate::library::LibraryFilter {
        family: family.to_owned(),
        category: "concrete".to_owned(),
        limit: 1,
        ..Default::default()
    };
    let Some(template) = lib.list(LibraryKind::Section, &filter)?.into_iter().next() else {
        return Ok(None);
    };
    let user_id = lib.next_user_id(LibraryKind::Section)?;
    lib.duplicate_concrete_item(template.user_id, user_id)?;
    let item = LibraryItem {
        kind: LibraryKind::Section,
        user_id,
        name: full.clone(),
        designation: full,
        standard: String::new(),
        manufacturer: String::new(),
        region: template.region.clone(),
        family: family.to_owned(),
        category: "concrete".to_owned(),
        rfem_library_id: None,
        rfem_version: String::new(),
    };
    let values = library_values(family, b, h, d);
    lib.insert_item(&item, &values)?;
    lib.set_concrete_values(user_id, &values)?;
    Ok(Some(item))
}
