# Jalon 1 — Prototype de faisabilité

Mesures du 26 novembre 2026, sur une connexion domestique (≈ 0,5–0,7 Mo/s depuis la Géoplateforme).

## Critère

- Taille et durée du téléchargement mesurées : **oui**, voir ci-dessous.
- Bloc affiché depuis le disque : **oui**, bloc de 10 km à 5 m (2001 × 2001 points) en vue 3D avec socle.

## Téléchargement et conversion

| Département | Archive 7z | Dalles | Converti sur disque | Durée totale |
| --- | --- | --- | --- | --- |
| D075 Paris | 14 Mo | 12 | 23 Mo | 100 s |
| D038 Isère | 505 Mo | 383 | 732 Mo | ≈ 15 min |

- Isère : 299 Mo en 620 s, puis **coupure par le serveur** (`HTTP/2 stream reset, CANCEL`). La reprise (requête `Range`) a terminé les 206 Mo restants et la conversion en 274 s.
- La conversion (décompression LZMA + lecture ASC + écriture binaire) a coûté ≈ 30 s de CPU pour Isère : le réseau domine largement.
- Le format interne (u16 en décimètres, sans compression) occupe 1,45 × la taille de l'archive.

## Construction du bloc

| Bloc | Lecture disque | Envoi GPU | Altitudes |
| --- | --- | --- | --- |
| Paris (2,35 E ; 48,85 N) | 0,02 s | — | 22–131 m |
| Belledonne (5,95 E ; 45,15 N) | 0,04 s | 0,10 s | 616–2939 m |

La bascule vers la 3D est donc immédiate une fois le relief installé.

## Constats pour la suite

1. **Coupures serveur** : les transferts longs sont interrompus (≈ 10 min ici). Le téléchargement reprend désormais automatiquement ; il faut le garder pour tous les fichiers volumineux.
2. **Durée** : à ce débit, un grand département prend 10 à 15 min. L'annonce de la taille avant accord et le téléchargement en tâche de fond sont indispensables.
3. **Grille IGN** : les dalles sont décalées de 5 m vers le nord par rapport aux kilomètres ronds ; le lecteur indexe donc les dalles présentes sur le disque au lieu de deviner leur nom.
4. **Bord de département** : les dalles de bord contiennent des données au-delà de la limite (bloc parisien de 10 km sans aucun trou). Le raccord entre départements reste à tester sur un cadre à cheval.
5. **Espace disque** : 732 Mo pour l'Isère. Une compression des dalles (ex. zstd) est à envisager si l'espace devient un frein.

## Reproduire

```sh
nix develop
cargo run --release -- installer D038        # télécharge, convertit, mesure
cargo run --release -- bloc 5.95 45.15       # lit un bloc de 10 km centré sur lon/lat
cargo run --release                          # application : tracer un cadre, télécharger, 3D
```

Données stockées dans `~/.local/share/blocs/relief/<département>/`.
