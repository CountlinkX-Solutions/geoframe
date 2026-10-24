//! Projections `proj4rs` does not have (or has in a form that differs from PROJ), written after
//! the PROJ formulas: ellipsoidal Equidistant Cylindrical (`eqc`, EPSG:4087), Cassini-Soldner
//! (`cass`), Hotine Oblique Mercator (`omerc`, variants A and B), Polyconic (`poly`), Equal Earth
//! (`eqearth`) and the New Zealand Map Grid (`nzmg`).
//!
//! A CRS that uses one of them is resolved as a pair: this module converts between the CRS
//! coordinates and geographic radians on the CRS ellipsoid (relative to its prime meridian), and
//! a `+proj=longlat` `proj4rs` projection with the same ellipsoid and datum does the datum shift
//! (see `reproject.rs`). Formulas follow PROJ's `eqc.cpp`, `cass.cpp`, `omerc.cpp`,
//! `poly.cpp`, `eqearth.cpp` and `nzmg.cpp`; `test/reference/fixtures/crs_datum/crs_parity.json`
//! holds the pyproj values they are tested against.

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

use proj4rs::Proj;

const TOL: f64 = 1e-7;
const EPS: f64 = 1e-10;

/// A projection added on top of `proj4rs`, with its false origin and unit already applied.
#[derive(Debug)]
pub(crate) struct Ext {
    kind: Kind,
    /// Semi-major axis (m) and eccentricity squared of the CRS ellipsoid.
    a: f64,
    es: f64,
    lam0: f64,
    phi0: f64,
    x0: f64,
    y0: f64,
    to_meter: f64,
    en: [f64; 5],
    /// Meridian distance of `phi0`.
    m0: f64,
    /// Direction of the first and second CRS axis (`+axis=swu` is `b"sw"`).
    axis: [u8; 2],
}

#[derive(Debug)]
enum Kind {
    Eqc { rc: f64 },
    Cass,
    Omerc(Omerc),
    Poly,
    EqEarth { qp: f64, rqda: f64 },
    Nzmg,
}

/// The projections handled here, by `+proj` name.
pub(crate) fn handles(proj: &str) -> bool {
    matches!(
        token_value(proj, "proj"),
        Some("eqc" | "cass" | "omerc" | "poly" | "eqearth" | "nzmg")
    )
}

fn token_value<'a>(proj: &'a str, key: &str) -> Option<&'a str> {
    proj.split_whitespace().find_map(|t| {
        let t = t.strip_prefix('+')?;
        let (k, v) = t.split_once('=')?;
        (k == key).then_some(v)
    })
}

/// The first two letters of `+axis` (default `en`).
fn axis_of(proj: &str) -> [u8; 2] {
    token_value(proj, "axis")
        .map(str::as_bytes)
        .filter(|b| b.len() >= 2)
        .map_or(*b"en", |b| [b[0], b[1]])
}

fn has_flag(proj: &str, key: &str) -> bool {
    proj.split_whitespace().any(|t| {
        t.strip_prefix('+') == Some(key) || t.strip_prefix('+') == Some(&format!("{key}="))
    })
}

fn number(proj: &str, key: &str) -> Result<Option<f64>, String> {
    token_value(proj, key)
        .map(|v| v.parse::<f64>().map_err(|_| format!("invalid +{key}={v}")))
        .transpose()
}

fn angle(proj: &str, key: &str) -> Result<Option<f64>, String> {
    Ok(number(proj, key)?.map(f64::to_radians))
}

/// Tokens that describe the projection (and its units) rather than the datum.
const PROJECTION_KEYS: &[&str] = &[
    "proj", "lat_0", "lon_0", "lonc", "alpha", "gamma", "azi", "k", "k_0", "x_0", "y_0", "lat_ts",
    "units", "to_meter", "no_uoff", "no_off", "no_rot", "axis", "lat_1", "lat_2", "south",
];

/// The `+proj=longlat` string with the datum of `proj` (ellipsoid, shift, prime meridian).
fn datum_string(proj: &str, drop: &[&str]) -> String {
    let kept: Vec<&str> = proj
        .split_whitespace()
        .filter(|t| {
            let key = t.trim_start_matches('+').split('=').next().unwrap_or("");
            !PROJECTION_KEYS.contains(&key) && !drop.contains(&key)
        })
        .collect();
    format!("+proj=longlat {}", kept.join(" "))
}

/// The proj string of the geographic CRS under a projected one, and the extension projection.
pub(crate) fn build(proj: &str) -> Result<(Ext, String), String> {
    let name = token_value(proj, "proj").unwrap_or("");
    // Semi-major axis and eccentricity from proj4rs: the geocentric coordinates of two points.
    let probe =
        datum_string(proj, &["towgs84", "nadgrids", "pm"]).replace("+datum=NAD27", "+ellps=clrk66");
    let geodetic = Proj::from_proj_string(&probe).map_err(|e| e.to_string())?;
    let geocentric = Proj::from_proj_string(&probe.replace("+proj=longlat", "+proj=geocent"))
        .map_err(|e| e.to_string())?;
    let mut equator = (0.0_f64, 0.0_f64, 0.0_f64);
    proj4rs::transform::transform(&geodetic, &geocentric, &mut equator)
        .map_err(|e| e.to_string())?;
    let mut pole = (0.0_f64, FRAC_PI_2, 0.0_f64);
    proj4rs::transform::transform(&geodetic, &geocentric, &mut pole).map_err(|e| e.to_string())?;
    let (a, b) = (equator.0, pole.2);
    let es = 1.0 - (b * b) / (a * a);

    let mut lam0 = angle(proj, "lon_0")?.unwrap_or(0.0);
    let mut phi0 = angle(proj, "lat_0")?.unwrap_or(0.0);
    let mut x0 = number(proj, "x_0")?.unwrap_or(0.0);
    let mut y0 = number(proj, "y_0")?.unwrap_or(0.0);
    let k0 = number(proj, "k_0")?.or(number(proj, "k")?).unwrap_or(1.0);
    let to_meter = match (number(proj, "to_meter")?, token_value(proj, "units")) {
        (Some(f), _) => f,
        (None, Some("m") | None) => 1.0,
        (None, Some("km")) => 1000.0,
        (None, Some("ft")) => 0.3048,
        (None, Some("us-ft")) => 1200.0 / 3937.0,
        (None, Some("ind-ft")) => 0.304_799_514_1,
        (None, Some("link")) => 0.201168,
        (None, Some(other)) => return Err(format!("unsupported +units={other}")),
    };
    let en = enfn(es);
    let kind = match name {
        "eqc" => {
            let ts = angle(proj, "lat_ts")?.unwrap_or(0.0);
            let rc = ts.cos() / (1.0 - es * ts.sin().powi(2)).sqrt();
            if ts.cos() <= 0.0 {
                return Err("lat_ts should be < 90 degrees".to_string());
            }
            Kind::Eqc { rc }
        }
        "cass" => Kind::Cass,
        "poly" => Kind::Poly,
        "eqearth" => {
            let e = es.sqrt();
            Kind::EqEarth {
                qp: qsfn(1.0, e, 1.0 - es),
                rqda: (0.5 * qsfn(1.0, e, 1.0 - es)).sqrt(),
            }
        }
        "nzmg" => {
            // Hard-wired in PROJ: International 1924, origin 41 S 173 E, false origin.
            phi0 = (-41.0_f64).to_radians();
            lam0 = 173.0_f64.to_radians();
            x0 = 2_510_000.0;
            y0 = 6_023_150.0;
            Kind::Nzmg
        }
        "omerc" => {
            let (omerc, lam0_omerc) = Omerc::new(proj, es, phi0, k0)?;
            lam0 = lam0_omerc;
            Kind::Omerc(omerc)
        }
        other => return Err(format!("projection {other} is not an extension")),
    };
    let (sin0, cos0) = phi0.sin_cos();
    let m0 = mlfn(phi0, sin0, cos0, &en);
    let ext = Ext {
        kind,
        a,
        es,
        lam0,
        phi0,
        x0,
        y0,
        to_meter,
        en,
        m0,
        axis: axis_of(proj),
    };
    // `+k` is only used by omerc here; the other projections ignore it like PROJ does for the
    // ones that have no scale factor (cass, poly, eqearth, eqc, nzmg).
    let _ = k0;
    let geographic = datum_string(proj, &[]);
    Ok((ext, geographic))
}

impl Ext {
    /// Geographic radians (relative to the prime meridian of the CRS) to CRS coordinates.
    pub(crate) fn forward(&self, lam: f64, phi: f64) -> Result<(f64, f64), String> {
        let lam = adjlon(lam - self.lam0);
        let (x, y) = match &self.kind {
            Kind::Eqc { rc } => {
                let (s, c) = phi.sin_cos();
                (rc * lam, mlfn(phi, s, c, &self.en) - self.m0)
            }
            Kind::Cass => self.cass_forward(lam, phi),
            Kind::Omerc(o) => o.forward(lam, phi, self.es)?,
            Kind::Poly => self.poly_forward(lam, phi),
            Kind::EqEarth { qp, rqda } => self.eqearth_forward(lam, phi, *qp, *rqda),
            Kind::Nzmg => self.nzmg_forward(lam, phi),
        };
        let (x, y) = (x * self.a, y * self.a);
        let (east, north) = ((x + self.x0) / self.to_meter, (y + self.y0) / self.to_meter);
        let pick = |d: u8| match d {
            b'e' => east,
            b'w' => -east,
            b'n' => north,
            _ => -north,
        };
        Ok((pick(self.axis[0]), pick(self.axis[1])))
    }

    /// CRS coordinates to geographic radians (relative to the prime meridian of the CRS).
    pub(crate) fn inverse(&self, x: f64, y: f64) -> Result<(f64, f64), String> {
        let (mut east, mut north) = (0.0, 0.0);
        for (d, v) in self.axis.iter().zip([x, y]) {
            match d {
                b'e' => east = v,
                b'w' => east = -v,
                b'n' => north = v,
                _ => north = -v,
            }
        }
        let (x, y) = (
            (east * self.to_meter - self.x0) / self.a,
            (north * self.to_meter - self.y0) / self.a,
        );
        let (lam, phi) = match &self.kind {
            Kind::Eqc { rc } => {
                let phi = inv_mlfn(y + self.m0, self.es, &self.en);
                (x / rc, phi)
            }
            Kind::Cass => self.cass_inverse(x, y),
            Kind::Omerc(o) => o.inverse(x, y, self.es)?,
            Kind::Poly => self.poly_inverse(x, y)?,
            Kind::EqEarth { qp, rqda } => self.eqearth_inverse(x, y, *qp, *rqda),
            Kind::Nzmg => self.nzmg_inverse(x, y)?,
        };
        if !(lam.is_finite() && phi.is_finite()) {
            return Err("non-finite result".to_string());
        }
        Ok((adjlon(lam + self.lam0), phi))
    }

    // -- Cassini-Soldner ---------------------------------------------------------------------

    fn cass_forward(&self, lam: f64, phi: f64) -> (f64, f64) {
        const C1: f64 = 1.0 / 6.0;
        const C2: f64 = 1.0 / 120.0;
        const C3: f64 = 1.0 / 24.0;
        let (s, mut c) = phi.sin_cos();
        let m = mlfn(phi, s, c, &self.en);
        let n = 1.0 / (1.0 - self.es * s * s).sqrt();
        let tn = phi.tan();
        let t = tn * tn;
        let a1 = lam * c;
        c *= self.es * c / (1.0 - self.es);
        let a2 = a1 * a1;
        let x = n * a1 * (1.0 - a2 * t * (C1 - (8.0 - t + 8.0 * c) * a2 * C2));
        let y = m - self.m0 + n * tn * a2 * (0.5 + (5.0 - t + 6.0 * c) * a2 * C3);
        (x, y)
    }

    fn cass_inverse(&self, x: f64, y: f64) -> (f64, f64) {
        const C3: f64 = 1.0 / 24.0;
        const C4: f64 = 1.0 / 12.0;
        const C5: f64 = 1.0 / 15.0;
        let ph1 = inv_mlfn(self.m0 + y, self.es, &self.en);
        let tn = ph1.tan();
        let t = tn * tn;
        let s = ph1.sin();
        let mut r = 1.0 / (1.0 - self.es * s * s);
        let n = r.sqrt();
        r *= (1.0 - self.es) * n;
        let dd = x / n;
        let d2 = dd * dd;
        let phi = ph1 - (n * tn / r) * d2 * (0.5 - (1.0 + 3.0 * t) * d2 * C3);
        let lam = dd * (1.0 + t * d2 * (-C4 + (1.0 + 3.0 * t) * d2 * C5)) / ph1.cos();
        (lam, phi)
    }

    // -- Polyconic ---------------------------------------------------------------------------

    fn poly_forward(&self, lam: f64, phi: f64) -> (f64, f64) {
        if phi.abs() <= TOL {
            return (lam, -self.m0);
        }
        let (sp, cp) = phi.sin_cos();
        let ms = if cp.abs() > TOL {
            cp / (1.0 - self.es * sp * sp).sqrt() / sp
        } else {
            0.0
        };
        let l = lam * sp;
        (
            ms * l.sin(),
            mlfn(phi, sp, cp, &self.en) - self.m0 + ms * (1.0 - l.cos()),
        )
    }

    fn poly_inverse(&self, x: f64, y: f64) -> Result<(f64, f64), String> {
        const ITOL: f64 = 1e-12;
        let es = self.es;
        let y = y + self.m0;
        if y.abs() <= TOL {
            return Ok((x, 0.0));
        }
        let r = y * y + x * x;
        let mut phi = y;
        let mut converged = false;
        for _ in 0..20 {
            let (sp, cp) = phi.sin_cos();
            let s2ph = sp * cp;
            if cp.abs() < ITOL {
                return Err("polyconic inverse failed".to_string());
            }
            let mut mlp = (1.0 - es * sp * sp).sqrt();
            let c = sp * mlp / cp;
            let ml = mlfn(phi, sp, cp, &self.en);
            let mlb = ml * ml + r;
            mlp = (1.0 - es) / (mlp * mlp * mlp);
            let dphi = (ml + ml + c * mlb - 2.0 * y * (c * ml + 1.0))
                / (es * s2ph * (mlb - 2.0 * y * ml) / c + 2.0 * (y - ml) * (c * mlp - 1.0 / s2ph)
                    - mlp
                    - mlp);
            phi += dphi;
            if dphi.abs() <= ITOL {
                converged = true;
                break;
            }
        }
        if !converged {
            return Err("polyconic inverse did not converge".to_string());
        }
        let c = phi.sin();
        let lam = (x * phi.tan() * (1.0 - es * c * c).sqrt()).asin() / phi.sin();
        Ok((lam, phi))
    }

    // -- Equal Earth -------------------------------------------------------------------------

    fn eqearth_forward(&self, lam: f64, phi: f64, qp: f64, rqda: f64) -> (f64, f64) {
        const A1: f64 = 1.340264;
        const A2: f64 = -0.081106;
        const A3: f64 = 0.000893;
        const A4: f64 = 0.003796;
        let m = 3.0_f64.sqrt() / 2.0;
        let e = self.es.sqrt();
        let beta = if self.es == 0.0 {
            phi
        } else {
            (qsfn(phi.sin(), e, 1.0 - self.es) / qp)
                .clamp(-1.0, 1.0)
                .asin()
        };
        let theta = (m * beta.sin()).asin();
        let t2 = theta * theta;
        let t6 = t2 * t2 * t2;
        let x = lam * theta.cos() / (m * (A1 + 3.0 * A2 * t2 + t6 * (7.0 * A3 + 9.0 * A4 * t2)));
        let y = theta * (A1 + A2 * t2 + t6 * (A3 + A4 * t2));
        (x * rqda, y * rqda)
    }

    fn eqearth_inverse(&self, x: f64, y: f64, qp: f64, rqda: f64) -> (f64, f64) {
        const A1: f64 = 1.340264;
        const A2: f64 = -0.081106;
        const A3: f64 = 0.000893;
        const A4: f64 = 0.003796;
        let m = 3.0_f64.sqrt() / 2.0;
        let (x, y) = (x / rqda, y / rqda);
        let mut theta = y;
        for _ in 0..12 {
            let t2 = theta * theta;
            let t6 = t2 * t2 * t2;
            let f = theta * (A1 + A2 * t2 + t6 * (A3 + A4 * t2)) - y;
            let fp = A1 + 3.0 * A2 * t2 + t6 * (7.0 * A3 + 9.0 * A4 * t2);
            let d = f / fp;
            theta -= d;
            if d.abs() < 1e-12 {
                break;
            }
        }
        let t2 = theta * theta;
        let t6 = t2 * t2 * t2;
        let beta = (theta.sin() / m).clamp(-1.0, 1.0).asin();
        let lam = m * x * (A1 + 3.0 * A2 * t2 + t6 * (7.0 * A3 + 9.0 * A4 * t2)) / theta.cos();
        let phi = if self.es == 0.0 {
            beta
        } else {
            geodetic_from_authalic(beta, self.es.sqrt(), 1.0 - self.es, qp)
        };
        (lam, phi)
    }

    // -- New Zealand Map Grid ----------------------------------------------------------------

    fn nzmg_forward(&self, lam: f64, phi: f64) -> (f64, f64) {
        let dphi = (phi - self.phi0) * RAD_TO_SEC5;
        let mut r = 0.0;
        for c in NZMG_TPSI.iter().rev() {
            r = c + dphi * r;
        }
        r *= dphi;
        let (pr, pi) = zpoly1((r, lam), &NZMG_BF);
        (pi, pr)
    }

    fn nzmg_inverse(&self, x: f64, y: f64) -> Result<(f64, f64), String> {
        let target = (y, x);
        let mut p = target;
        let mut converged = false;
        for _ in 0..20 {
            let (f, fp) = zpoly1_with_derivative(p, &NZMG_BF);
            let (fr, fi) = (f.0 - target.0, f.1 - target.1);
            let den = fp.0 * fp.0 + fp.1 * fp.1;
            let dr = -(fr * fp.0 + fi * fp.1) / den;
            let di = -(fi * fp.0 - fr * fp.1) / den;
            p.0 += dr;
            p.1 += di;
            if dr.abs() + di.abs() <= EPS {
                converged = true;
                break;
            }
        }
        if !converged {
            return Err("nzmg inverse did not converge".to_string());
        }
        let mut phi = 0.0;
        for c in NZMG_TPHI.iter().rev() {
            phi = c + p.0 * phi;
        }
        Ok((p.1, self.phi0 + p.0 * phi * SEC5_TO_RAD))
    }
}

const SEC5_TO_RAD: f64 = 0.484_813_681_109_536;
const RAD_TO_SEC5: f64 = 2.062_648_062_470_963_8;
const NZMG_BF: [(f64, f64); 6] = [
    (0.7557853228, 0.0),
    (0.249204646, 0.003371507),
    (-0.001541739, 0.041058560),
    (-0.10162907, 0.01727609),
    (-0.26623489, -0.36249218),
    (-0.6870983, -1.1651967),
];
const NZMG_TPHI: [f64; 9] = [
    1.5627014243,
    0.5185406398,
    -0.03333098,
    -0.1052906,
    -0.0368594,
    0.007317,
    0.01220,
    0.00394,
    -0.0013,
];
const NZMG_TPSI: [f64; 10] = [
    0.6399175073,
    -0.1358797613,
    0.063294409,
    -0.02526853,
    0.0117879,
    -0.0055161,
    0.0026906,
    -0.001333,
    0.00067,
    -0.00034,
];

type Complex = (f64, f64);

fn cmul(a: Complex, b: Complex) -> Complex {
    (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
}

/// `z * sum(c[k] z^k)` (PROJ's `pj_zpoly1`).
fn zpoly1(z: Complex, c: &[Complex]) -> Complex {
    let mut acc = *c.last().unwrap();
    for k in (0..c.len() - 1).rev() {
        let t = cmul(z, acc);
        acc = (c[k].0 + t.0, c[k].1 + t.1);
    }
    cmul(z, acc)
}

/// The polynomial of [`zpoly1`] and its derivative.
fn zpoly1_with_derivative(z: Complex, c: &[Complex]) -> (Complex, Complex) {
    let mut value = (0.0, 0.0);
    let mut derivative = (0.0, 0.0);
    // p(z) = sum c[k] z^(k+1); Horner on z^k terms then multiply by z.
    let mut acc = (0.0, 0.0);
    let mut dacc = (0.0, 0.0);
    for k in (0..c.len()).rev() {
        let d = cmul(z, dacc);
        dacc = (acc.0 + d.0, acc.1 + d.1);
        let t = cmul(z, acc);
        acc = (c[k].0 + t.0, c[k].1 + t.1);
    }
    let zacc = cmul(z, acc);
    value.0 += zacc.0;
    value.1 += zacc.1;
    let zd = cmul(z, dacc);
    derivative.0 += acc.0 + zd.0;
    derivative.1 += acc.1 + zd.1;
    (value, derivative)
}

// -- Hotine Oblique Mercator ----------------------------------------------------------------

#[derive(Debug)]
struct Omerc {
    a_big: f64,
    b: f64,
    e_big: f64,
    ar_b: f64,
    br_a: f64,
    r_b: f64,
    singam: f64,
    cosgam: f64,
    sinrot: f64,
    cosrot: f64,
    u_0: f64,
    v_pole_n: f64,
    v_pole_s: f64,
    no_rot: bool,
}

impl Omerc {
    /// Setup of PROJ's `omerc.cpp` for the `+alpha` / `+gamma` + `+lonc` forms (the two-point
    /// form has no EPSG code). Returns the projection and the central meridian it implies.
    fn new(proj: &str, es: f64, phi0: f64, k0: f64) -> Result<(Self, f64), String> {
        let alpha = angle(proj, "alpha")?;
        let gamma_given = angle(proj, "gamma")?;
        let lamc = angle(proj, "lonc")?.unwrap_or(0.0);
        if alpha.is_none() && gamma_given.is_none() {
            return Err("omerc: only the +alpha / +gamma forms are supported".to_string());
        }
        let no_off = has_flag(proj, "no_uoff") || has_flag(proj, "no_off");
        let no_rot = has_flag(proj, "no_rot");
        let e = es.sqrt();
        let one_es = 1.0 - es;
        let com = one_es.sqrt();
        let (b, a_big, d, mut f, e_big);
        if phi0.abs() > TOL {
            let (sinph0, cosph0) = phi0.sin_cos();
            let con = 1.0 - es * sinph0 * sinph0;
            let bb = cosph0 * cosph0;
            b = (1.0 + es * bb * bb / one_es).sqrt();
            a_big = b * k0 * com / con;
            d = b * com / (cosph0 * con.sqrt());
            f = d * d - 1.0;
            if f <= 0.0 {
                f = 0.0;
            } else {
                f = f.sqrt();
                if phi0 < 0.0 {
                    f = -f;
                }
            }
            f += d;
            e_big = f * tsfn(phi0, sinph0, e).powf(b);
        } else {
            b = 1.0 / com;
            a_big = k0;
            d = 1.0;
            f = 1.0;
            e_big = 1.0;
        }
        let (gamma0, alpha_c, gamma);
        if let Some(al) = alpha {
            alpha_c = al;
            gamma0 = (al.sin() / d).asin();
            gamma = gamma_given.unwrap_or(al);
        } else {
            let g = gamma_given.unwrap();
            gamma0 = g;
            alpha_c = (d * g.sin()).asin();
            gamma = g;
        }
        if (phi0.abs() - FRAC_PI_2).abs() <= TOL {
            return Err("omerc: lat_0 at a pole".to_string());
        }
        let lam0 = lamc - ((0.5 * (f - 1.0 / f)) * gamma0.tan()).asin() / b;
        let u_0 = if no_off {
            0.0
        } else {
            let u = (a_big * ((d * d - 1.0).max(0.0).sqrt() / alpha_c.cos()).atan() / b).abs();
            if phi0 < 0.0 {
                -u
            } else {
                u
            }
        };
        let ar_b = a_big / b;
        let half = 0.5 * gamma0;
        let (sinrot, cosrot) = if no_rot { (0.0, 1.0) } else { gamma.sin_cos() };
        Ok((
            Omerc {
                a_big,
                b,
                e_big,
                ar_b,
                br_a: 1.0 / ar_b,
                r_b: 1.0 / b,
                singam: gamma0.sin(),
                cosgam: gamma0.cos(),
                sinrot,
                cosrot,
                u_0,
                v_pole_n: ar_b * (FRAC_PI_4 - half).tan().ln(),
                v_pole_s: ar_b * (FRAC_PI_4 + half).tan().ln(),
                no_rot,
            },
            lam0,
        ))
    }

    fn forward(&self, lam: f64, phi: f64, es: f64) -> Result<(f64, f64), String> {
        let e = es.sqrt();
        let (u, v);
        if (phi.abs() - FRAC_PI_2).abs() > EPS {
            let w = self.e_big / tsfn(phi, phi.sin(), e).powf(self.b);
            let temp = 1.0 / w;
            let s = 0.5 * (w - temp);
            let t = 0.5 * (w + temp);
            let vv = (self.b * lam).sin();
            let uu = (s * self.singam - vv * self.cosgam) / t;
            if (uu.abs() - 1.0).abs() < EPS {
                return Err("omerc: point at infinity".to_string());
            }
            v = 0.5 * self.ar_b * ((1.0 - uu) / (1.0 + uu)).ln();
            let temp = (self.b * lam).cos();
            u = if temp.abs() < TOL {
                self.a_big * lam
            } else {
                self.ar_b * (s * self.cosgam + vv * self.singam).atan2(temp)
            };
        } else {
            v = if phi > 0.0 {
                self.v_pole_n
            } else {
                self.v_pole_s
            };
            u = self.ar_b * phi;
        }
        Ok(if self.no_rot {
            (u, v)
        } else {
            let u = u - self.u_0;
            (
                v * self.cosrot + u * self.sinrot,
                u * self.cosrot - v * self.sinrot,
            )
        })
    }

    fn inverse(&self, x: f64, y: f64, es: f64) -> Result<(f64, f64), String> {
        let e = es.sqrt();
        let (u, v) = if self.no_rot {
            (x, y)
        } else {
            (
                y * self.cosrot + x * self.sinrot + self.u_0,
                x * self.cosrot - y * self.sinrot,
            )
        };
        let qp = (-self.br_a * v).exp();
        if qp == 0.0 {
            return Err("omerc: out of range".to_string());
        }
        let sp = 0.5 * (qp - 1.0 / qp);
        let tp = 0.5 * (qp + 1.0 / qp);
        let vp = (self.br_a * u).sin();
        let up = (vp * self.cosgam + sp * self.singam) / tp;
        if (up.abs() - 1.0).abs() < EPS {
            return Ok((0.0, if up < 0.0 { -FRAC_PI_2 } else { FRAC_PI_2 }));
        }
        let ts = self.e_big / ((1.0 + up) / (1.0 - up)).sqrt();
        let phi = phi2(ts.powf(1.0 / self.b), e)?;
        let lam = -self.r_b * (sp * self.cosgam - vp * self.singam).atan2((self.br_a * u).cos());
        Ok((lam, phi))
    }
}

// -- shared series and helpers --------------------------------------------------------------

fn adjlon(lon: f64) -> f64 {
    if lon.abs() <= PI + 1e-12 {
        return lon;
    }
    let two_pi = 2.0 * PI;
    let mut l = lon + PI;
    l -= two_pi * (l / two_pi).floor();
    l - PI
}

fn tsfn(phi: f64, sinphi: f64, e: f64) -> f64 {
    let con = e * sinphi;
    (0.5 * (FRAC_PI_2 - phi)).tan() / ((1.0 - con) / (1.0 + con)).powf(0.5 * e)
}

fn phi2(ts: f64, e: f64) -> Result<f64, String> {
    let eccnth = 0.5 * e;
    let mut phi = FRAC_PI_2 - 2.0 * ts.atan();
    for _ in 0..15 {
        let con = e * phi.sin();
        let dphi = FRAC_PI_2 - 2.0 * (ts * ((1.0 - con) / (1.0 + con)).powf(eccnth)).atan() - phi;
        phi += dphi;
        if dphi.abs() <= 1e-10 {
            return Ok(phi);
        }
    }
    Err("omerc: latitude did not converge".to_string())
}

fn qsfn(sinphi: f64, e: f64, one_es: f64) -> f64 {
    if e >= 1e-7 {
        let con = e * sinphi;
        one_es * (sinphi / (1.0 - con * con) - (0.5 / e) * ((1.0 - con) / (1.0 + con)).ln())
    } else {
        sinphi + sinphi
    }
}

/// Geodetic latitude from the authalic latitude `beta` by Newton's method on `qsfn`.
fn geodetic_from_authalic(beta: f64, e: f64, one_es: f64, qp: f64) -> f64 {
    let target = qp * beta.sin();
    let mut phi = beta;
    for _ in 0..20 {
        let s = phi.sin();
        let con = e * s;
        let dqdphi = 2.0 * one_es * phi.cos() / ((1.0 - con * con) * (1.0 - con * con));
        let d = (qsfn(s, e, one_es) - target) / dqdphi;
        phi -= d;
        if d.abs() < 1e-14 {
            break;
        }
    }
    phi
}

fn enfn(es: f64) -> [f64; 5] {
    const C00: f64 = 1.0;
    const C02: f64 = 0.25;
    const C04: f64 = 0.046875;
    const C06: f64 = 0.019_531_25;
    const C08: f64 = 0.010_681_152_343_75;
    const C22: f64 = 0.75;
    const C44: f64 = 0.46875;
    const C46: f64 = 0.013_020_833_333_333_333;
    const C48: f64 = 0.007_120_768_229_166_667;
    const C66: f64 = 0.364_583_333_333_333_3;
    const C68: f64 = 0.005_696_614_583_333_333;
    const C88: f64 = 0.307_617_187_5;
    let t = es * es;
    [
        C00 - es * (C02 + es * (C04 + es * (C06 + es * C08))),
        es * (C22 - es * (C04 + es * (C06 + es * C08))),
        t * (C44 - es * (C46 + es * C48)),
        t * es * (C66 - es * C68),
        t * es * es * C88,
    ]
}

fn mlfn(phi: f64, sphi: f64, cphi: f64, en: &[f64; 5]) -> f64 {
    let cphi = cphi * sphi;
    let sphi = sphi * sphi;
    en[0] * phi - cphi * (en[1] + sphi * (en[2] + sphi * (en[3] + sphi * en[4])))
}

fn inv_mlfn(arg: f64, es: f64, en: &[f64; 5]) -> f64 {
    let k = 1.0 / (1.0 - es);
    let mut phi = arg;
    for _ in 0..20 {
        let (s, c) = phi.sin_cos();
        let mut t = 1.0 - es * s * s;
        t = (mlfn(phi, s, c, en) - arg) * (t * t.sqrt()) * k;
        phi -= t;
        if t.abs() < 1e-11 {
            break;
        }
    }
    phi
}
