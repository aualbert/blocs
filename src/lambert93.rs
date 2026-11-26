//! Lambert-93 (EPSG:2154) ↔ WGS84, closed-form conic projection on GRS80.
//! RGF93 and WGS84 differ by a few centimetres, which we ignore.

use std::f64::consts::FRAC_PI_4;

const E: f64 = 0.081_819_191_042_815_8; // GRS80 eccentricity
const N: f64 = 0.725_607_765_053_267; // cone constant
const C: f64 = 11_754_255.426_096; // projection constant (m)
const XS: f64 = 700_000.0; // pole x (m)
const YS: f64 = 12_655_612.049_876; // pole y (m)
const LON0: f64 = 3.0; // central meridian (deg)

/// Degrees (lon, lat) → Lambert-93 metres (x, y).
pub fn from_wgs84(lon: f64, lat: f64) -> (f64, f64) {
    let phi = lat.to_radians();
    let s = E * phi.sin();
    let iso_lat = ((FRAC_PI_4 + phi / 2.0).tan() * ((1.0 - s) / (1.0 + s)).powf(E / 2.0)).ln();
    let r = C * (-N * iso_lat).exp();
    let gamma = N * (lon - LON0).to_radians();
    (XS + r * gamma.sin(), YS - r * gamma.cos())
}

/// Lambert-93 metres (x, y) → degrees (lon, lat).
pub fn to_wgs84(x: f64, y: f64) -> (f64, f64) {
    let (dx, dy) = (x - XS, YS - y);
    let r = dx.hypot(dy);
    let lon = LON0 + (dx.atan2(dy) / N).to_degrees();
    let iso_lat = -(r / C).ln() / N;

    // Fixed-point iteration on latitude; converges in a handful of steps.
    let mut phi = 2.0 * iso_lat.exp().atan() - 2.0 * FRAC_PI_4;
    for _ in 0..10 {
        let s = E * phi.sin();
        phi =
            2.0 * (((1.0 + s) / (1.0 - s)).powf(E / 2.0) * iso_lat.exp()).atan() - 2.0 * FRAC_PI_4;
    }
    (lon, phi.to_degrees())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin() {
        let (x, y) = from_wgs84(3.0, 46.5);
        assert!((x - 700_000.0).abs() < 0.01 && (y - 6_600_000.0).abs() < 0.01);
    }

    #[test]
    fn round_trip() {
        let (lon, lat) = to_wgs84(from_wgs84(5.72, 45.19).0, from_wgs84(5.72, 45.19).1);
        assert!((lon - 5.72).abs() < 1e-9 && (lat - 45.19).abs() < 1e-9);
    }
}
