//! The sun's apparent course: only positions it reaches during a year at the block's
//! latitude, so that the lighting stays realistic. Directions are in world axes: x east,
//! y up, z south.

use std::f32::consts::{PI, TAU};

use glam::Vec3;

/// Tilt of the Earth's axis: the sun's declination stays within ± this.
const TILT: f32 = 23.44 * PI / 180.0;
/// Lowest sun height kept: lower, the block would be all in shade.
const LOWEST: f32 = 5.0 * PI / 180.0;

/// The sun by solar time and season, at a latitude (radians).
#[derive(Clone, Copy)]
pub struct Sun {
    latitude: f32,
    /// Hour angle: 0 at solar noon, positive in the afternoon (15° per hour).
    hour: f32,
    /// Declination: -TILT at the winter solstice, +TILT at the summer one.
    declination: f32,
}

impl Sun {
    /// The reachable position nearest to the direction `d`.
    pub fn new(latitude: f32, d: Vec3) -> Self {
        let mut sun = Sun {
            latitude,
            hour: 0.0,
            declination: 0.0,
        };
        sun.snap(d);
        sun
    }

    /// Unit vector towards the sun.
    pub fn dir(&self) -> Vec3 {
        let (sp, cp) = self.latitude.sin_cos();
        let (sd, cd) = self.declination.sin_cos();
        let (sh, ch) = self.hour.sin_cos();
        let north = cp * sd - sp * cd * ch;
        Vec3::new(-cd * sh, sp * sd + cp * cd * ch, -north)
    }

    /// Moves to the reachable position nearest to `d`: its season, then its time of day
    /// within that day's sunrise and sunset.
    pub fn snap(&mut self, d: Vec3) {
        let (sp, cp) = self.latitude.sin_cos();
        let (east, up, north) = (d.x, d.y, -d.z);
        self.declination = (sp * up + cp * north)
            .clamp(-1.0, 1.0)
            .asin()
            .clamp(-TILT, TILT);
        let max = self.half_day();
        self.hour = (-east).atan2(cp * up - sp * north).clamp(-max, max);
    }

    /// The season: -1 at the winter solstice, 1 at the summer one.
    pub fn season(&self) -> f32 {
        self.declination / TILT
    }

    /// The day's course, from sunrise to sunset.
    pub fn path(&self) -> Vec<Vec3> {
        let max = self.half_day();
        (0..=32)
            .map(|k| {
                let hour = max * (k as f32 / 16.0 - 1.0);
                Sun { hour, ..*self }.dir()
            })
            .collect()
    }

    /// Solar time and the two dates of that season, e.g. "15 h 20, 5 mai ou 8 août".
    pub fn describe(&self) -> String {
        let minutes = (12.0 * 60.0 + self.hour.to_degrees() * 4.0).round() as u32;
        // Days from the spring equinox (20 March), back to it from the autumn one (22 Sept).
        let days = (self.declination / TILT).asin() / TAU * 365.25;
        tr!(
            "{} h {}, {} ou {}",
            minutes / 60,
            format!("{:02}", minutes % 60),
            date(78.0 + days),
            date(264.0 - days)
        )
    }

    /// Hour angle of sunset (above `LOWEST`) on this day.
    fn half_day(&self) -> f32 {
        let (sp, cp) = self.latitude.sin_cos();
        let (sd, cd) = self.declination.sin_cos();
        ((LOWEST.sin() - sp * sd) / (cp * cd))
            .clamp(-1.0, 1.0)
            .acos()
    }
}

/// "21 juin" for a day of the year, 0 being 1 January.
fn date(day: f32) -> String {
    const MONTHS: [(&str, u32); 12] = [
        ("janvier", 31),
        ("février", 28),
        ("mars", 31),
        ("avril", 30),
        ("mai", 31),
        ("juin", 30),
        ("juillet", 31),
        ("août", 31),
        ("septembre", 30),
        ("octobre", 31),
        ("novembre", 30),
        ("décembre", 31),
    ];
    let mut day = day.round().rem_euclid(365.0) as u32;
    for (name, length) in MONTHS {
        if day < length {
            return format!("{} {}", day + 1, crate::i18n::tr(name));
        }
        day -= length;
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equinox_noon_is_due_south_at_the_colatitude() {
        let sun = Sun {
            latitude: 45f32.to_radians(),
            hour: 0.0,
            declination: 0.0,
        };
        let d = sun.dir();
        assert!(d.x.abs() < 1e-6 && (d.y - d.z).abs() < 1e-6 && d.z > 0.0);
        assert_eq!(sun.describe(), "12 h 00, 20 mars ou 22 septembre");
    }

    #[test]
    fn snapping_keeps_reachable_positions_and_bounds_the_others() {
        let sun = Sun {
            latitude: 48f32.to_radians(),
            hour: 0.7,
            declination: 0.2,
        };
        let back = Sun::new(sun.latitude, sun.dir());
        assert!((back.hour - 0.7).abs() < 1e-4 && (back.declination - 0.2).abs() < 1e-4);
        // High in the north: never reached; snaps to a summer sunrise or sunset.
        let north = Sun::new(sun.latitude, Vec3::new(0.0, 0.5, -1.0).normalize());
        assert!((north.declination - TILT).abs() < 1e-6);
        assert!((north.dir().y - LOWEST.sin()).abs() < 1e-4);
    }
}
