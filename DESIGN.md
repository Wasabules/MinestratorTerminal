# Minestrator Terminal — Charte graphique « Biome »

> Direction artistique de l'app **desktop** (le mobile suivra). Source des valeurs :
> `desktop/src/app.css` (tokens et primitives) et `desktop/src/lib/servers/colors.svelte.ts`
> (biomes). Ce document en donne les règles ; en cas de doute, le code fait foi.

## 1. Intention

Une app d'admin Minecraft, pas une « app IA » : pas de colonne centrée sur fond vide, pas de
dégradés, pas de cartes molles. Biome reprend l'esprit MineStrator (Nunito, vert et corail,
Minecraft) avec une signature propre : **chaque serveur a un biome** (forêt, océan, désert,
neige, Nether, End). Son biome colore ses onglets et l'en-tête de son tableau de bord, où un
**paysage en pixels** dessiné par l'app sert de bannière : on sait sur quel serveur on est d'un
coup d'œil.

L'interface reste **claire, aérée et pratique** : la couleur sert à se repérer et à signaler un
état, jamais à décorer.

## 2. Couleurs (thème sombre, par défaut)

### Surfaces

| Token | Valeur | Rôle |
|---|---|---|
| `--chrome` | `#0d1113` | Barre d'onglets, fonds de console et d'éditeur |
| `--bg` | `#14191d` | Page (et onglet actif, qui se fond dans la page) |
| `--surface` | `#1b2126` | Carte |
| `--inset` | `#161b1f` | Encart, champ, onglet inactif, bandeau d'infos |
| `--elevated` | `#20272d` | Menu, popover, dialogue |
| `--border` | `#232b31` | Bordures |
| `--border-strong` | `#262e35` | Pistes de jauge, grilles des graphes |

### Texte

| Token | Valeur | Contraste sur `--surface` |
|---|---|---|
| `--text` | `#f1f5f3` | 14:1 |
| `--text-muted` | `#b3bfba` | 8,6:1 |
| `--text-dim` | `#8c9a94` | 5,5:1 |

### États

| Token | Valeur | Usage |
|---|---|---|
| `--state-running` | `#5fd68c` | En ligne (pastille) — texte : `--state-running-text` `#8fe8b0` |
| `--state-pending` | `#f2b84b` | Démarrage, arrêt, mises à jour |
| `--state-offline` | `#6b7a7f` | Arrêté — **pastille seulement** (3,7:1) ; le libellé prend `--text-dim` |
| `--state-danger` | `#ff6b63` | Erreur, crash |
| `--state-hibernate` | `#c79bff` | Veille, hibernation, staff |

`status.ts` renvoie pour chaque état une couleur de pastille (`color`) et une couleur de
libellé lisible (`ink`).

### Boutons

| Rôle | Fond | Texte | Classe |
|---|---|---|---|
| Principal | `#f1f5f3` | `#0d1113`, gras 900 | `.btn` |
| Secondaire | `#0d1113` | `#f1f5f3`, gras 800 | `.btn .btn--ghost` |
| Danger | `#3a1517` | `#ff9d97` | `.btn .btn--danger` |

Le principal est **clair** : il ressort sur la page comme sur tous les paysages. Rayon 10 px.

### Accents

- **Corail `#ff715b`** : réservé au curseur du logo. Nulle part ailleurs.
- **Vert de marque `#5fd68c`** (`--brand-primary`) : liens, icônes actives, anneau de focus ;
  texte posé dessus : `--on-primary`.
- **Accent de sélection** (`--accent`, `--on-accent`) : période choisie, onglet actif, segment
  sélectionné. Par défaut le vert de la forêt ; dans une vue serveur, l'accent de **son biome**.
  Fond teinté du biome : `--accent-soft` (tuiles d'icône, boutons d'action du biome), texte
  posé dessus : `--accent-text`.

### Graphes

`--chart-cpu` `#3fbf6e` (trait plein = surveillance de l'app, pointillés = mesures de
l'hébergeur), `--chart-ram` `#ff8a75`, `--chart-players` `#c79bff`.

### Thème clair

`:root[data-theme="light"]` redéfinit les mêmes tokens (page `#f3f6f4`, carte blanche, texte
`#0d1113`, discret `#5d6a65` à 5,2:1 sur la page, états assombris pour rester lisibles en
texte). Les boutons s'inversent : principal sombre, secondaire clair. Les accents de biome
passent à leur variante foncée (`accentLight`, ≥ 4,5:1 sur blanc, texte blanc dessus).

Ce qui reste sombre dans les deux thèmes :

- les **paysages** et ce qui est posé dessus (nom, puces, boutons de la bannière) ;
- la **console** et l'**éditeur de fichiers**, marqués `.force-dark` : la classe redéclare les
  tokens du thème sombre pour tout ce qu'elle contient ;
- la tuile du logo.

## 3. Biomes

Six palettes, dans `colors.svelte.ts` (`BIOMES`) :

| Biome | Accent | Ciel | Collines lointaines | Collines proches | Décor |
|---|---|---|---|---|---|
| Forêt | `#3fbf6e` | `#13261d` | `#1c3d2c` | `#235237` | arbres : tronc `#5a3b22`, feuillage `#2f8a4f`, cime `#46b26a` |
| Océan | `#4aa3ff` | `#0f2133` | `#173755` | `#1d4e7a` | mer, écume, îles au loin |
| Désert | `#e0b34f` | `#2a2212` | `#4a3a1c` | `#7a5a1d` | dunes, cactus verts |
| Neige | `#e6f0f7` | `#1a2430` | `#3a4a5c` | `#8fa3b8` | montagnes, sapins à cime blanche |
| Nether | `#ff6b5b` | `#2a0f10` | `#4a1a1a` | `#7a2626` | relief déchiqueté, touches de lave |
| End | `#c79bff` | `#160f24` | `#2a1d45` | `#3f2a6b` | îles basses, plantes de chorus, étoiles |

Chaque biome porte aussi son encre (`ink`, texte sur l'accent : **sombre**, même sur la neige),
sa variante claire (`accentLight`) et ses fonds teintés (`sky` en sombre, `softLight` en clair).

API :

- `serverBiome(id)` : la palette du serveur ; `serverColor(id)` : son accent (compatibilité) ;
- `setServerBiome(id, key | null)` : choix (persisté en localStorage), `null` = automatique ;
- `biomeVars(biome)` : variables CSS à poser sur un conteneur de classe `.biome`, qui résout
  `--accent` & co. selon le thème. `ServerPanel` le fait pour toutes les vues d'un serveur ;
- **biome par défaut** : tiré de l'id du serveur (mélange entier), donc stable sans rien
  enregistrer ;
- **migration** : les anciennes couleurs hex (`mnstr-server-colors`) deviennent le biome dont
  l'accent est le plus proche dans l'espace OKLab (teinte privilégiée sur la clarté).

### Paysage en pixels

`BiomeLandscape.svelte` + `servers/landscape.ts`. Une hauteur entière par colonne pour chaque
plan, colonnes de même hauteur fusionnées en rectangles, **un seul chemin SVG par couleur**,
rendu `shape-rendering: crispEdges`. Le relief et le décor sont tirés de l'id du serveur : chaque
serveur a le sien, identique d'une session à l'autre. Les pixels restent carrés (`pixel` px) :
la largeur fixe le nombre de colonnes, le paysage est ancré en bas. Décoratif (`aria-hidden`).

Usages : bannière du tableau de bord (190 px, pixels de 6), cartes de l'accueil (58 px, pixels
de 4), sélecteur de biome (36 px, pixels de 3), écran de connexion.

`BiomeBlock.svelte` : le petit bloc 4×4 de la tête de groupe (accent en haut, sol en bas, comme
un bloc d'herbe).

## 4. Typographie

- **Nunito** (variable, 600 à 900) pour l'interface. Corps en 600, emphase en 800, titres en 900.
- **JetBrains Mono** (variable) pour les chiffres, la console, les adresses, les identifiants.
- Les deux sont **embarquées** (`@fontsource-variable/nunito`, `@fontsource-variable/jetbrains-mono`,
  licence OFL), importées dans `routes/+layout.svelte`. Jamais de Google Fonts : le webview ne
  fait aucune requête réseau. La console attend le chargement de la police avant que xterm
  mesure ses cellules.

Tailles de référence : nom du serveur 48 px, titres de carte 18 px, texte 14 à 15 px, détails
12,5 à 13 px.

## 5. Formes

- Rayons : cartes **16 px**, boutons et champs **10 px**, onglets **12 px en haut**, puces en
  pilule.
- **Pas d'ombres**, sauf ce qui flotte : menus, popovers, dialogues (`--shadow`).
- **Aucun dégradé.** Le logo est le « curseur » : chevron vert puis bloc corail
  (`Logo.svelte`).

## 6. Composants clés

- **Barre d'onglets** (`TabBar.svelte`) : onglets en intercalaires sur `--chrome`. Le bord haut
  (3 px) prend l'accent du biome du serveur ; l'onglet actif prend la couleur de la page. Les
  onglets d'un même serveur, adjacents, sont regroupés derrière une **tête de groupe** : bloc du
  biome (avec la pastille d'état), nom en couleur d'accent, chevron qui ouvre le menu. Accueil,
  MyBox, Réglages, Copilote et Alertes restent neutres (`--tab-neutral`). Mode compact : icônes
  seules, le nom du serveur reste en tête de groupe. Le menu contextuel propose le **choix du
  biome** avec mini-paysages.
- **Tableau de bord** (`ServerOverview.svelte`) : bannière paysage (nom, état, version, adresse
  copiable, alimentation), bandeau d'infos, avis, tuiles « Ouvrir une vue », historique,
  activité ; à droite ressources, joueurs, disponibilité. Pleine largeur.
- **Accueil** : cartes serveur avec une fine bannière paysage, l'icône du jeu à cheval dessus.

## 7. Note

Les couleurs `#1F52F7` / `#5371B1` renvoyées par l'API (`global_alert`, `event_alert`) sont des
**thèmes de campagne promo temporaires**, pas la charte. Ne pas les câbler en dur : si on affiche
ces bannières, lire `color_bg`/`color_text` depuis la réponse `/user`.
