//! Time & coordinate math: Julian dates, GMST, sun direction, frame transforms.
//!
//! World frame: right-handed, +Y = north pole, +X pierces (lat 0, lon 0).
//! ECEF -> world: (x, y, z)_world = (x_e, z_e, -y_e).
//! Units: 1.0 = one Earth radius (6371 km).

use chrono::{DateTime, Utc};
use glam::{DVec3, Vec3};

pub const EARTH_RADIUS_KM: f64 = 6371.0;
/// Earth rotation rate, rad/s.
pub const OMEGA_EARTH: f64 = 7.292_115_9e-5;

pub fn unix_seconds(t: DateTime<Utc>) -> f64 {
    t.timestamp() as f64 + f64::from(t.timestamp_subsec_micros()) * 1e-6
}

pub fn julian_date(t: DateTime<Utc>) -> f64 {
    2440587.5 + unix_seconds(t) / 86400.0
}

/// Greenwich Mean Sidereal Time in radians (IAU 1982, plenty for visualization).
pub fn gmst_rad(t: DateTime<Utc>) -> f64 {
    let jd = julian_date(t);
    let d = jd - 2451545.0;
    let tc = d / 36525.0;
    let deg = 280.460_618_37 + 360.985_647_366_29 * d + 0.000_387_933 * tc * tc
        - tc * tc * tc / 38_710_000.0;
    deg.rem_euclid(360.0).to_radians()
}

/// TEME (or ECI-equinox) -> ECEF rotation about Z by GMST.
pub fn teme_to_ecef(gmst: f64, p: DVec3) -> DVec3 {
    let (s, c) = gmst.sin_cos();
    DVec3::new(c * p.x + s * p.y, -s * p.x + c * p.y, p.z)
}

/// Velocity transform TEME -> ECEF: rotate, then subtract earth-rotation term.
pub fn teme_vel_to_ecef(gmst: f64, r_ecef: DVec3, v_teme: DVec3) -> DVec3 {
    let v_rot = teme_to_ecef(gmst, v_teme);
    // v_ecef = R v_teme - omega x r_ecef
    let w = DVec3::new(0.0, 0.0, OMEGA_EARTH);
    v_rot - w.cross(r_ecef)
}

/// ECEF (z = north) -> render world (y = north).
pub fn ecef_to_world(p: DVec3) -> DVec3 {
    DVec3::new(p.x, p.z, -p.y)
}

/// lat/lon in degrees -> unit vector in world frame.
pub fn latlon_to_world(lat_deg: f32, lon_deg: f32) -> Vec3 {
    let lat = lat_deg.to_radians();
    let lon = lon_deg.to_radians();
    Vec3::new(
        lat.cos() * lon.cos(),
        lat.sin(),
        -lat.cos() * lon.sin(),
    )
}

/// Unit sun direction in world frame (low-precision solar ephemeris, ~0.01 deg).
pub fn sun_dir_world(t: DateTime<Utc>) -> Vec3 {
    let n = julian_date(t) - 2451545.0;
    let l = (280.460 + 0.985_647_4 * n).rem_euclid(360.0).to_radians();
    let g = (357.528 + 0.985_600_3 * n).rem_euclid(360.0).to_radians();
    let lambda = l + 1.915_f64.to_radians() * g.sin() + 0.020_f64.to_radians() * (2.0 * g).sin();
    let eps = (23.439 - 0.000_000_4 * n).to_radians();
    // Equatorial frame (ECI)
    let eci = DVec3::new(
        lambda.cos(),
        eps.cos() * lambda.sin(),
        eps.sin() * lambda.sin(),
    );
    let ecef = teme_to_ecef(gmst_rad(t), eci);
    let w = ecef_to_world(ecef).normalize();
    Vec3::new(w.x as f32, w.y as f32, w.z as f32)
}
