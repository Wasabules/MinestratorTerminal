# Minestrator Terminal — Charte graphique

> Extraite du design system réel de **minestrator.com** (Nuxt UI / Tailwind, variables `--ui-*` du CSS `entry.*.css`, relevé 2026-07-04).
> Objectif : que l'app desktop soit visuellement cohérente avec le panel. **Dark-first** (idéal pour une app orientée console).

## 1. Couleurs de marque (rôles sémantiques)

| Rôle | Hex | Usage |
|---|---|---|
| **Primary** | `#009b72` | Couleur principale : actions positives, accents, liens, jauges « OK ». |
| Primary (dark) | `#005d44` | Variante foncée pour hover/pressed sur fonds clairs. |
| **Accent / signature** | `#ff715b` | Coral : accent chaud, CTA secondaire, moitié « chaude » du gradient. |
| Strator | `#ffb4a8` | Coral doux : badges, surbrillances discrètes. |
| Secondary | `#cc5740` | Terracotta : alertes douces, catégories. |
| Tertiary | `#7828c8` | Violet : mises en avant premium / catégorisation. |
| Pro | `#394666` | Slate bleuté : offres/éléments « Pro ». |
| Discord | `#5865f2` | Uniquement pour le bouton/lien Discord. |

**Gradient signature** (confirmé `--tw-gradient-from/to`) : `linear-gradient(135deg, #009b72 0%, #ff715b 100%)` — vert → coral. À réserver aux éléments héros (logo, en-tête, écran d'accueil), pas au chrome courant.

## 2. Surfaces & fonds

| Rôle | Dark (défaut) | Light |
|---|---|---|
| Fond principal (`--ui-main-background`) | `#12171a` | `#f9fafb` |
| Surface / carte (`--ui-bg`) | `#181d22` | `#ffffff` |
| Surface élevée (modale, popover) | `#2c3038` | `#ffffff` + ombre |
| Bordure | `#2c3038` / `#394050` | `#e5e7eb` |

## 3. Texte & neutres

| Rôle | Dark | Light |
|---|---|---|
| Texte principal | `#f1f5f9` | `#1a1a1a` |
| Texte secondaire | `#cbd5e1` | `#4b5563` |
| Texte atténué | `#859398` | `#9ca3af` |
| Inversé / sur primaire | `#ffffff` | `#ffffff` |

*(`#cbd5e1` et `#859398` sont présents dans le CSS du site — neutres slate cohérents avec la marque.)*

## 4. Couleurs d'état (mappées console/serveur)

| État serveur (Wings `status`) | Couleur | Base |
|---|---|---|
| `running` | `#009b72` (primary) | vert marque |
| `starting` / `stopping` | `#f59e0b` (amber, vu sur le site) | transition |
| `offline` / stoppé | `#859398` (gris) | neutre |
| `kill` / erreur | `#ff5f57` (rouge, vu sur le site) | danger |
| Hibernation | `#7828c8` (tertiary) | violet = « en veille » |

Jauges ressources : vert `#009b72` < 70 %, amber `#f59e0b` 70–90 %, rouge `#ff5f57` > 90 %.

## 5. Typographie & forme

- **UI** : sans-serif système (Inter / `-apple-system, Segoe UI, Roboto`) — net, dense, lisible.
- **Console** : monospace (`ui-monospace, "Cascadia Code", "JetBrains Mono", Consolas`) — xterm.js.
- Coins arrondis modérés (`6–10px`), ombres discrètes, densité d'information élevée (esprit dashboard/terminal, pas marketing).

## 6. Tokens prêts à coller (SvelteKit `app.css`)

```css
:root {
  /* Marque */
  --brand-primary:      #009b72;
  --brand-primary-700:  #005d44;
  --brand-accent:       #ff715b;
  --brand-strator:      #ffb4a8;
  --brand-secondary:    #cc5740;
  --brand-tertiary:     #7828c8;
  --brand-pro:          #394666;
  --brand-discord:      #5865f2;
  --brand-gradient:     linear-gradient(135deg, #009b72 0%, #ff715b 100%);

  /* États */
  --state-running:  #009b72;
  --state-pending:  #f59e0b;
  --state-offline:  #859398;
  --state-danger:   #ff5f57;
  --state-hibernate:#7828c8;
}

/* Dark (défaut) */
:root, :root[data-theme="dark"] {
  --bg:        #12171a;
  --surface:   #181d22;
  --elevated:  #2c3038;
  --border:    #2c3038;
  --text:      #f1f5f9;
  --text-muted:#cbd5e1;
  --text-dim:  #859398;
}

/* Light */
:root[data-theme="light"] {
  --bg:        #f9fafb;
  --surface:   #ffffff;
  --elevated:  #ffffff;
  --border:    #e5e7eb;
  --text:      #1a1a1a;
  --text-muted:#4b5563;
  --text-dim:  #9ca3af;
}
```

## 7. Note

Les couleurs `#1F52F7` / `#5371B1` renvoyées par l'API (`global_alert`, `event_alert`) sont des **thèmes de campagne promo temporaires** (ex. « StratorGoal »), pas la charte de marque. Ne pas les câbler en dur : si on veut afficher ces bannières, lire dynamiquement `color_bg`/`color_text` depuis la réponse `/user`.
