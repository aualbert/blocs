# Cahier des charges — Blocs, générateur de blocs diagrammes (données ouvertes France)

Nov 25, 2026 · @Augustin

## Contexte et objectifs

Blocs, application de bureau gratuite et open source (GPL-3.0), affiche une carte de France et génère un bloc diagramme 3D d'au plus 10 km de côté, à partir du relief RGE ALTI 5 m de l'IGN téléchargé une fois par département.

Les données existent (relief, photos aériennes, cartes, hydrographie) mais les outils pour en tirer un bloc diagramme (QGIS, Blender) demandent des compétences SIG. L'application comble ce vide pour un public non spécialiste.

Objectifs mesurables :

- Obtenir un premier bloc en 3 actions : zoomer, tracer le cadre, accepter le téléchargement du relief ; le bloc s'ouvre de lui-même à côté de la carte.
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
- Couches gérées sur la carte, appliquées sur le terrain 3D, gardées en cache pour un usage hors connexion.
- Export PNG à fond transparent.
- Windows, Linux, macOS.

## Parcours utilisateur

Le parcours nominal tient en huit étapes ; le téléchargement (étape 4) n'a lieu qu'une fois par département.

1. L'utilisateur ouvre l'application sur la vue Carte : une carte de France (Plan IGN) s'affiche.
2. Il cherche un lieu ou zoome sur la carte.
3. Il trace un cadre rectangulaire ; ses dimensions s'affichent en km et le cadre ne dépasse pas 10 km.
4. Si le relief manque, l'app propose aussitôt de télécharger le ou les départements nécessaires au cadre, avec la taille totale ; il accepte d'un clic.
5. Il affiche, masque ou atténue les calques sur la carte.
6. Dès que le relief du cadre est installé, le bloc se construit et s'affiche à côté de la carte, avec les couches appliquées sur le terrain ; il glisse le séparateur pour ne garder qu'une vue.
7. En 3D, il oriente le bloc à la souris ou avec le widget d'axes, règle le soleil et, dans la carte Bloc, l'étirement du relief et la profondeur du socle.
8. Il exporte une image PNG à fond transparent, sources citées automatiquement.

## Exigences fonctionnelles

Vingt-neuf exigences décrivent la v1 hors couches. Les exigences sur les couches sont regroupées dans la section suivante.

| ID | Exigence |
| --- | --- |
| VUE-1 | Deux vues, Carte et 3D ; quand une seule est montrée, un bouton bascule sur l'autre |
| VUE-2 | Écran partagé : carte à gauche, 3D à droite, séparées par un trait que l'on glisse ; glissé à moins de 100 points d'un bord, il s'y aimante et, relâché, ne laisse que l'autre vue ; un bouton « Vue double » revient au partage, moitié-moitié |
| VUE-3 | En écran partagé, vues indépendantes : la carte reste nord en haut, la 3D s'oriente librement |
| VUE-4 | Toute modification des couches sur la carte se répercute sur le bloc 3D ; les calques s'y appliquent du bas vers le haut, les toponymes en dernier |
| VUE-5 | Pas de boutons de vue : la carte seule tant qu'il n'y a pas de bloc. Le bloc se construit dès que le cadre est tracé et son relief installé, y compris à la fin d'un téléchargement en cours, quelle que soit la vue ; une fois construit, l'écran partagé s'ouvre |
| IHM-1 | Pas de panneaux : les vues occupent toute la fenêtre, les commandes flottent dessus sur de petites cartes, le long du bord gauche. En haut, la barre : recherche, « Tracer le cadre » (grisé en 3D seule), Options, puis Aide, un menu des gestes de la souris sur la carte et en 3D ; à sa droite, si une seule vue est montrée et le bloc chargé, les boutons de vue. Dessous, la carte Calques, toujours présente, puis la carte Bloc dès que la 3D est montrée : « Exporter… » et la taille de l'image à droite de son titre, puis les réglages du bloc |
| IHM-2 | Tous les messages au même endroit, en bas à gauche, une ligne chacun, et rien quand rien ne se passe : téléchargement du relief (avancement en texte, pause), proposition de téléchargement, recherche des départements du cadre, construction du bloc, préparation des calques, enregistrement de l'image, téléchargement du Plan IGN de base. Pas de message pour les tuiles de la carte : il clignoterait sans cesse. L'issue de la dernière tâche, réussite ou erreur, s'efface après 10 s, ou tout de suite d'un clic sur « x ». Au démarrage, sans connexion à Internet, un avertissement le signale et reste jusqu'au clic sur « x » ; l'app s'en tient alors à son cache, sans requête, et revérifie la connexion après 10 s, 20 s, 40 s… (10 min au plus), puis annonce son retour |
| ZON-1 | Recherche de lieu avec autocomplétion (service Géoplateforme) ; la carte se centre sur le lieu choisi, au niveau de zoom 15 (\~5 km de large) |
| ZON-2 | Cadre rectangulaire tracé et modifié uniquement sur la carte, côté ≤ 10 km, dimensions affichées. Une poignée, au milieu de son côté nord, le fait tourner par degré entier ; la carte reste nord en haut, le bloc suit le cadre, le soleil et les axes 3D restent sur les points cardinaux |
| TEL-1 | Une fois le cadre tracé, proposition de télécharger le relief RGE ALTI 5 m des seuls départements nécessaires au cadre, taille annoncée ; un seul bouton « Télécharger », sans case à cocher ni bouton « Plus tard » ; la proposition s'affiche immédiatement, la taille suit dès qu'elle est connue |
| TEL-2 | Cadre à cheval sur plusieurs départements : on propose le plus petit ensemble d'archives qui couvre le cadre. Une archive contient toutes les dalles de 5 km qui touchent son département ; un département voisin n'est proposé que si une dalle du cadre ne touche aucun département déjà retenu ou installé (ex. Clécy : le Calvados suffit) |
| TEL-3 | Progression, pause et reprise du téléchargement ; décompression et conversion automatiques |
| STO-1 | Menu Options : relief de chaque département téléchargé, Plan IGN de base (CAR-2) et cache des tuiles, avec leur taille et leur suppression ; taille du cache réglable ; en bas, la langue (français par défaut, anglais), changée aussitôt et gardée. Reporté : dossier de stockage au choix |
| GEN-1 | Bloc 3D à 5 m, pleine résolution, construit depuis les données locales |
| GEN-2 | Socle fermé (faces latérales et dessous), du gris clair uni des bâtiments |
| GEN-3 | Bâtiments en 3D, toujours affichés : chaque bâtiment du Plan IGN est extrudé à sa hauteur réelle (non exagérée), posé sur le relief, en gris clair uni. L'eau en coupe sur les faces latérales a été essayée puis écartée : rendu peu convaincant, et pas de profondeur des lacs disponible |
| NAV-1 | Projection axonométrique (orthographique) unique, celle des blocs diagrammes dessinés : sans point de fuite, une seule échelle sur tout le bloc ; orientation libre, la vue isométrique en est un cas particulier |
| NAV-2 | Orientation libre à la souris : rotation, inclinaison, zoom, déplacement. Molette : zoom ; Maj + molette : déplacement gauche-droite ; Ctrl + molette : déplacement haut-bas. Même convention sur la carte et en 3D. Sur la carte, le zoom arrière s'arrête quand le monde remplit la vue, et la carte ne sort jamais de ses bords |
| NAV-3 | Widget d'axes de navigation façon Blender / Godot : un clic sur un axe aligne la vue |
| REG-1 | Carte Bloc : étirement du relief (exagération x1 à x2, défaut x1,5) et profondeur du socle (10 à 1 000 m, curseur logarithmique, défaut 6 % de la largeur du bloc) |
| REG-2 | Azimut et hauteur du soleil |
| REG-3 | Ombres portées du relief |
| REG-4 | Ciel derrière le bloc, réglé par le soleil : du zénith en haut à l'horizon en bas, chaud et lumineux du côté du soleil quand il est bas, bleu profond quand il est haut ; plus pâle en hiver, plus soutenu en été. À l'écran seulement : l'export garde son fond transparent |
| EXP-1 | Export PNG à fond transparent, en 1 280, 1 920 (défaut), 3 840, 5 760 ou 7 680 px sur le plus grand côté, dans la limite du GPU |
| EXP-2 | Crédits des sources incrustés sur l'image, avec leur date de consultation comme le demande la Licence Ouverte (ex. « Source : IGN – RGE ALTI® 5 m, Plan IGN – Licence Ouverte 2.0 – consulté le 27/11/2026 ») ; les mêmes crédits s'affichent en bas à droite de la fenêtre, selon les sources montrées |
| EXP-3 | Export conforme à la vue 3D affichée (projection, orientation, réglages), cadré sur le bloc entier quels que soient le zoom et le déplacement à l'écran |
| CAR-1 | Tuiles des calques (photo aérienne et Plan IGN vectoriel) mises en cache sur le disque : le Plan IGN de base (CAR-2) est gardé en permanence ; les autres tuiles partagent un cache plafonné (500 Mo par défaut, de 100 Mo à 5 Go dans les options), les moins récemment vues effacées en premier. Les tuiles drapées sur un bloc y restent aussi : pas de téléchargement des calques à part |
| CAR-2 | Plan IGN de base : les niveaux 0 à 10 du Plan IGN vectoriel sur la France métropolitaine (\~95 Mo, dont 89 Mo pour le niveau 10), et les niveaux 0 à 4 sur le monde entier, téléchargés une fois, en tâche de fond : vues d'ensemble immédiates, jusqu'à l'échelle des continents, et disponibles hors connexion. Une tuile que le serveur n'a pas (404, ex. les pôles) est notée sur le disque par un fichier vide et n'est plus jamais redemandée ; sa tuile parente s'affiche à sa place |
| CAR-3 | Chargement fluide : tuiles visibles prioritaires, chargées du centre de l'écran vers les bords ; niveaux de zoom voisins préchargés ; en attendant, la tuile plus grossière s'affiche, jamais un carré vide ; fond de carte blanc. Plan IGN dessiné en tuiles de 512 px, comme le prévoit son style : détails et toponymes apparaissent un niveau de zoom plus tard, pour une carte moins chargée. Quand la carte bouge, les requêtes pas encore commencées sont abandonnées, celles en cours vont à leur terme |

## Système de calques

Les calques se gèrent dans la carte Calques, présente dans toutes les vues ; la vue 3D les applique tels quels sur le terrain du bloc. La pile se compose de la photo aérienne, toujours en dessous, et du Plan IGN vectoriel découpé en catégories, de haut en bas : toponymes, limites, voies ferrées, routes, bâti, eau, relief, végétation, comme les thèmes de l'IGN. Au premier lancement, toutes les catégories sont visibles et la photo aérienne masquée. En vue d'ensemble (niveaux de zoom inférieurs à 11), la carte montre le Plan IGN complet, quels que soient les calques choisis.

| ID | Exigence |
| --- | --- |
| CAL-1 | Pile de calques : visibilité et opacité de chaque calque, sans délai. Ordre fixe : celui du Plan IGN, la photo aérienne toujours en dessous. Les catégories du Plan IGN forment un groupe, replié par défaut, dont la visibilité et l'opacité s'appliquent à toutes ; chacune garde les siennes. Les toponymes, sans curseur, restent toujours opaques : seule la case du groupe les concerne |
| CAL-2 | Plan IGN vectoriel drapé, dessiné avec le style officiel de l'IGN et découpé en catégories selon les données des tuiles. Pas d'ombrage : l'éclairage de la 3D fait les ombres, un ombrage précalculé les doublerait (le Plan IGN en images, ombré, est écarté pour cette raison). Le fond blanc du Plan IGN n'est dessiné que si la photo aérienne est masquée |
| CAL-3 | Photo aérienne drapée, seul calque image ; elle garde ses ombres naturelles. Reportés : styles réglables des catégories (couleur, épaisseur, remplissage), teintes d'altitude, mode de fusion (produit) |
| CAL-4 | Import de fichiers : GeoTIFF, PNG / JPEG géoréférencé, GeoJSON, GPX, KML, GeoPackage, Shapefile |
| CAL-5 | Reprojection automatique vers Lambert-93 depuis WGS84 et Web Mercator ; message clair si la projection est inconnue |

## Sources de données ouvertes

Toutes les données par défaut viennent de l'IGN, sous Licence Ouverte 2.0 : réutilisation libre, à condition de citer la source. Le relief est lu sur le disque après téléchargement.

| Donnée | Usage dans l'app | Résolution | Accès | Remarque |
| --- | --- | --- | --- | --- |
| [RGE ALTI 5 m](https://geoservices.ign.fr/rgealti) | Relief du bloc | 5 m | Archives 7z par département (fichiers ASC, Lambert-93 / IGN69), service de téléchargement Géoplateforme | Mise à jour arrêtée en 2024 ; vide (nodata) sur les zones interdites (ZICAD) |
| BD ORTHO | Couche photo aérienne | 20 cm à 50 cm (à confirmer) | WMTS, tuiles téléchargées pour l'emprise du cadre | Tuiles à reprojeter sur le maillage |
| [Plan IGN vectoriel](https://data.geopf.fr/tms/1.0.0/PLAN.IGN) | Fond de la vue Carte, calques par catégorie, bâtiments 3D (hauteur et altitude au sol des bâtiments) | Multi-échelle | Tuiles vectorielles PLAN.IGN (TMS, niveaux 0 à 18) et style officiel « standard » de l'IGN, embarqué dans l'application | Sans ombrage ; Plan IGN de base gardé sur le disque (CAR-2), cache plafonné au-delà (CAR-1) ; style converti à la volée pour walkers ; les tuiles les plus lourdes, servies à une résolution de 2 048 ou 1 024 au lieu de 4 096 (ex. l'Europe au niveau 3), sont remises à 4 096, que walkers seul sait lire |
| [Géocodage Géoplateforme](https://www.data.gouv.fr/dataservices/api-geoplateforme-geocodage/discussions) | Recherche de lieu | — | REST `data.geopf.fr/geocodage` | Remplace l'API Adresse historique |

Sources : [Référentiel à grande échelle (RGE) sur data.gouv.fr](https://www.data.gouv.fr/datasets/58e5842688ee386c65805755) · [Documentation des géoservices IGN](https://geoservices.ign.fr/usages)

## Exigences non fonctionnelles

Le relief étant local, la construction du bloc ne dépend pas du réseau ; le téléchargement, ponctuel, a ses propres exigences.

| Critère | Exigence |
| --- | --- |
| Résolution | Toujours 5 m : au plus 2 000 × 2 000 points pour un cadre de 10 km |
| Bascule de vue | Passage Carte ↔ 3D immédiat une fois le bloc construit |
| Fluidité | Manipulation 3D fluide sur GPU intégré récent ; démarrage rapide de l'app ; carte fluide même quand la Géoplateforme est lente (mesuré à \~100 Ko/s le 26 novembre 2026) |
| Téléchargement | Taille annoncée avant accord ; reprise après coupure ; conversion des fichiers ASC en un format interne rapide à lire, puis suppression de l'archive |
| Hors connexion | Relief téléchargé, fond de carte et tuiles des calques déjà vues, en cache, utilisables sans réseau |
| Respect des services | Limites d'usage de la Géoplateforme respectées ([CGU](https://cartes.gouv.fr/cgu/)) : au plus 6 tuiles en parallèle par calque (TMS : 400 requêtes/s), recherche après 300 ms sans frappe (autocomplétion : 10 requêtes/s), 1,5 s au moins entre deux requêtes au service de téléchargement (5 requêtes/5 s, puis 5 minutes de blocage) ; User-Agent identifiant l'app |
| Plateformes | Windows, macOS (Apple Silicon et Intel), Linux |
| Prise en main | Premier bloc obtenu sans aide ; interface en français par défaut, en anglais au choix ; textes traduits par une table, le français servant de clé |
| Distribution | Binaires signés : notarisation Apple (compte développeur payant), signature Windows ; AppImage ou Flatpak sous Linux |
| Licence du code | GPL-3.0 |
| Technologie | Rust ; interface egui (eframe), vue Carte avec walkers (tuiles IGN en Web Mercator, style vectoriel MapLibre), vue 3D en wgpu |

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
| Géoplateforme lente ou saturée | Carte vide, attente | Cache disque, base France pré-téléchargée, priorité aux tuiles visibles (CAR-1 à CAR-3) |
| Empaquetage et signature sur 3 systèmes | Alertes de sécurité à l'installation | Intégration continue dédiée, budget pour les certificats |

## Feuille de route

Sans échéance imposée, le projet avance par jalons ; chacun se termine par un critère vérifiable avant de passer au suivant.

1. **Prototype de faisabilité** : télécharger et convertir le RGE ALTI 5 m d'un département, construire et afficher un bloc de 10 km.
   - Critère : taille et durée du téléchargement mesurées ; bloc affiché depuis le disque.
2. **Version minimale** : vue Carte avec recherche et cadre, proposition de téléchargement, vue 3D avec socle, projection axonométrique, orientation et widget d'axes, export PNG à fond transparent.
   - Critère : un testeur non spécialiste obtient et exporte un bloc seul.
3. **Calques et écran partagé** : d'abord la pile de calques (photo aérienne et Plan IGN vectoriel par catégories) sur la carte, reprise telle quelle par le bloc ; puis bâtiments en 3D, styles, carte et 3D côte à côte.
   - Critère : un changement de couche sur la carte apparaît sur le bloc sans reconstruction complète.
4. **Hors connexion et imports** : écran de gestion du stockage, imports de fichiers.
   - Critère : un bloc complet se reconstruit sans réseau.
5. **Version 1.0** : binaires signés pour les 3 systèmes, publication du code sous GPL-3.0.
   - Critère : installation sans alerte bloquante sur Windows, macOS et Linux.
