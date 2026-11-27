//! Géoplateforme web services: place search and département lookup.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// One client for the short requests (search, lookups, layers on the block), so that they
/// share its connections rather than opening new ones each time.
pub fn shared() -> Result<reqwest::blocking::Client> {
    static SHARED: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    if let Some(http) = SHARED.get() {
        return Ok(http.clone());
    }
    let http = client()?;
    Ok(SHARED.get_or_init(|| http).clone())
}

/// Set when the Géoplateforme did not answer at start-up: the app then sticks to its
/// cache, and asks the network nothing more until it answers a check again.
static OFFLINE: AtomicBool = AtomicBool::new(false);

/// Fails at once, before any request, when the app started offline.
pub fn reachable() -> Result<()> {
    match OFFLINE.load(Ordering::Relaxed) {
        true => Err(tr!("hors connexion").into()),
        false => Ok(()),
    }
}

/// Whether the Géoplateforme answers at all, whatever its reply; if not, the app goes offline.
pub fn online() -> bool {
    let head = |http: reqwest::blocking::Client| {
        let request = http
            .head("https://data.geopf.fr/")
            .timeout(Duration::from_secs(5));
        request.send().is_ok()
    };
    let online = shared().is_ok_and(head);
    OFFLINE.store(!online, Ordering::Relaxed);
    online
}

/// A search suggestion and its WGS84 position.
pub struct Place {
    pub label: String,
    pub lon: f64,
    pub lat: f64,
}

/// Autocompletion on addresses and named places.
pub fn search(text: &str) -> Result<Vec<Place>> {
    reachable()?;
    let json: Value = serde_json::from_str(
        &shared()?
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

/// Départements touching a Lambert-93 rectangle, coded as in RGE ALTI (`"D038"`).
pub fn departments(
    http: &reqwest::blocking::Client,
    [x0, y0, x1, y1]: [f64; 4],
) -> Result<Vec<String>> {
    reachable()?;
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
            ("PROPERTYNAME", "code_insee"),
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
            let code = f["properties"]["code_insee"].as_str()?;
            // Overseas départements (3-digit codes) are outside the v1 scope.
            (code.len() == 2).then(|| format!("D{code:0>3}"))
        })
        .collect();
    deps.sort();
    Ok(deps)
}

/// A département's name from its code (`"D038"` → `"Isère"`), or the code if unknown.
pub fn department_name(code: &str) -> &str {
    let number = code.strip_prefix("D0").unwrap_or(code);
    NAMES.iter().find(|n| n.0 == number).map_or(code, |n| n.1)
}

/// Metropolitan départements, by INSEE code.
const NAMES: [(&str, &str); 96] = [
    ("01", "Ain"),
    ("02", "Aisne"),
    ("03", "Allier"),
    ("04", "Alpes-de-Haute-Provence"),
    ("05", "Hautes-Alpes"),
    ("06", "Alpes-Maritimes"),
    ("07", "Ardèche"),
    ("08", "Ardennes"),
    ("09", "Ariège"),
    ("10", "Aube"),
    ("11", "Aude"),
    ("12", "Aveyron"),
    ("13", "Bouches-du-Rhône"),
    ("14", "Calvados"),
    ("15", "Cantal"),
    ("16", "Charente"),
    ("17", "Charente-Maritime"),
    ("18", "Cher"),
    ("19", "Corrèze"),
    ("2A", "Corse-du-Sud"),
    ("2B", "Haute-Corse"),
    ("21", "Côte-d'Or"),
    ("22", "Côtes-d'Armor"),
    ("23", "Creuse"),
    ("24", "Dordogne"),
    ("25", "Doubs"),
    ("26", "Drôme"),
    ("27", "Eure"),
    ("28", "Eure-et-Loir"),
    ("29", "Finistère"),
    ("30", "Gard"),
    ("31", "Haute-Garonne"),
    ("32", "Gers"),
    ("33", "Gironde"),
    ("34", "Hérault"),
    ("35", "Ille-et-Vilaine"),
    ("36", "Indre"),
    ("37", "Indre-et-Loire"),
    ("38", "Isère"),
    ("39", "Jura"),
    ("40", "Landes"),
    ("41", "Loir-et-Cher"),
    ("42", "Loire"),
    ("43", "Haute-Loire"),
    ("44", "Loire-Atlantique"),
    ("45", "Loiret"),
    ("46", "Lot"),
    ("47", "Lot-et-Garonne"),
    ("48", "Lozère"),
    ("49", "Maine-et-Loire"),
    ("50", "Manche"),
    ("51", "Marne"),
    ("52", "Haute-Marne"),
    ("53", "Mayenne"),
    ("54", "Meurthe-et-Moselle"),
    ("55", "Meuse"),
    ("56", "Morbihan"),
    ("57", "Moselle"),
    ("58", "Nièvre"),
    ("59", "Nord"),
    ("60", "Oise"),
    ("61", "Orne"),
    ("62", "Pas-de-Calais"),
    ("63", "Puy-de-Dôme"),
    ("64", "Pyrénées-Atlantiques"),
    ("65", "Hautes-Pyrénées"),
    ("66", "Pyrénées-Orientales"),
    ("67", "Bas-Rhin"),
    ("68", "Haut-Rhin"),
    ("69", "Rhône"),
    ("70", "Haute-Saône"),
    ("71", "Saône-et-Loire"),
    ("72", "Sarthe"),
    ("73", "Savoie"),
    ("74", "Haute-Savoie"),
    ("75", "Paris"),
    ("76", "Seine-Maritime"),
    ("77", "Seine-et-Marne"),
    ("78", "Yvelines"),
    ("79", "Deux-Sèvres"),
    ("80", "Somme"),
    ("81", "Tarn"),
    ("82", "Tarn-et-Garonne"),
    ("83", "Var"),
    ("84", "Vaucluse"),
    ("85", "Vendée"),
    ("86", "Vienne"),
    ("87", "Haute-Vienne"),
    ("88", "Vosges"),
    ("89", "Yonne"),
    ("90", "Territoire de Belfort"),
    ("91", "Essonne"),
    ("92", "Hauts-de-Seine"),
    ("93", "Seine-Saint-Denis"),
    ("94", "Val-de-Marne"),
    ("95", "Val-d'Oise"),
];

/// Live service checks: `cargo test -- --ignored`.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_departements() {
        assert_eq!(department_name("D038"), "Isère");
        assert_eq!(department_name("D02A"), "Corse-du-Sud");
        assert_eq!(department_name("D999"), "D999");
    }

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
            departments(&client().unwrap(), rect).unwrap()
        };
        assert_eq!(codes(640_000.0, 6_855_005.0), ["D078", "D092"]);
        assert_eq!(codes(640_000.0, 6_860_005.0), ["D075", "D092"]);
    }
}
