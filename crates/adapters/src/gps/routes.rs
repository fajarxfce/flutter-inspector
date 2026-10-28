use futures_util::StreamExt;
use inspector_domain::{GeoPoint, GpsRoute, MAX_ROUTE_POINTS};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::Path,
    time::Duration,
};

const MAX_BYTES: usize = 2 * 1024 * 1024;

pub fn import(path: &Path) -> Result<Vec<GeoPoint>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut text = String::new();
    file.take(MAX_BYTES as u64 + 1)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    parse(&text)
}

fn parse(text: &str) -> Result<Vec<GeoPoint>, String> {
    if text.len() > MAX_BYTES {
        return Err("Route files are limited to 2 MiB".into());
    }
    let points = if text.trim_start().starts_with('<') {
        // roxmltree rejects DTDs by default; do not resolve external entities.
        let document = roxmltree::Document::parse(text).map_err(|e| e.to_string())?;
        let segments: Vec<_> = document
            .descendants()
            .filter(|n| n.has_tag_name("trkseg") || n.has_tag_name("rte"))
            .collect();
        if segments.len() != 1 {
            return Err("GPX must contain one track segment or one route".into());
        }
        let mut points = Vec::new();
        for node in segments[0]
            .children()
            .filter(|n| n.has_tag_name("trkpt") || n.has_tag_name("rtept"))
        {
            if points.len() >= MAX_ROUTE_POINTS {
                return Err("Route exceeds 5000 points".into());
            }
            let number = |key: &str| {
                node.attribute(key)
                    .ok_or_else(|| format!("Missing {key}"))?
                    .parse::<f64>()
                    .map_err(|e| e.to_string())
            };
            let altitude = node
                .children()
                .find(|n| n.has_tag_name("ele"))
                .map(|n| {
                    n.text()
                        .unwrap_or("")
                        .parse::<f64>()
                        .map_err(|e| e.to_string())
                })
                .transpose()?
                .unwrap_or(0.0);
            points.push(GeoPoint {
                latitude: number("lat")?,
                longitude: number("lon")?,
                altitude,
            });
        }
        points
    } else {
        let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let geometry = if value["type"] == "Feature" {
            &value["geometry"]
        } else {
            &value
        };
        coordinates(geometry)?
    };
    Ok(GpsRoute::new(points)?.points().to_vec())
}

fn coordinates(geometry: &Value) -> Result<Vec<GeoPoint>, String> {
    if geometry["type"] != "LineString" {
        return Err("Use a GeoJSON LineString or Feature with a LineString geometry".into());
    }
    let coordinates = geometry["coordinates"]
        .as_array()
        .ok_or("Missing route coordinates")?;
    if coordinates.len() > MAX_ROUTE_POINTS {
        return Err("Route exceeds 5000 points".into());
    }
    coordinates
        .iter()
        .map(|item| {
            let point = item.as_array().ok_or("Invalid coordinate")?;
            if !(2..=3).contains(&point.len()) {
                return Err(
                    "A coordinate must contain longitude, latitude, and optional altitude".into(),
                );
            }
            Ok(GeoPoint {
                longitude: point[0].as_f64().ok_or("Invalid longitude")?,
                latitude: point[1].as_f64().ok_or("Invalid latitude")?,
                altitude: if point.len() == 3 {
                    point[2].as_f64().ok_or("Invalid altitude")?
                } else {
                    0.0
                },
            })
        })
        .collect()
}

pub fn export(path: &Path, points: Vec<GeoPoint>) -> Result<(), String> {
    let route = GpsRoute::new(points)?;
    let data = json!({"type":"Feature", "properties":{"name":"Inspector GPS route"}, "geometry":{"type":"LineString", "coordinates":route.points().iter().map(|p| vec![p.longitude,p.latitude,p.altitude]).collect::<Vec<_>>()}});
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(&mut temporary, &data).map_err(|e| e.to_string())?;
    temporary.flush().map_err(|e| e.to_string())?;
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    temporary.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

/// Explicit user action. Sends only the selected waypoints to the public OSRM service.
pub async fn directions(points: Vec<GeoPoint>) -> Result<Vec<GeoPoint>, String> {
    if !(2..=25).contains(&points.len()) {
        return Err("Road directions need 2–25 waypoints".into());
    }
    for point in &points {
        point.validate()?;
    }
    let coordinate_text = points
        .iter()
        .map(|p| format!("{},{}", p.longitude, p.latitude))
        .collect::<Vec<_>>()
        .join(";");
    let url = format!(
        "https://router.project-osrm.org/route/v1/driving/{coordinate_text}?overview=full&geometries=geojson&steps=false"
    );
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .user_agent("FlutterInspector/0.1 GPS")
        .build()
        .map_err(|e| e.to_string())?
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        if bytes.len() + chunk.len() > MAX_BYTES {
            return Err("Road route exceeds 2 MiB; use closer waypoints".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    tokio::task::spawn_blocking(move || {
        let value: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if value["code"] != "Ok" {
            return Err(value["message"]
                .as_str()
                .unwrap_or("No road route found")
                .to_owned());
        }
        let mut result = coordinates(&value["routes"][0]["geometry"])?;
        for point in &mut result {
            point.altitude = points[0].altitude;
        }
        Ok(GpsRoute::new(result)?.points().to_vec())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_gpx_and_geojson_but_rejects_disconnected_or_invalid_routes() {
        let gpx = "<gpx><trk><trkseg><trkpt lat='-6' lon='106'><ele>12</ele></trkpt><trkpt lat='-6.01' lon='106.01'/></trkseg></trk></gpx>";
        let points = parse(gpx).unwrap();
        assert_eq!(points[0].altitude, 12.0);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("route.geojson");
        export(&path, points.clone()).unwrap();
        assert_eq!(import(&path).unwrap(), points);
        assert!(parse("<gpx><trk><trkseg/><trkseg/></trk></gpx>").is_err());
        assert!(parse(r#"{"type":"LineString","coordinates":[[106,91],[106,1]]}"#).is_err());
        assert!(
            parse("<gpx><rte><rtept lat='NaN' lon='106'/><rtept lat='1' lon='2'/></rte></gpx>")
                .is_err()
        );
        assert!(parse(&"x".repeat(MAX_BYTES + 1)).is_err());
    }
}
