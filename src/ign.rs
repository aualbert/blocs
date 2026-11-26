//! Géoplateforme web services: place search and département lookup.

use std::time::Duration;

use serde_json::Value;

use crate::relief::Result;

const USER_AGENT: &str = concat!("blocs/", env!("CARGO_PKG_VERSION"));
/// Longest wait for a short request (tile, search, lookup), so a stalled one is given up.
pub const SHORT: Duration = Duration::from_secs(30);

/// No overall timeout: relief archives take minutes. Short requests set `SHORT`.
pub fn client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(15))
        .timeout(None)
        .build()?)
}

/// A search suggestion and its WGS84 position.
pub struct Place {
    pub label: String,
    pub lon: f64,
    pub lat: f64,
}

/// Autocompletion on addresses and named places.
pub fn search(text: &str) -> Result<Vec<Place>> {
    let json: Value = serde_json::from_str(
        &client()?
            .get("https://data.geopf.fr/geocodage/completion")
            .query(&[("text", text), ("maximumResponses", "8")])
            .timeout(SHORT)
            .send()?
            .error_for_status()?
            .text()?,
    )?;
    let results = json["results"].as_array().cloned().unwrap_or_default();
    Ok(results
        .iter()
        .filter_map(|r| {
            Some(Place {
                label: r["fulltext"].as_str()?.to_owned(),
                lon: r["x"].as_f64()?,
                lat: r["y"].as_f64()?,
            })
        })
        .collect())
}

/// Départements `(code, name)` touching a Lambert-93 rectangle, codes as in RGE ALTI (`"D038"`).
pub fn departments(
    http: &reqwest::blocking::Client,
    [x0, y0, x1, y1]: [f64; 4],
) -> Result<Vec<(String, String)>> {
    // An exact geometry test: the WFS BBOX filter only compares envelopes.
    let filter = format!(
        "INTERSECTS(geometrie,SRID=2154;POLYGON(({x0} {y0},{x1} {y0},{x1} {y1},{x0} {y1},{x0} {y0})))"
    );
    let text = http
        .get("https://data.geopf.fr/wfs/ows")
        .query(&[
            ("SERVICE", "WFS"),
            ("VERSION", "2.0.0"),
            ("REQUEST", "GetFeature"),
            ("TYPENAMES", "ADMINEXPRESS-COG-CARTO.LATEST:departement"),
            ("outputFormat", "application/json"),
            ("PROPERTYNAME", "code_insee,nom_officiel"),
            ("CQL_FILTER", &filter),
        ])
        .timeout(SHORT)
        .send()?
        .error_for_status()?
        .text()?;
    let json: Value = serde_json::from_str(&text)?;
    let features = json["features"].as_array().cloned().unwrap_or_default();
    let mut deps: Vec<_> = features
        .iter()
        .filter_map(|f| {
            let p = &f["properties"];
            let code = p["code_insee"].as_str()?;
            // Overseas départements (3-digit codes) are outside the v1 scope.
            (code.len() == 2).then_some(())?;
            Some((
                format!("D{code:0>3}"),
                p["nom_officiel"].as_str()?.to_owned(),
            ))
        })
        .collect();
    deps.sort();
    Ok(deps)
}

/// Live service checks: `cargo test -- --ignored`.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn finds_chamrousse() {
        let places = search("chamrousse").unwrap();
        assert!(places.iter().any(|p| (p.lat - 45.11).abs() < 0.05));
    }

    #[test]
    #[ignore]
    fn paris_border_dalles() {
        // Two dalles west of Paris: only the second one is in the Paris archive.
        let codes = |x: f64, y: f64| -> Vec<String> {
            let rect = [x, y, x + 4995.0, y + 4995.0];
            let deps = departments(&client().unwrap(), rect).unwrap();
            deps.into_iter().map(|d| d.0).collect()
        };
        assert_eq!(codes(640_000.0, 6_855_005.0), ["D078", "D092"]);
        assert_eq!(codes(640_000.0, 6_860_005.0), ["D075", "D092"]);
    }
}
