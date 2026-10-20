# LightCraft en français

Choisir **Édition ▸ Langue ▸ Français** ou **Réglages ▸ Général ▸ Langue**. Le changement s'applique
tout de suite et se conserve au prochain lancement (`language: "fr"` dans `ui.json`). **Au premier
lancement, LightCraft prend la langue du système** quand elle est dans la table (un Mac en français
s'ouvre en français) ; `LIGHTCRAFT_LANGUAGE` accepte toute variante (`fr`, `fr-FR`, `fr_CA.UTF-8`,
`fr-BE`, `fr-CH`) et passe avant le système, pour les captures sans fenêtre et les tests. Les tests
de la crate démarrent toujours en anglais, quelle que soit la machine.

Le catalogue français (`crates/ui-egui/locales/fr.json`, 2 100 messages ; `fr-formats.json`, 229
messages à valeurs) couvre les menus, les raccourcis, la bibliothèque, le développement, le recadrage,
les masques, l'import et l'export, les réglages, les préréglages et profils intégrés, l'historique,
les dates, les messages d'avancement et les notes de version, **ainsi que les panneaux ajoutés dans
ce fork** : espaces de travail (Bibliothèque · Retoucher · Créer), barre de contexte, accueil
d'import, Tri rapide, Classer, Aide au tri, Personnes / Reconnaissance, Lieux, Propositions,
Développement par tâche, palette de commandes, Vidéos (liste, moments, descriptions, recherche « qui + quoi »). Comme dans les autres langues, les erreurs techniques
des couches basses restent en anglais, et un texte absent du catalogue s'affiche en anglais.

Les noms de fichiers, les identifiants de commandes, les gabarits de renommage et les noms propres
(albums, préréglages importés, métadonnées) ne se traduisent pas. La police de l'interface, Inter,
couvre tous les caractères du français ; aucune police supplémentaire n'est nécessaire.

## Terminologie

Vocabulaire des photographes francophones (Lightroom en français, Capture One), à garder cohérent :

| Anglais | Français |
|---|---|
| Library / Retouch / Create (espaces) | Bibliothèque / Retoucher / Créer |
| Import / Sort / Classify (étapes) | Importer / Trier / Classer |
| Edit (menu) · Develop (panneau) | Édition · Développement |
| Presets / Preset / Profile | Préréglages / Préréglage / Profil |
| Crop & Rotate · Remove · Masking · Red Eye | Recadrer et redresser · Supprimer · Masquage · Yeux rouges |
| Heal / Clone / Dust spots | Corriger / Dupliquer / Poussières |
| Light / Color / Effects / Detail / Optics / Calibration | Lumière / Couleur / Effets / Détails / Optique / Calibrage |
| Highlights / Shadows / Whites / Blacks | Hautes lumières / Ombres / Blancs / Noirs |
| Texture / Clarity / Dehaze / Vibrance | Texture / Clarté / Correction du voile / Vibrance |
| Temp / Tint / As Shot | Température / Teinte / Telle quelle |
| Sharpening / Noise Reduction / Denoise | Netteté / Réduction du bruit / Débruitage |
| Tone Curve / Color Mixer / Color Grading / Point Color | Courbe des tonalités / Mélangeur de couleurs / Étalonnage des couleurs / Couleur ponctuelle |
| Filmstrip / Stack / Burst / Virtual copy | Pellicule / Pile / Rafale / Copie virtuelle |
| Album / Smart Album / Folder | Album / Album dynamique / Dossier |
| Pick / Reject / Unflag / Rating / Label | Retenue / Rejetée / Sans marqueur / Note / Libellé |
| Culling / Sorting Help / Quick sort | Tri / Aide au tri / Tri rapide |
| Soft / Closed eyes / Same file | Floue / Yeux fermés / Même fichier |
| People / Faces / Recognition / Unknown person | Personnes / Visages / Reconnaissance / Personne inconnue |
| Videos / Moment / Scene / Action / Description | Vidéos / Moment / Scène / Action / Description |
| Proposed / Confirmed / Confirm / Propose descriptions | Proposé / Confirmé / Confirmer / Proposer des descriptions |
| Cut pictures / Find faces / Play at | Extraire des images / Trouver les visages / Lire à |
| Places / Geotag | Lieux / Géolocaliser |
| Proposals / Propose / Natural skin / Light, Medium, Strong | Propositions / Proposer / Peau naturelle / Léger, Moyen, Fort |
| Web Gallery / Slideshow / Kit / Contact sheet | Galerie web / Diaporama / Kit / Planche contact |
| Before / After · Versions · History | Avant / Après · Versions · Historique |
| Settings · Workspaces · Command Palette | Réglages · Espaces de travail · Palette de commandes |

Typographie : espace insécable (U+00A0) avant `:` `;` `?` `!` et à l'intérieur des guillemets « ».
Boutons à l'infinitif ou au nominal (« Importer », « Réinitialiser l'espace »), phrases d'aide à
l'indicatif, casse de phrase.

## Maintenance et vérification

Textes fixes : `crates/ui-egui/locales/fr.json`. Messages avec valeurs : `fr-formats.json` (chaque
message y est obligatoire, le build le vérifie dans toutes les langues ; un suffixe de pluriel anglais
inutile se consomme avec `{:.0}`). Contrôles :

```sh
cargo test -p lightcraft-ui-egui i18n::tests                 # catalogues, clés, placeholders
cargo test -p lightcraft-ui-egui tests_v14::french           # couverture de nos panneaux, vocabulaire, langue système
LIGHTCRAFT_LANGUAGE=fr lightcraft-cli snapshot --demo -o fr.png --size 1600x1000
```

`tests_v14::french_covers_every_string_of_our_panels` lit les sources de nos panneaux : un libellé
passé à un bouton, une étiquette, une infobulle ou un toast **sans** `tr` / `tr_format!` fait échouer
le test, de même qu'une clé `tr("…")` sans ligne française. Les corrections de locuteurs natifs sont
bienvenues : ouvrir un issue avec le texte anglais, la traduction actuelle et la proposition.
