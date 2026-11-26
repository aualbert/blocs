# Cahier des charges — Blocs, générateur de blocs diagrammes (données ouvertes France)

Nov 25, 2026 · @Augustin

## Contexte et objectifs

Blocs, application de bureau gratuite et open source (GPL-3.0), affiche une carte de France et génère un bloc diagramme 3D d'au plus 10 km de côté, à partir du relief RGE ALTI 5 m de l'IGN téléchargé une fois par département.

Les données existent (relief, photos aériennes, cartes, hydrographie) mais les outils pour en tirer un bloc diagramme (QGIS, Blender) demandent des compétences SIG. L'application comble ce vide pour un public non spécialiste.

Objectifs mesurables :

- Obtenir un premier bloc en 4 actions : zoomer, accepter le téléchargement du département, tracer le cadre, passer en 3D.
- Passer de la carte à la 3D en un clic, ou afficher les deux côte à côte.
- Composer le rendu avec les couches de la carte, appliquées sur le terrain en 3D.
- Fonctionner hors connexion une fois les données téléchargées.

## Utilisateurs et principes

La cible est le grand public curieux : randonneurs, passionnés de paysages, curieux de leur région. Ils n'ont aucune compétence SIG et ne savent pas ce qu'est un MNT ou Lambert-93.

- **Zéro réglage obligatoire** : les valeurs par défaut donnent un beau bloc du premier coup.
- **Pas de jargon** : « relief » plutôt que « MNT », « photo aérienne » plutôt que « orthophoto ».
- **Rapide** : chaque action donne un retour visible immédiat, la génération affiche sa progression.
- **Honnête** : les sources et la résolution réelle des données sont toujours affichées et citées à l'export.
- **Puissant en option** : calques, imports et styles sont là pour qui les cherche, sans encombrer l'écran principal.

## Périmètre

La version 1 couvre la France métropolitaine (Corse comprise), sur ordinateur.

- Relief RGE ALTI 5 m, téléchargé par département.
- Blocs jusqu'à \~10 km de côté, en pleine résolution.
- Deux vues, Carte et 3D, avec écran partagé.
- Couches gérées sur la carte, appliquées sur le terrain 3D, téléchargeables pour l'emprise du cadre.
- Export PNG à fond transparent.
- Windows, Linux, macOS.

## Parcours utilisateur

Le parcours nominal tient en huit étapes ; le téléchargement (étape 3) n'a lieu qu'une fois par département.

1. L'utilisateur ouvre l'application sur la vue Carte : une carte de France (Plan IGN) s'affiche.
2. Il cherche un lieu ou zoome jusqu'à l'échelle d'un département.
3. L'app propose de télécharger le relief du ou des départements visibles, avec la taille totale ; il accepte.
4. Il trace un cadre rectangulaire ; ses dimensions s'affichent en km et le cadre ne dépasse pas 10 km.
5. Il active, masque ou réordonne les couches sur la carte, et peut les télécharger pour l'emprise du cadre.
6. Il passe en vue 3D en un clic, ou affiche carte et 3D côte à côte ; le bloc apparaît avec les couches appliquées sur le terrain.
7. En 3D, il oriente le bloc à la souris ou avec le widget d'axes, choisit la projection, règle le soleil et, sur le curseur vertical, l'étirement du relief et les profondeurs.
8. Il exporte une image PNG à fond transparent, sources citées automatiquement.

## Exigences fonctionnelles

Vingt-cinq exigences décrivent la v1 hors couches. Les exigences sur les couches sont regroupées dans la section suivante.

| ID | Exigence |
| --- | --- |
| VUE-1 | Deux vues, Carte et 3D, bascule en un clic |
| VUE-2 | Écran partagé : carte et 3D côte à côte |
| VUE-3 | En écran partagé, vues indépendantes : la carte reste nord en haut, la 3D s'oriente librement |
| VUE-4 | Toute modification des couches sur la carte se répercute sur le bloc 3D |
| VUE-5 | Bouton 3D grisé tant que le relief n'est pas téléchargé ou le cadre pas tracé, avec une infobulle qui dit ce qui manque |
| ZON-1 | Recherche de lieu avec autocomplétion (service Géoplateforme) |
| ZON-2 | Cadre rectangulaire tracé et modifié uniquement sur la carte, côté ≤ 10 km, dimensions affichées |
| ZON-3 | Saisie manuelle d'une emprise (coordonnées des coins) |
| TEL-1 | À l'échelle d'un département (\~1:250 000), proposition de télécharger le relief RGE ALTI 5 m des départements visibles, taille annoncée |
| TEL-2 | Cadre à cheval sur plusieurs départements : tous proposés en une seule demande, taille totale affichée |
| TEL-3 | Progression, pause et reprise du téléchargement ; décompression et conversion automatiques |
| TEL-4 | Téléchargement des couches (photo aérienne, Plan IGN, BD TOPO) pour l'emprise du cadre |
| STO-1 | Écran de gestion : départements et zones téléchargés, taille, suppression, dossier de stockage au choix |
| GEN-1 | Bloc 3D à 5 m, pleine résolution, construit depuis les données locales |
| GEN-2 | Socle fermé (faces latérales et dessous) |
| GEN-3 | Masses d'eau coupées par les bords du bloc (lacs, rivières larges, mer) correctement représentées en coupe : surface plane à son niveau, volume d'eau distinct du sol sur les faces latérales, fond plat dont la profondeur se règle sur le curseur vertical (REG-1), raccord propre avec la berge |
| NAV-1 | Projections perspective, orthographique et isométrique (inclinaison isométrique fixe, azimut libre) |
| NAV-2 | Orientation libre à la souris : rotation, inclinaison, zoom, déplacement |
| NAV-3 | Widget d'axes de navigation façon Blender / Godot : un clic sur un axe aligne la vue |
| REG-1 | Curseur vertical unique à droite de la vue 3D, à trois poignées : étirement du relief (exagération x1 à x5, défaut proposé x1,5), profondeur du fond de l'eau, profondeur du socle ; le fond de l'eau ne descend pas sous le dessous du socle |
| REG-2 | Azimut et hauteur du soleil |
| REG-3 | Ombres portées du relief |
| EXP-1 | Export PNG à fond transparent, à la résolution choisie (jusqu'à 7 680 px de large) |
| EXP-2 | Crédits des sources incrustés sur l'image (ex. « Source : IGN – Licence Ouverte ») |
| EXP-3 | Export conforme à la vue 3D affichée (projection, orientation, réglages) |

## Système de calques

Les couches se gèrent uniquement dans la vue Carte ; la vue 3D les applique telles quelles sur le terrain du bloc. Chaque couche porte son propre style. Au premier lancement, la pile contient la photo aérienne et les cours d'eau (visibles), les forêts et le Plan IGN (masqués).

| ID | Exigence |
| --- | --- |
| CAL-1 | Pile de couches ordonnée : visibilité, opacité, ordre par glisser-déposer |
| CAL-2 | Couches raster drapées (photo aérienne, Plan IGN, ombrage, teintes d'altitude, pentes) avec mode de fusion (normal, produit) |
| CAL-3 | Couches vecteur drapées (cours d'eau, plans d'eau, zones boisées) avec style propre : couleur, épaisseur, remplissage |
| CAL-4 | Import de fichiers : GeoTIFF, PNG / JPEG géoréférencé, GeoJSON, GPX, KML, GeoPackage, Shapefile |
| CAL-5 | Reprojection automatique vers Lambert-93 depuis WGS84 et Web Mercator ; message clair si la projection est inconnue |

## Sources de données ouvertes

Toutes les données par défaut viennent de l'IGN, sous Licence Ouverte 2.0 : réutilisation libre, à condition de citer la source. Le relief est lu sur le disque après téléchargement.

| Donnée | Usage dans l'app | Résolution | Accès | Remarque |
| --- | --- | --- | --- | --- |
| [RGE ALTI 5 m](https://geoservices.ign.fr/rgealti) | Relief du bloc | 5 m | Archives 7z par département (fichiers ASC, Lambert-93 / IGN69), service de téléchargement Géoplateforme | Mise à jour arrêtée en 2024 ; vide (nodata) sur les zones interdites (ZICAD) |
| BD ORTHO | Couche photo aérienne | 20 cm à 50 cm (à confirmer) | WMTS, tuiles téléchargées pour l'emprise du cadre | Tuiles à reprojeter sur le maillage |
| Plan IGN | Fond de la vue Carte, couche carte | Multi-échelle | WMTS / tuiles vectorielles |  |
| BD TOPO | Cours d'eau, plans d'eau, zones boisées | Vecteur métrique | WFS, objets téléchargés pour l'emprise du cadre | Couches à choisir dans le catalogue WFS |
| [Géocodage Géoplateforme](https://www.data.gouv.fr/dataservices/api-geoplateforme-geocodage/discussions) | Recherche de lieu | — | REST `data.geopf.fr/geocodage` | Remplace l'API Adresse historique |

Sources : [Référentiel à grande échelle (RGE) sur data.gouv.fr](https://www.data.gouv.fr/datasets/58e5842688ee386c65805755) · [Documentation des géoservices IGN](https://geoservices.ign.fr/usages)

## Exigences non fonctionnelles

Le relief étant local, la construction du bloc ne dépend pas du réseau ; le téléchargement, ponctuel, a ses propres exigences.

| Critère | Exigence |
| --- | --- |
| Résolution | Toujours 5 m : au plus 2 000 × 2 000 points pour un cadre de 10 km |
| Bascule de vue | Passage Carte ↔ 3D immédiat une fois le bloc construit |
| Fluidité | Manipulation 3D fluide sur GPU intégré récent ; démarrage rapide de l'app |
| Téléchargement | Taille annoncée avant accord ; reprise après coupure ; conversion des fichiers ASC en un format interne rapide à lire, puis suppression de l'archive |
| Hors connexion | Relief et couches téléchargés utilisables sans réseau |
| Respect des services | Limites d'usage de la Géoplateforme respectées, User-Agent identifiant l'app |
| Plateformes | Windows, macOS (Apple Silicon et Intel), Linux |
| Prise en main | Premier bloc obtenu sans aide ; interface en français, textes prêts pour la traduction |
| Distribution | Binaires signés : notarisation Apple (compte développeur payant), signature Windows ; AppImage ou Flatpak sous Linux |
| Licence du code | GPL-3.0 |
| Technologie | Rust ; interface egui (eframe), vue Carte avec walkers (tuiles IGN en Web Mercator), vue 3D en wgpu |

## Risques et points de vigilance

Le risque principal devient le téléchargement : des archives départementales lourdes, au format texte, à transformer avant usage. Le prototype doit mesurer taille et durée sur un département réel.

| Risque | Conséquence | Parade |
| --- | --- | --- |
| Archives départementales lourdes (taille exacte à mesurer) | Attente longue, abandon par le grand public | Taille annoncée avant accord, téléchargement en tâche de fond, reprise après coupure |
| Archives 7z multi-volumes et fichiers ASC (texte) lents à lire | Construction du bloc trop lente | Conversion unique en format binaire tuilé après téléchargement |
| Raccord entre dalles et entre départements | Coutures ou marches visibles sur le bloc | Index spatial des dalles, tests sur des cadres à cheval sur deux départements |
| Espace disque insuffisant | Échec du téléchargement ou de la conversion | Vérification de l'espace libre avant accord, écran de gestion du stockage |
| Téléchargement de tuiles en masse limité par les conditions d'usage de la Géoplateforme | Couches hors ligne incomplètes ou bloquées | Limiter à l'emprise du cadre, respecter les quotas, vérifier les conditions d'usage |
| Zones vides (ZICAD) dans le RGE ALTI | Trous dans le relief | Détection du nodata, comblement par interpolation, message à l'utilisateur |
| Tuiles WMTS en Web Mercator, relief en Lambert-93 | Couches décalées ou déformées en 3D | Reprojection des textures sur le maillage, tests sur des repères connus |
| Imports utilisateurs hétérogènes (projections, tailles) | Plantages, couches mal placées | Liste fermée de projections en v1, découpage à l'emprise, limite de taille |
| Empaquetage et signature sur 3 systèmes | Alertes de sécurité à l'installation | Intégration continue dédiée, budget pour les certificats |

## Feuille de route

Sans échéance imposée, le projet avance par jalons ; chacun se termine par un critère vérifiable avant de passer au suivant.

1. **Prototype de faisabilité** : télécharger et convertir le RGE ALTI 5 m d'un département, construire et afficher un bloc de 10 km.
   - Critère : taille et durée du téléchargement mesurées ; bloc affiché depuis le disque.
2. **Version minimale** : vue Carte avec recherche et cadre, proposition de téléchargement, vue 3D avec socle, projections, orientation et widget d'axes, export PNG à fond transparent.
   - Critère : un testeur non spécialiste obtient et exporte un bloc seul.
3. **Couches et écran partagé** : pile complète sur la carte, BD TOPO, styles, couches appliquées en 3D, carte et 3D côte à côte.
   - Critère : un changement de couche sur la carte apparaît sur le bloc sans reconstruction complète.
4. **Hors connexion et imports** : téléchargement des couches pour l'emprise du cadre, écran de gestion du stockage, imports de fichiers.
   - Critère : un bloc complet se reconstruit sans réseau.
5. **Version 1.0** : binaires signés pour les 3 systèmes, publication du code sous GPL-3.0.
   - Critère : installation sans alerte bloquante sur Windows, macOS et Linux.
