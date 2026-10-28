use serde::{Deserialize, Serialize};

const EARTH_METERS: f64 = 6_371_008.8;
pub const MAX_ROUTE_POINTS: usize = 5000;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GeoPoint {
    pub latitude: f64,
    pub longitude: f64,
    #[serde(default)]
    pub altitude: f64,
}
impl Default for GeoPoint {
    fn default() -> Self {
        Self {
            latitude: -6.1754,
            longitude: 106.8272,
            altitude: 10.0,
        }
    }
}
impl GeoPoint {
    pub fn validate(self) -> Result<(), String> {
        for (name, value, min, max) in [
            ("Latitude", self.latitude, -90.0, 90.0),
            ("Longitude", self.longitude, -180.0, 180.0),
            ("Altitude", self.altitude, -12000.0, 100000.0),
        ] {
            if !value.is_finite() || value < min || value > max {
                return Err(format!("{name} must be between {min} and {max}"));
            }
        }
        Ok(())
    }
    pub fn distance_to(self, other: Self) -> f64 {
        let lat1 = self.latitude.to_radians();
        let lat2 = other.latitude.to_radians();
        let a = ((lat2 - lat1) / 2.0).sin().powi(2)
            + lat1.cos()
                * lat2.cos()
                * ((other.longitude - self.longitude).to_radians() / 2.0)
                    .sin()
                    .powi(2);
        EARTH_METERS * 2.0 * a.clamp(0.0, 1.0).sqrt().asin()
    }
    /// Degrees clockwise from true north, as reported by Android Location.
    pub fn bearing_to(self, other: Self) -> f64 {
        let delta = (other.longitude - self.longitude).to_radians();
        let lat1 = self.latitude.to_radians();
        let lat2 = other.latitude.to_radians();
        (delta.sin() * lat2.cos())
            .atan2(lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * delta.cos())
            .to_degrees()
            .rem_euclid(360.0)
    }
    pub fn moved(self, meters: f64, heading: f64) -> Self {
        let delta = meters / EARTH_METERS;
        let bearing = heading.to_radians();
        let latitude = self.latitude.to_radians();
        let lat = (latitude.sin() * delta.cos() + latitude.cos() * delta.sin() * bearing.cos())
            .clamp(-1.0, 1.0)
            .asin();
        let lon = self.longitude.to_radians()
            + (bearing.sin() * delta.sin() * latitude.cos())
                .atan2(delta.cos() - latitude.sin() * lat.sin());
        Self {
            latitude: lat.to_degrees(),
            longitude: (lon.to_degrees() + 180.0).rem_euclid(360.0) - 180.0,
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GpsFix {
    #[serde(flatten)]
    pub point: GeoPoint,
    pub accuracy: f64,
    /// Meters per second.
    pub speed: f64,
    pub heading: f64,
}
impl Default for GpsFix {
    fn default() -> Self {
        Self {
            point: Default::default(),
            accuracy: 5.0,
            speed: 0.0,
            heading: 0.0,
        }
    }
}
impl GpsFix {
    pub fn validate(self) -> Result<(), String> {
        self.point.validate()?;
        for (name, value, min, max) in [
            ("Accuracy", self.accuracy, 0.1, 10000.0),
            ("Speed", self.speed, 0.0, 400.0),
            ("Heading", self.heading, 0.0, 360.0),
        ] {
            if !value.is_finite() || value < min || value > max {
                return Err(format!("{name} must be between {min} and {max}"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct GpsRoute {
    points: Vec<GeoPoint>,
    distances: Vec<f64>,
}
impl GpsRoute {
    pub fn new(points: Vec<GeoPoint>) -> Result<Self, String> {
        if points.len() > MAX_ROUTE_POINTS {
            return Err(format!("Routes are limited to {MAX_ROUTE_POINTS} points"));
        }
        let mut clean: Vec<GeoPoint> = Vec::new();
        let mut distances = Vec::new();
        let mut total = 0.0;
        for point in points {
            point.validate()?;
            if let Some(previous) = clean.last() {
                let distance = previous.distance_to(point);
                if distance < 0.01 {
                    continue;
                }
                total += distance;
            }
            clean.push(point);
            distances.push(total);
        }
        if clean.len() < 2 {
            return Err("Add at least two different route points".into());
        }
        Ok(Self {
            points: clean,
            distances,
        })
    }
    pub fn points(&self) -> &[GeoPoint] {
        &self.points
    }
    pub fn length(&self) -> f64 {
        *self.distances.last().unwrap_or(&0.0)
    }
    pub fn sample(&self, meters: f64) -> (GeoPoint, f64) {
        let meters = meters.clamp(0.0, self.length());
        let segment = self
            .distances
            .partition_point(|distance| *distance <= meters)
            .saturating_sub(1)
            .min(self.points.len() - 2);
        let start = self.points[segment];
        let end = self.points[segment + 1];
        let distance = meters - self.distances[segment];
        let fraction = distance / (self.distances[segment + 1] - self.distances[segment]);
        let heading = start.bearing_to(end);
        let mut point = if meters >= self.length() {
            end
        } else {
            start.moved(distance, heading)
        };
        point.altitude = start.altitude + (end.altitude - start.altitude) * fraction;
        (point, heading)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cardinal_movement_and_dateline_are_consistent_in_meters() {
        let start = GeoPoint {
            latitude: 0.0,
            longitude: 179.999,
            altitude: 0.0,
        };
        for heading in [0.0, 90.0, 180.0, 270.0] {
            let end = start.moved(1000.0, heading);
            assert!((start.distance_to(end) - 1000.0).abs() < 0.001);
            assert!((start.bearing_to(end) - heading).abs() < 0.001);
            end.validate().unwrap();
        }
        assert!(start.moved(1000.0, 90.0).longitude < 0.0);
    }
    #[test]
    fn route_interpolates_turns_altitude_and_exact_endpoint_without_nan() {
        let start = GeoPoint::default();
        let middle = GeoPoint {
            altitude: 30.0,
            ..start.moved(100.0, 90.0)
        };
        let end = middle.moved(100.0, 0.0);
        let route = GpsRoute::new(vec![start, start, middle, end]).unwrap();
        assert_eq!(route.points().len(), 3);
        assert!((route.length() - 200.0).abs() < 0.001);
        let (half, heading) = route.sample(50.0);
        assert!((half.altitude - 20.0).abs() < 0.001);
        assert!((heading - 90.0).abs() < 0.001);
        assert!(route.sample(150.0).1 < 0.001);
        assert_eq!(route.sample(1000.0).0, end);
        assert!(GpsRoute::new(vec![start, start]).is_err());
        assert!(
            GpsFix {
                speed: f64::NAN,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
}
