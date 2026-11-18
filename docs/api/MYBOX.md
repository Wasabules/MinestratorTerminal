# MyBox : ressources, création et gestion des serveurs

Analyse du 2026-11-18. Sources, citées point par point :

- **[doc]** : la documentation officielle (`openapi-public-fr.yaml`, résumée dans
  [`openapi-resume.txt`](./openapi-resume.txt)) ;
- **[panel]** : le JavaScript public du panel web (page « Serveurs » d'une MyBox, assistant de
  création, SmartScan, fiche d'un serveur ; dictionnaires de traduction fr/en) ;
- **[lu]** : des lectures `GET` réelles avec une clé API, sur un compte de test (une MyBox 32 à
  9 serveurs dont 6 en pause, une MyBox Free). **Aucune écriture n'a été envoyée** : tout ce qui
  concerne les écritures vient de la doc et du panel, et reste à confirmer à l'usage.

L'intégration correspondante est dans `crates/minestrator-core/src/mybox.rs` ; la référence
générale des endpoints dans [`REFERENCE.md`](./REFERENCE.md).

## 1. Modèle de ressources

| Ressource | Unité | Niveau | Source |
|---|---|---|---|
| CPU | **cœurs entiers** | alloué **par serveur** | [panel] curseur `min 1`, pas de 1, libellé « cœurs » ; [lu] 4 cœurs alloués → `cpu.dedicated: 400` dans `/server/{id}/live/light` (100 = un cœur) |
| RAM | **Go entiers** | alloué **par serveur** | [panel] curseur `min 1`, « Go » ; [lu] 10 Go alloués → `memory.limit: 10000` (Mo) |
| Disque | Go (total), Mo (occupation) | **partagé** par la MyBox | [lu] chaque serveur de la MyBox 32 a `disk.limit: 300500` Mo = les 300 Go de l'offre (+ 500 Mo) ; aucun champ disque à la création ni au redimensionnement [doc] |
| Emplacements (`pods`) | serveurs | MyBox | [panel] création possible si `serveurs actifs < offer.pods` ; affichage « actifs N / pods » et « en pause N / ⌊pods × 1,5⌋ » ; libellés « Jusqu'à N serveurs », « Trop de serveurs en sauvegarde pour cette offre » |
| FlexCore | cœurs | MyBox, non allouable | [lu] `offer.flexcore: 2` → chaque serveur a `cpu.flexcore: 200` et `cpu.limit = dédié + 200` ; un serveur sans FlexCore (MyBox Free) a `limit = dédié` |

Points clés :

- **Seuls les serveurs actifs consomment** CPU et RAM. [lu] MyBox 32 : 3 serveurs actifs à
  8 cœurs / 20 Go, 6 en pause à 13 cœurs / 51 Go ; l'API annonce `cpu_remaining: 0` et
  `ram_remaining: 12`, soit exactement `32 − 20`. [panel] « Les ressources qu'il occupe sont
  rendues à votre MyBox » (mise en pause).
- **Total = offre + options payantes.** [panel] le total est `offer.cpu/ram/disk` augmenté des
  options `CPU_2`, `RAM_2`, `DISK_SPACE_25` (champ `amount`) listées dans `GET /mybox/{id}`
  (`options[]`). `offer.disk_total` [doc] semble déjà inclure les options disque (égal à `disk`
  sans option) : l'app prend le maximum des deux. *Incertain* : forme exacte de `options[]` (vide
  sur le compte de test).
- **Reste** : l'API le donne (`mybox.cpu_remaining`, `mybox.ram_remaining` [doc]) ; l'app le
  recalcule (total − actifs) et garde **le plus petit** des deux, pour ne jamais promettre plus que
  ce que l'API acceptera.
- **Disque occupé** : `GET /mybox/{id}/disk` → `{ disk_servers: 3768, disk_servers_unit: "MB" }`
  [lu], affiché par le panel en Go (÷ 1000) face au total de l'offre. `GET /server/{id}/disk/total`
  donne l'occupation d'un serveur (`total_bytes`, `base_game_bytes`, `overhead_applied`) [lu].
- **Pods** : `offer.pods` est le nombre maximal de serveurs **actifs** (8 sur la MyBox 32,
  1 sur la Free) — et non un nombre d'IP dédiées, comme l'app le supposait jusqu'ici
  (`MyBoxOffer.max_servers`, corrigé). Le compte de test a 9 serveurs pour 8 pods : la limite ne
  porte que sur les actifs. Maximum en pause : `⌊pods × 1,5⌋` [panel], erreur
  `API_MYBOX_DISABLED_SERVERS_COUNT_MAX`.
- **`offer.flexcpu`** existe aussi [lu] (2, comme `flexcore`) ; sens non établi, l'app ne s'en sert pas.
- **MyBox Pro** (`mybox.is_pro`) : ressources « sur mesure ». [panel] l'assistant ne borne alors
  ni CPU ni RAM par la MyBox (plafond : deux fois la recommandation du jeu, 1 cœur et 4 Go par
  défaut), ne demande pas de localisation, et masque les compteurs d'emplacements. *Incertain* :
  aucune MyBox Pro sur le compte de test ; l'app reproduit le comportement du panel.
- **MyBox Free** : 1 cœur, 4 Go, 8 Go, 1 pod [lu] ; seule la localisation Toulouse est proposée
  [panel] (« Cette localisation n'est pas disponible pour les offres gratuites »).

## 2. Création d'un serveur — `POST /mybox/{id}/server` [doc]

Corps envoyé par le panel :

```json
{
  "name": "Mon serveur Minecraft",
  "cpu": 2,
  "ram": 4,
  "location": 1,
  "id_egg": 1,
  "required_fields": [{ "key": "STEAM_USER", "value": "…" }]
}
```

- `id_egg` **ou** `id_egg_experimental` (catalogue expérimental), jamais les deux
  (`API_EMPTY_EGG_OR_EXPERIMENTAL_EGG_ID` si aucun).
- `location` : identifiant de datacenter, **requis** [doc] sauf MyBox Pro [panel].
- `required_fields` : seulement si le jeu en déclare ; `key` = nom de la variable d'environnement
  de l'egg (`STEAM_USER`, `AUTHKEY`, `OWNER_ID`…). Tous doivent être non vides [panel]. Certains
  sont des secrets (mot de passe Steam) : l'app ne les journalise jamais.
- `force_settings` [doc] : le panel l'envoie (`1`) uniquement pour un premier serveur créé hors de
  l'assistant d'accueil. *Sens incertain* (application forcée des réglages de l'egg ?) : l'app ne
  l'envoie pas.
- `location_context` : télémétrie de ping du panel, ignorée par l'app.
- Réponse [doc] : `{ id_mybox, id_server, ip }`. Le panel attend ensuite ~15 s puis ouvre le
  serveur : l'installation se poursuit côté hébergeur (statut `installing` puis `offline`).
- Bornes vérifiées par le panel **avant** envoi, et reprises par l'app : `cpu ≥ 1`, `ram ≥ 1`,
  `actifs + nouveau ≤ total` (CPU et RAM), `actifs < pods`, champs requis remplis, localisation
  choisie et **en stock** (`in_stock`, sinon « rupture de stock »). Les préréglages du panel :
  minimum = 1 cœur / 2 Go, recommandé = ressources de l'egg, maximum = tout le reste.
- Nom : 1 à 50 caractères (`maxlength="50"` du champ du panel) ; erreurs `API_NAME_TOO_SHORT`,
  `API_NAME_TOO_LONG`, `API_MISSING_INCORRECT_NAME`. Le panel propose « Mon serveur <jeu> ».
- Erreurs possibles [panel, traductions] : `API_MYBOX_NOT_ENOUGH_CPU`, `API_MYBOX_NOT_ENOUGH_RAM`,
  `API_MYBOX_FULL`, `API_MYBOX_EXPIRED`, `API_MYBOX_SUSPENDED`, `API_MYBOX_BUSY`,
  `API_INCORRECT_CPU_AMOUNT`, `API_INCORRECT_RAM_AMOUNT`, `API_INVALID_LOCATION`,
  `API_LOCATION_UNAVAILABLE_REGION` (« réservée aux joueurs proches du datacenter »),
  `API_LOCATION_UNAVAILABLE_VPN`, `API_MISSING_REQUIRED_FIELDS`, `API_MYBOX_OUT_OF_STOCK`.

### Catalogues

| Endpoint | Contenu | Usage dans l'app |
|---|---|---|
| `GET /server/catalog/eggs` [doc] | `eggs[{ id, name, required_fields[{ key, label }] }]` — 43 jeux [lu] | non utilisé (mêmes 43 identifiants que `/site/eggs` [lu], sans les ressources) |
| `GET /site/eggs` [panel] | mêmes jeux + `icon`, `resources{ cpu, ram, disk, disk_install }` (chaînes, cœurs et Go), `certification` (1 à 3), `required_fields[{ name, label, label_description, translations }]`, `eggs_categories[{ category, eggs }]` (`TYPE_PODS_VIDEOGAMES` 36, `TYPE_PODS_PROGRAMMING` 4, `TYPE_PODS_DATABASES` 3) [lu] | catalogue principal |
| `GET /site/eggs/experimentals` [panel] | `eggs.{APPLICATION, VIDEO_GAME, GENERIC}[{ id, name, fields[{ name, description }] }]` — 209 types [lu] ; non testés par l'hébergeur, sans support garanti | catégorie `experimental`, envoyés en `id_egg_experimental` |
| `GET /user/{id}/pimpmybox` [panel] | `eggs.validated[{ id, name, required_fields }]` : types personnalisés validés (PimpMyBox™) | catégorie `custom`, envoyés en `id_egg` |
| `GET /server/catalog/locations` [doc] | `locations[{ id, city, country, country_iso, in_stock }]` — 7 localisations [lu] (`in_stock` absent de la doc) | localisations proposées |
| `GET /site/datacenters?context=creation` [panel] | mêmes 7 + `geo_masked`, `ping` (URL de test de latence) ; sans `context`, 10 dont 3 masquées | non utilisé |

## 3. Redimensionnement — `PATCH /mybox/{id}/server` [doc]

- Corps `{ id_server, cpu, ram }` (valeurs **finales**, toutes requises [doc]).
- [panel] contrôle avant envoi : `somme des autres serveurs actifs + nouvelle valeur ≤ total`,
  pour CPU et RAM ; pour un serveur en pause aussi (« les ressources ne seront réellement occupées
  qu'à sa reprise »).
- **Serveur arrêté** : le panel tente l'envoi ; sur `API_MYBOX_NEED_OFFLINE_SERVER`, il propose
  d'arrêter le serveur, attend l'état `offline`, puis renvoie la même requête. L'app reproduit ce
  comportement (l'arrêt reste une action explicite de l'utilisateur). *Incertain* : quelles
  modifications exigent l'arrêt (toutes, ou seulement les baisses).

## 4. Pause et reprise — `PATCH /mybox/{id}/server/disable|enable` [doc]

- Corps `{ id_server }`. Vocabulaire du panel : « Mettre en pause » / « Reprendre » (l'API dit
  `disabled`).
- En pause : données conservées, ressources rendues à la MyBox, reprise possible à tout moment
  [panel]. [lu] un serveur en pause est **aussi** signalé `is_suspended: 1` par l'API.
- Reprise : il faut un emplacement actif libre (`API_MYBOX_FULL`) et ses ressources disponibles
  (`API_MYBOX_NOT_ENOUGH_CPU/RAM`). La réponse peut porter
  `reactivation: { status: "in_progress", job_id }` : les fichiers sont remis en place
  (« peut prendre quelques minutes »), suivi via `GET /user/{id}/backup_job/{job_id}`. Refus
  possibles : `API_SERVER_REACTIVATION_RETRY` (machine injoignable), `API_SERVER_REACTIVATION_SUPPORT`
  (intervention du support, `data.support_code`).
- Autres erreurs : `API_MYBOX_SERVER_ALREADY_DISABLED`, `API_MYBOX_SERVER_ALREADY_ENABLED`,
  `API_MYBOX_DISABLED_SERVERS_COUNT_MAX`.

## 5. Suppression — `DELETE /mybox/{id}/server/{id}` [doc]

- Immédiate et **irréversible** : « fichiers, configurations et maps supprimés ; sauvegardes
  automatiques perdues ; aucune récupération possible » [panel].
- Confirmation du panel : saisir le **code support** du serveur, puis un compte à rebours de 10 s.
  L'app demande de saisir le **nom exact** du serveur (et l'outil MCP exige `confirm_name`).
- **Snapshot avant suppression** — `POST /mybox/{id}/server/{id}/snapshot-before-delete` (sans
  corps) [panel] → `{ job_id, snapshot_name, current_count, limit }` ; suivi par
  `GET /user/{id}/backup_job/{job_id}` toutes les 3 s → `{ status, progress, … }` jusqu'à
  `completed` (ou `failed`, `cancelled`). Erreurs : `API_SNAPSHOTS_COUNT_MAX` (limite de 20
  snapshots par compte, `current_count`/`limit` dans `data`), `API_SNAPSHOT_ALREADY_PROCESSING`.
  Proposée seulement si la MyBox n'est ni gratuite, ni expirée, ni suspendue, et que le serveur
  est sain ou en pause [panel]. La snapshot reste restaurable sur un autre serveur.
- `POST/DELETE /mybox/{id}/server/{id}/dedicated_ip` : option d'IP dédiée **payante** (hors
  périmètre).

## 6. Renommage et ordre

- `PATCH /server/{id}/name` `{ name }` [doc] ; `PATCH /mybox/{id}/name` `{ name }` [panel].
  1 à 50 caractères. Envoyer un nom vide ou le nom par défaut revient au nom par défaut
  (« Mon serveur <jeu> » ; pour la MyBox, son nom d'offre) [panel] — l'app exige un nom non vide.
- `PATCH /mybox/{id}/servers/order` `{ servers_ids: [...] }` [panel] : la liste **complète** des
  serveurs de la MyBox (actifs dans l'ordre voulu, puis en pause). Le tableau de bord envoie aussi
  la liste complète (ordre affiché, puis les autres). Ordre lu dans `servers[].order` [lu].
- Groupes du tableau de bord (indépendants des MyBox, cosmétiques) [panel] :
  `POST /user/{id}/servers_groups` `{ name }` (≤ 40 caractères), `PATCH` `{ id_group, name }`,
  `DELETE /user/{id}/servers_groups/{id}`, `PATCH /user/{id}/servers_groups/assign`
  `{ assignments: [{ id_server, id_group }] }`. Non intégrés.

## 7. SmartScan — `GET /mybox/{id}/smartscan` [panel, lu]

Analyse de l'hébergeur sur l'usage réel (p95 sur 14 à 30 jours, indice de confiance) :

- `shrink_recommendation` (SmartShrink™) : serveurs surdimensionnés, `suggested_cpu/ram`,
  économies possibles ;
- `balance_recommendation` (SmartBalance™) : redistribution entre serveurs (`reallocation_plan`,
  `target_cpu/ram`, `reason`) ;
- `scale_recommendation` (SmartScale™) : conseil d'évolution (`optimize_first`, upgrade
  MyBoxPerf/Pro…) — commercial, l'app ne l'expose qu'en lecture brute ;
- `servers_analysis`, `statistics`, et un bloc `mybox` (totaux, allocations, `is_dedicated`).

Unités [panel, lu] : `current/suggested/target_cpu` en cœurs, `*_ram` en Go, `p95_ram_mb` en Mo,
`p95_cpu_percent` en % d'un cœur. Appliquer : `PATCH /mybox/{id}/smartscan`
`{ action: "shrink" | "balance", servers?: [ids] }` — serveurs concernés **arrêtés**
(`API_MYBOX_NEED_OFFLINE_SERVER`), erreurs `API_INVALID_SERVER_SELECTION`,
`API_NO_OPTIMIZATION_NEEDED`. L'app n'expose que la lecture (outil MCP `mybox_smartscan`) ;
l'application d'une suggestion passe par un redimensionnement explicite.

## 8. Cycle de vie d'un serveur

`POST /mybox/{id}/server` → installation par l'hébergeur (statut `installing`) → `offline` →
démarrage manuel. Pause (`disable`) → données gardées, ressources rendues → reprise (`enable`,
éventuellement asynchrone). Expiration de la MyBox → serveurs inaccessibles, puis suppression
après `deletion_days` [doc]. Suppression (`DELETE`) → définitive.

## 9. Hors périmètre (payant ou commercial)

Achat, renouvellement, changement d'offre (`/mybox/subscription`, `/mybox/downgrade`,
`/mybox/type/downgrade`, `/mybox/{id}/free/renew`), options de ressources
(`POST/DELETE /mybox/{id}/option`, catalogue `GET /site/mybox/options` : `CPU_2`, `RAM_2`,
`DISK_SPACE_25`), IP dédiées (`/dedicated_ip`), remboursement (`/refund`), transfert de propriété
(`/owner`), avis (`/review`), suppression de la MyBox elle-même (`DELETE /mybox/{id}`). Aucun n'est
intégré.

## 10. Ce que l'app intègre

| Fonction | Endpoint | Cœur (`mybox.rs`) | MCP |
|---|---|---|---|
| Capacité de toutes les MyBox | `GET /mybox/{id}` + `/disk` | `mybox_capacities`, `mybox_capacity` | `mybox_capacity` |
| Catalogue (jeux, champs, localisations) | `/site/eggs`, `/site/eggs/experimentals`, `/user/{id}/pimpmybox`, `/server/catalog/locations` | `server_catalog` (cache 1 h) | `server_catalog` |
| Plan de placement (pur) | — | `plan_placement`, `mybox_plan_placement` | `plan_placement` |
| SmartScan (lecture) | `GET /mybox/{id}/smartscan` | `mybox_smartscan` | `mybox_smartscan` |
| Créer | `POST /mybox/{id}/server` | `mybox_create_server` | `mybox_create_server` ✎ |
| Redimensionner | `PATCH /mybox/{id}/server` | `mybox_resize_server` | `mybox_resize_server` ✎ |
| Pause / reprise | `PATCH …/server/disable·enable` | `mybox_set_server_paused` | `mybox_pause_server` ✎ |
| Ordre | `PATCH /mybox/{id}/servers/order` | `mybox_reorder_servers` | `mybox_reorder_servers` ✎ |
| Renommer | `PATCH /server/{id}/name`, `/mybox/{id}/name` | `rename_server`, `rename_mybox` | idem ✎ |
| Snapshot avant suppression | `POST …/snapshot-before-delete`, `GET /user/{id}/backup_job/{id}` | `mybox_snapshot_before_delete`, `backup_job` | (dans la suppression) |
| Supprimer | `DELETE /mybox/{id}/server/{id}` | `mybox_delete_server`, `mybox_delete_server_guarded` | `mybox_delete_server` ✎ (nom exact) |

Les outils ✎ sont soumis à `allow_writes` et **jamais** proposés au Copilote (`INFRA_TOOLS`) : il
lit le chat des joueurs, une injection ne doit pas pouvoir créer ou supprimer de serveurs.

## 11. Incertitudes à lever à l'usage

1. Forme exacte de `options[]` dans `GET /mybox/{id}` (vide sur le compte de test).
2. Comportement d'une MyBox Pro (pas d'exemplaire sur le compte de test).
3. Sens de `force_settings` et de `offer.flexcpu`.
4. Le redimensionnement exige-t-il toujours le serveur arrêté ?
5. Format de `GET /user/{id}/backup_job/{id}` (lu par le panel, jamais appelé avec une clé API).
6. Un sous-utilisateur : forme de `mybox.perms` (`perms.mybox[]` lu par le panel : `server_create`,
   `server_update`…), absente pour le propriétaire.
