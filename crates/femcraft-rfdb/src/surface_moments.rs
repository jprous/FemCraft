//! Surface moments built up from the layer stresses, after RFEM 6's "Internal Forces" manual.
//!
//! Basic moments are thickness integrals of the stresses: `m_x = ∫σ_x z dz`, `m_xy = ∫τ_xy z dz`
//! over the thickness `d`. With the bending stress varying linearly between the two faces
//! (`σ_+` at `z = +d/2`, `σ_-` at `z = -d/2`) this gives `m = (σ_+ − σ_-)·d²/12`. Principal and
//! design moments are formulas on the basic moments. The principal and design formulas were
//! checked against RFEM 6 output for a slab (principal to 1e-5, design to 2e-5 kNm/m).
//!
//! Units: stresses in Pa, thickness in m; moments in N·m/m (divide by 1000 for kNm/m).
//! The design formulas use the RFEM 5 manual summary; the `m_c,D+` branch for `m_x > m_y` with
//! `m_y < -|m_xy|` and `m_x ≥ -|m_xy|` is taken from the RFEM output (the summary omits it).

/// Stresses on the two faces of a surface at one point: index 0 = `+` face (`z = +d/2`), index 1
/// = `-` face (`z = -d/2`), in the surface's local axes.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FaceStresses {
    pub sigma_x: [f64; 2],
    pub sigma_y: [f64; 2],
    pub tau_xy: [f64; 2],
}

/// Basic moments `m_x`, `m_y`, `m_xy` (N·m/m).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BasicMoments {
    pub mx: f64,
    pub my: f64,
    pub mxy: f64,
}

/// Principal moments (N·m/m) and angles (radians).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PrincipalMoments {
    pub m1: f64,
    pub m2: f64,
    /// Angle of principal axis 1 from the local x axis, `α_b = ½·atan2(2m_xy, m_x − m_y)`.
    pub alpha_b: f64,
    /// Maximum twisting moment `m_T,max,b`.
    pub m_t_max: f64,
}

/// Design moments (N·m/m) for the two faces (`+` and `-`), per the RFEM manual.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DesignMoments {
    pub mx_plus: f64,
    pub my_plus: f64,
    pub mc_plus: f64,
    pub mx_minus: f64,
    pub my_minus: f64,
    pub mc_minus: f64,
}

/// Basic moments from the face stresses over thickness `d` (m).
pub fn basic_moments(s: &FaceStresses, d: f64) -> BasicMoments {
    let k = d * d / 12.0;
    BasicMoments {
        mx: (s.sigma_x[0] - s.sigma_x[1]) * k,
        my: (s.sigma_y[0] - s.sigma_y[1]) * k,
        mxy: (s.tau_xy[0] - s.tau_xy[1]) * k,
    }
}

/// Generalised strains at one surface point, in the surface's local axes: the bending curvatures
/// `[κx, κy, κxy]` (1/m), the transverse shear strains `[γxz, γyz]` and the membrane strains
/// `[εx, εy, εxy]`. This is the order RFEM stores per element corner in the result block at
/// `i32[163]` (see `results.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GeneralisedStrains {
    pub kappa_x: f64,
    pub kappa_y: f64,
    pub kappa_xy: f64,
    pub gamma_xz: f64,
    pub gamma_yz: f64,
    pub epsilon_x: f64,
    pub epsilon_y: f64,
    pub epsilon_xy: f64,
}

/// The eight basic internal forces of a surface, in local axes (`m` in N·m/m, `v`/`n` in N/m).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BasicForces {
    pub mx: f64,
    pub my: f64,
    pub mxy: f64,
    pub vx: f64,
    pub vy: f64,
    pub nx: f64,
    pub ny: f64,
    pub nxy: f64,
}

/// Isotropic-plate stiffnesses for a surface of thickness `d` (m), elastic modulus `e` (Pa),
/// Poisson's ratio `nu`: bending `D`, membrane `C` and transverse shear `S`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlateStiffness {
    /// Bending stiffness `D = E·d³ / [12(1−ν²)]`.
    pub d_bend: f64,
    /// Membrane stiffness `C = E·d / (1−ν²)`.
    pub c_memb: f64,
    /// Transverse shear stiffness `S` (shear-corrected; `5/6·G·d` for a homogeneous section).
    pub s_shear: f64,
    pub nu: f64,
}

impl PlateStiffness {
    /// Isotropic stiffnesses from `e` (Pa), `nu`, thickness `d` (m). The shear stiffness uses the
    /// 5/6 shear correction factor; RFEM lets this be configured, so `s_shear` may be overridden.
    pub fn isotropic(e: f64, nu: f64, d: f64) -> Self {
        let g = e / (2.0 * (1.0 + nu));
        PlateStiffness {
            d_bend: e * d * d * d / (12.0 * (1.0 - nu * nu)),
            c_memb: e * d / (1.0 - nu * nu),
            s_shear: 5.0 / 6.0 * g * d,
            nu,
        }
    }
}

/// Basic internal forces from the generalised strains, by the isotropic plate constitutive law.
///
/// Confirmed against RFEM 6 output for a C30/37 slab (`E = 33 GPa`, `ν = 0.2`, `d = 0.4 m`):
/// deriving the basic forces from the export's own face strains reproduces `n_x`/`n_y`/`n_xy` to
/// < 0.01 kN/m and gives `m_xy/κ_xy = D(1−ν)/2` exactly. The sign convention matches RFEM with
/// moments negated (positive curvature → tension on the `+z` face → negative moment here).
pub fn basic_forces(g: &GeneralisedStrains, k: &PlateStiffness) -> BasicForces {
    let (d, c, s, nu) = (k.d_bend, k.c_memb, k.s_shear, k.nu);
    BasicForces {
        mx: -d * (g.kappa_x + nu * g.kappa_y),
        my: -d * (g.kappa_y + nu * g.kappa_x),
        mxy: -d * (1.0 - nu) / 2.0 * g.kappa_xy,
        vx: s * g.gamma_xz,
        vy: s * g.gamma_yz,
        nx: c * (g.epsilon_x + nu * g.epsilon_y),
        ny: c * (g.epsilon_y + nu * g.epsilon_x),
        nxy: c * (1.0 - nu) / 2.0 * g.epsilon_xy,
    }
}

/// The bending part of [`basic_forces`] as [`BasicMoments`] (feeds principal / design moments).
pub fn moments_from_forces(f: &BasicForces) -> BasicMoments {
    BasicMoments {
        mx: f.mx,
        my: f.my,
        mxy: f.mxy,
    }
}

/// Principal moments and the angle of axis 1 (`α_b`).
pub fn principal_moments(m: &BasicMoments) -> PrincipalMoments {
    let r = ((m.mx - m.my).powi(2) + 4.0 * m.mxy * m.mxy).sqrt() / 2.0;
    let mean = (m.mx + m.my) / 2.0;
    PrincipalMoments {
        m1: mean + r,
        m2: mean - r,
        alpha_b: 0.5 * (2.0 * m.mxy).atan2(m.mx - m.my),
        m_t_max: r,
    }
}

/// Design moments for both faces, RFEM manual formulas.
pub fn design_moments(m: &BasicMoments) -> DesignMoments {
    let (mx, my, mxy) = (m.mx, m.my, m.mxy);
    let a = mxy.abs();
    let (xy2, ay) = (mxy * mxy, my.abs());
    let ax = mx.abs();
    DesignMoments {
        mx_plus: if (mx <= my && mx >= -a) || (mx > my && my >= -a) {
            mx + a
        } else if mx > my && my < -a {
            mx + xy2 / ay
        } else {
            0.0
        },
        my_plus: if (my <= mx && my >= -a) || (my > mx && mx >= -a) {
            my + a
        } else if my > mx && mx < -a {
            my + xy2 / ax
        } else {
            0.0
        },
        mc_plus: if (mx <= my && mx >= -a) || (mx > my && my >= -a) {
            -2.0 * a
        } else if mx <= my && mx < -a {
            mx - xy2 / ax
        } else {
            // mx > my and my < -|mxy| (covers the case the manual summary leaves out).
            my - xy2 / ay
        },
        mx_minus: if (mx <= my && my <= a) || (mx > my && mx <= a) {
            -mx + a
        } else if mx <= my && my > a {
            -mx + xy2 / ay
        } else {
            0.0
        },
        my_minus: if (my <= mx && mx <= a) || (my > mx && my <= a) {
            -my + a
        } else if my <= mx && mx > a {
            -my + xy2 / ax
        } else {
            0.0
        },
        mc_minus: if (mx <= my && my <= a) || (mx > my && mx <= a) {
            -2.0 * a
        } else if mx <= my && my > a {
            -my - xy2 / ay
        } else {
            -mx - xy2 / ax
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9 * (1.0 + b.abs()), "{a} != {b}");
    }

    #[test]
    fn pure_bending_moment_from_face_stresses() {
        // σ = ±1 MPa over d = 0.2 m: m = 2e6 · 0.04 / 12.
        let s = FaceStresses { sigma_x: [1e6, -1e6], ..Default::default() };
        close(basic_moments(&s, 0.2).mx, 2e6 * 0.04 / 12.0);
    }

    #[test]
    fn principal_moments_of_simple_states() {
        let p = principal_moments(&BasicMoments { mx: 3.0, my: 1.0, mxy: 0.0 });
        close(p.m1, 3.0);
        close(p.m2, 1.0);
        close(p.alpha_b, 0.0);
        close(p.m_t_max, 1.0);
        // Pure twist: principal moments ±m_xy at 45°.
        let t = principal_moments(&BasicMoments { mx: 0.0, my: 0.0, mxy: 2.0 });
        close(t.m1, 2.0);
        close(t.m2, -2.0);
        close(t.alpha_b, std::f64::consts::FRAC_PI_4);
    }

    #[test]
    fn basic_forces_from_generalised_strains_use_the_plate_law() {
        // C30/37 slab: E = 33 GPa, nu = 0.2, d = 0.4 m (the confirmed Chamber values).
        let k = PlateStiffness::isotropic(33.0e9, 0.2, 0.4);
        close(k.d_bend, 33.0e9 * 0.4_f64.powi(3) / (12.0 * (1.0 - 0.04)));
        close(k.c_memb, 33.0e9 * 0.4 / (1.0 - 0.04));
        // Pure twist curvature: m_xy = -D(1-nu)/2 * kappa_xy, confirmed = 73333.3 * kappa_xy.
        let g = GeneralisedStrains { kappa_xy: 1.0e-4, ..Default::default() };
        close(basic_forces(&g, &k).mxy, -k.d_bend * 0.8 / 2.0 * 1.0e-4);
        // Pure membrane strain: n_x = C*(eps_x + nu*eps_y).
        let m = GeneralisedStrains { epsilon_x: 1.0e-4, epsilon_y: 0.0, ..Default::default() };
        close(basic_forces(&m, &k).nx, k.c_memb * 1.0e-4);
        close(basic_forces(&m, &k).ny, k.c_memb * 0.2 * 1.0e-4);
    }

    #[test]
    fn design_moments_follow_the_manual_branches() {
        // mx > my, my < -|mxy|: m_x,D+ = mx + mxy²/|my|, m_c,D+ = my - mxy²/|my|.
        let d = design_moments(&BasicMoments { mx: 1.0, my: -1.0, mxy: 0.5 });
        close(d.mx_plus, 1.0 + 0.25);
        close(d.mc_plus, -1.0 - 0.25);
        // Inside the band |m_x|, |m_y| small against |m_xy|: the concrete moment is -2|m_xy|.
        let e = design_moments(&BasicMoments { mx: -0.2, my: 0.1, mxy: 0.5 });
        close(e.mc_plus, -1.0);
        close(e.mc_minus, -1.0);
    }
}
