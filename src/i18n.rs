//! Translations. French is the source text and the key: `tr!("Calques")` reads "Layers"
//! in English. A text missing from a table stays in French.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::relief::data_dir;

/// `tr!("Calques")` translates a text; `tr!("Relief installé : {}.", name)` also fills
/// its `{}` in order, giving a `String`.
macro_rules! tr {
    ($fr:literal) => {
        $crate::i18n::tr($fr)
    };
    ($fr:literal, $($arg:expr),+ $(,)?) => {
        $crate::i18n::fill($crate::i18n::tr($fr), &[$(&$arg as &dyn std::fmt::Display),+])
    };
}

/// The interface languages, French by default.
#[derive(Clone, Copy, PartialEq)]
pub enum Lang {
    Fr,
    En,
}

static ENGLISH: AtomicBool = AtomicBool::new(false);

/// Where the chosen language is kept between launches.
fn saved() -> std::path::PathBuf {
    data_dir().join("langue")
}

/// Takes up the language chosen last time.
pub fn load() {
    let en = std::fs::read_to_string(saved()).is_ok_and(|s| s.trim() == "en");
    ENGLISH.store(en, Ordering::Relaxed);
}

pub fn lang() -> Lang {
    match ENGLISH.load(Ordering::Relaxed) {
        true => Lang::En,
        false => Lang::Fr,
    }
}

/// Switches the interface to `lang` at once, and keeps it for the next launch.
pub fn set(lang: Lang) {
    ENGLISH.store(lang == Lang::En, Ordering::Relaxed);
    let _ = std::fs::create_dir_all(data_dir());
    let _ = std::fs::write(saved(), if lang == Lang::En { "en" } else { "fr" });
}

/// `fr` in the interface language.
pub fn tr(fr: &str) -> &str {
    if lang() == Lang::Fr {
        return fr;
    }
    static TABLE: OnceLock<HashMap<&str, &str>> = OnceLock::new();
    let table = TABLE.get_or_init(|| EN.iter().copied().collect());
    table.get(fr).copied().unwrap_or(fr)
}

/// `text` with each `{}` replaced by the next of `args`.
pub fn fill(text: &str, args: &[&dyn Display]) -> String {
    let mut parts = text.split("{}");
    let mut out = parts.next().unwrap_or_default().to_owned();
    for (arg, part) in args.iter().zip(parts) {
        out += &arg.to_string();
        out += part;
    }
    out
}

/// A number's text with the language's decimal mark: "8,2" in French, "8.2" in English.
pub fn decimal(number: String) -> String {
    match lang() {
        Lang::Fr => number.replace('.', ","),
        Lang::En => number,
    }
}

/// English, by French text.
const EN: &[(&str, &str)] = &[
    // Messages, bottom left.
    (
        "{} : téléchargement {} / {} Mo",
        "{}: downloading {} / {} MB",
    ),
    (
        "{} : préparation {} / {} dalles",
        "{}: preparing {} / {} tiles",
    ),
    ("Relief installé : {}.", "Relief installed: {}."),
    ("Téléchargement interrompu : {}", "Download interrupted: {}"),
    (
        "Calque indisponible sur le bloc : {}",
        "Layer unavailable on the block: {}",
    ),
    ("Échec : {}", "Failed: {}"),
    ("Réseau indisponible : {}", "Network unavailable: {}"),
    (
        "Départements introuvables : {}",
        "Départements not found: {}",
    ),
    ("Image enregistrée : {}", "Image saved: {}"),
    ("Échec de l'export : {}", "Export failed: {}"),
    (
        "Pas de connexion à Internet : seules les données en cache sont disponibles.",
        "No internet connection: only cached data is available.",
    ),
    ("Relief à télécharger : {}", "Relief to download: {}"),
    ("Télécharger", "Download"),
    ("Télécharger ({})", "Download ({})"),
    ("Reprendre", "Resume"),
    ("Pause", "Pause"),
    (" · {} en attente", " · {} waiting"),
    (" · en pause", " · paused"),
    (
        "Recherche des départements du cadre…",
        "Finding the frame's départements…",
    ),
    ("Construction du bloc…", "Building the block…"),
    ("Préparation : {}", "Preparing: {}"),
    ("Enregistrement de l'image…", "Saving the image…"),
    (
        "Plan IGN de base : {} / {} tuiles",
        "Base Plan IGN: {} / {} tiles",
    ),
    // Top bar and help.
    ("Rechercher un lieu…", "Search for a place…"),
    ("Tracer le cadre", "Draw the frame"),
    (
        "Glissez sur la carte pour tracer le cadre (10 km au plus par côté).",
        "Drag on the map to draw the frame (10 km per side at most).",
    ),
    ("Aide", "Help"),
    ("Carte", "Map"),
    ("Vue 3D", "3D view"),
    ("Vue double", "Both views"),
    (
        "Glisser : déplacer\nMolette : zoom\nMaj + molette : gauche-droite\nCtrl + molette : haut-bas",
        "Drag: move\nWheel: zoom\nShift + wheel: left-right\nCtrl + wheel: up-down",
    ),
    (
        "Glisser : tourner\nClic droit : déplacer\nMolette : zoom\nMaj + molette : gauche-droite\nCtrl + molette : haut-bas",
        "Drag: turn\nRight click: move\nWheel: zoom\nShift + wheel: left-right\nCtrl + wheel: up-down",
    ),
    // Options.
    ("Options", "Options"),
    ("Relief téléchargé", "Downloaded relief"),
    ("Aucun département.", "No département."),
    ("Plan IGN de base", "Base Plan IGN"),
    (
        "{} · téléchargé au lancement",
        "{} · downloaded at start-up",
    ),
    ("Cache des tuiles", "Tile cache"),
    ("au plus", "at most"),
    ("Supprimer", "Delete"),
    ("Confirmer", "Confirm"),
    ("Langue", "Language"),
    ("Mo", "MB"),
    ("Go", "GB"),
    // Layers.
    ("Calques", "Layers"),
    (
        "Zoomez pour afficher les calques.",
        "Zoom in to show the layers.",
    ),
    ("Opacité", "Opacity"),
    ("Photo aérienne", "Aerial photo"),
    ("Fond du Plan IGN", "Plan IGN background"),
    ("Bâtiments", "Buildings"),
    ("Végétation", "Vegetation"),
    ("Relief", "Relief"),
    ("Eau", "Water"),
    ("Bâti", "Built-up areas"),
    ("Routes", "Roads"),
    ("Voies ferrées", "Railways"),
    ("Limites", "Boundaries"),
    ("Toponymes", "Place names"),
    // Block.
    ("Bloc", "Block"),
    ("Exporter…", "Export…"),
    (
        "Image PNG, fond transparent",
        "PNG image, transparent background",
    ),
    ("Image PNG", "PNG image"),
    ("Socle", "Base"),
    ("Soleil à {} (heure solaire)", "Sun at {} (solar time)"),
    ("{} h {}, {} ou {}", "{}:{}, {} or {}"),
    (
        "Source : IGN – {} – Licence Ouverte 2.0 – consulté le {}",
        "Source: IGN – {} – Open Licence 2.0 – accessed on {}",
    ),
    // Axes: west, up, down.
    ("O", "W"),
    ("H", "U"),
    ("B", "D"),
    ("janvier", "January"),
    ("février", "February"),
    ("mars", "March"),
    ("avril", "April"),
    ("mai", "May"),
    ("juin", "June"),
    ("juillet", "July"),
    ("août", "August"),
    ("septembre", "September"),
    ("octobre", "October"),
    ("novembre", "November"),
    ("décembre", "December"),
    // Errors, shown after a message.
    ("pas de relief 5 m pour {}", "no 5 m relief for {}"),
    ("téléchargement en pause", "download paused"),
    (
        "le téléchargement n'avance plus",
        "the download has stalled",
    ),
    (
        "le serveur ne reprend pas le téléchargement",
        "the server cannot resume the download",
    ),
    ("en-tête tronqué", "truncated header"),
    (
        "dalle inattendue {}×{} au pas {}",
        "unexpected {}×{} tile, {} m apart",
    ),
    ("dalle tronquée", "truncated tile"),
    ("aucun relief téléchargé ici", "no relief downloaded here"),
    (
        "tuiles du Plan IGN indisponibles",
        "Plan IGN tiles unavailable",
    ),
    ("hors connexion", "offline"),
    ("Connexion rétablie.", "Connection restored."),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The text of a string literal that `code` starts inside, unescaped.
    fn literal(code: &str) -> String {
        let mut out = String::new();
        let mut chars = code.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' => match chars.next() {
                    Some('n') => out.push('\n'),
                    Some(c) => out.push(c),
                    None => break,
                },
                c => out.push(c),
            }
        }
        out
    }

    #[test]
    fn every_text_has_its_english() {
        let table: HashMap<_, _> = EN.iter().copied().collect();
        assert_eq!(table.len(), EN.len(), "a French text twice");
        for (fr, en) in EN {
            assert_eq!(fr.matches("{}").count(), en.matches("{}").count(), "{fr}");
        }
        // Each `tr!` of the code (not `include_str!`), its call split so that this line is not one.
        let call = concat!("tr", "!(");
        for file in std::fs::read_dir("src").unwrap() {
            let code = std::fs::read_to_string(file.unwrap().path()).unwrap();
            for (at, _) in code.match_indices(call) {
                let named = code[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_');
                let after = code[at + call.len()..].trim_start();
                if let Some(text) = after.strip_prefix('"').filter(|_| !named) {
                    let text = literal(text);
                    assert!(table.contains_key(text.as_str()), "no English: {text:?}");
                }
            }
        }
        // Texts translated by value.
        for c in crate::style::Category::ALL {
            assert!(table.contains_key(c.name()), "no English: {}", c.name());
        }
    }

    #[test]
    fn fills_in_order() {
        assert_eq!(fill("{} / {} Mo", &[&3, &"12"]), "3 / 12 Mo");
    }
}
