#!/usr/bin/env python3
"""Génère `docs/api/REFERENCE.md` : référence complète de l'API MineStrator, documentée ou non.

Croise trois sources versionnées dans `docs/api/` :
- `openapi-resume.txt` : endpoints de la documentation officielle (openapi_resume.py) ;
- `endpoints-panel.txt` : endpoints appelés par le panel web (relevé du 2026-11-18) ;
- `acces-cle-api.json` : accès avec une clé API et structure des réponses (lectures testées).

Usage : `python3 scripts/api-watch/reference.py` (réécrit docs/api/REFERENCE.md).
"""
import json
import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parents[2]
API = ROOT / "docs" / "api"
METHOD_ORDER = ["GET", "POST", "PUT", "PATCH", "DELETE", "?"]

# Remarques sur les endpoints qui comptent pour l'app ou méritent une explication.
NOTES = {
    "/server/{x}/sftp": "**Seul endpoint qui renvoie le mot de passe SFTP à une clé API** (le bloc `sftp` de `GET /server/{id}` ne l'a plus). `PATCH` : probablement la réinitialisation.",
    "/server/{x}": "Le bloc `sftp` n'y contient plus `password` pour une clé API (il le contient pour la session web).",
    "/server/{x}/files/list/{x}": "API fichiers REST : liste d'un dossier (`{chemin}` vide = racine), sans SFTP.",
    "/server/{x}/files/content/{x}": "Contenu d'un fichier : `{ content, encoding }`.",
    "/server/{x}/files/content": "Écriture d'un fichier (corps non relevé).",
    "/server/{x}/files/exists": "Corps `{ files: [chemins] }` → `{ exists: { chemin: bool } }`.",
    "/servers/live/light/bulk": "État de plusieurs serveurs en un appel. Corps `{ server_ids: [ids] }`.",
    "/server/{x}/schedules": "Tâches planifiées **du panel** (distinctes des automatisations de l'app).",
    "/user/{x}/tasks": "Tâches en cours du compte (`global`, `nodes`, `targeted`).",
    "/server/{x}/stats/{x}/{x}": "Historique sur une période : `{début}` et `{fin}` en ISO 8601 (`2026-11-18T15:00:04`).",
    "/mybox/{x}/stats/{x}/{x}": "Historique de la MyBox ; même format de période.",
    "/user/{x}/apikey": "Gestion de la clé API (une clé principale, sans permissions par portée).",
    "/payment-methods": "Refusé à une clé API (`403 API_FORBIDDEN`).",
    "/user/{x}/passkeys": "Refusé à une clé API (`403 API_FORBIDDEN`).",
    "/user/{x}/2fa/setup": "Non testé : un `GET` qui peut générer un nouveau secret 2FA.",
    "/server/{x}/files/upload/url": "Non testé : génère une URL d'envoi.",
    "/server/{x}/files/bulk-upload-token": "Non testé : génère un jeton d'envoi.",
}

SECTIONS = [
    ("Compte", lambda p: p.startswith("/user") or p == "/payment-methods"),
    ("MyBox", lambda p: p.startswith("/mybox")),
    ("Serveurs", lambda p: p.startswith("/server") or p.startswith("/servers")),
    ("Catalogues du site", lambda p: p.startswith("/site")),
    ("Hébergement web", lambda p: p.startswith("/webhosting")),
]

SERVER_GROUPS = [
    ("Fichiers", ("files",)),
    ("Tâches planifiées du panel", ("schedule", "schedules")),
    ("Supervision et statistiques", ("live", "stats", "disk", "activity", "websocket")),
    ("Joueurs et commandes", ("command", "players", "poweraction")),
    ("Mods, plugins et contenus", ("mods", "mod", "plugin", "plugins", "modrinth", "curseforge", "datapacks",
                                    "bedrock", "hytale", "steamworkshop", "modpack", "palworld", "enshrouded", "ark")),
    ("Réseau et accès", ("sftp", "port", "address", "livemap", "bungeecord")),
    ("Configuration et démarrage", ("properties", "startup", "docker", "java", "php", "version", "minecraft_version",
                                    "reinstallation", "worlds", "world", "pod")),
    ("Sauvegardes", ("backup", "backups", "snapshots")),
]
USER_GROUPS = [
    ("Connexion et sécurité", ("login", "logout", "register", "2fa", "password", "passkey", "passkeys", "sessions",
                               "apikey", "oauth", "discord", "email", "delete")),
    ("Serveurs, sauvegardes et archives", ("servers", "servers_groups", "snapshot", "snapshots", "archive", "archive_v2",
                                           "archives", "backup_job", "backup_jobs", "database_archive_v2", "tasks",
                                           "ark", "pimpmybox", "migration", "migrations")),
    ("Crédits, achats et programmes", ("credits", "transactions", "transaction", "subscriptions", "subscription",
                                       "gifts", "gift", "goodies", "dailydrop", "affiliate", "partnership",
                                       "hosting-program")),
    ("Support et retours", ("ticket", "tickets", "ideas", "location-pings")),
]


def norm(path):
    """`/server/{id_server}/x` → `/server/{x}/x` : clé de rapprochement entre sources."""
    return re.sub(r"\{[^}]*\}", "{x}", path)


def pretty(path):
    """Nomme les paramètres de chemin pour la lecture."""
    parts = path.strip("/").split("/")
    out = []
    stats_seen = 0
    for i, seg in enumerate(parts):
        if seg != "{x}":
            out.append(seg)
            continue
        prev = parts[i - 1] if i else ""
        if prev in ("server", "mybox", "user", "webhosting"):
            out.append("{id_%s}" % prev)
        elif prev in ("content", "list", "download") or (prev == "{x}" and "files" in parts):
            out.append("{chemin}")
        elif "stats" in parts[:i]:
            out.append("{début}" if stats_seen == 0 else "{fin}")
            stats_seen += 1
        elif prev == "site":
            out.append("{source}")
        else:
            out.append("{id}")
    return "/" + "/".join(out)


def load_documented():
    methods = {}
    for line in (API / "openapi-resume.txt").read_text(encoding="utf-8").splitlines():
        m = re.fullmatch(r"(GET|POST|PUT|PATCH|DELETE) (/\S+)", line)
        if m:
            methods.setdefault(norm(m.group(2)), set()).add(m.group(1))
    return methods


def load_panel():
    methods = {}
    for line in (API / "endpoints-panel.txt").read_text(encoding="utf-8").splitlines():
        if not line or line.startswith("#"):
            continue
        meth, path = line.split(" ", 1)
        methods.setdefault(path.strip(), set()).update(meth.split("/"))
    return methods


def load_access():
    data = json.loads((API / "acces-cle-api.json").read_text(encoding="utf-8"))["resultats"]
    out = {}
    for key, v in data.items():
        method, path = key.split(" ", 1)
        out[(method, path)] = v
    return out


def access_cell(path, methods, access):
    tested = [(m, access[(m, path)]) for m in methods if (m, path) in access]
    if not tested:
        return "écriture, non testé" if methods and "GET" not in methods else "non testé"
    cells = []
    for m, r in tested:
        if r.get("status") == 200 and not r.get("erreur_api"):
            cells.append(f"✓ {m}")
        else:
            cells.append(f"✗ {m} ({r.get('status')} {r.get('erreur_api', '')})".replace(" )", ")"))
    return ", ".join(cells)


def group_of(path):
    parts = path.strip("/").split("/")
    if parts[0] in ("server", "servers"):
        key = parts[2] if len(parts) > 2 else ""
        if parts[0] == "servers":
            return "Supervision et statistiques"
        for label, keys in SERVER_GROUPS:
            if key in keys:
                return label
        return "Serveur" if not key else "Divers"
    if parts[0] == "user" or parts[0] == "payment-methods":
        if parts[0] == "payment-methods":
            return "Crédits, achats et programmes"
        key = parts[2] if len(parts) > 2 and parts[1] == "{x}" else (parts[1] if len(parts) > 1 else "")
        for label, keys in USER_GROUPS:
            if key in keys:
                return label
        return "Profil et préférences"
    return ""


def flatten(shape, prefix=""):
    if isinstance(shape, dict):
        if not shape:
            yield f"{prefix or '·'} : objet vide"
        for k, v in shape.items():
            p = f"{prefix}.{k}" if prefix else k
            if isinstance(v, (dict, list)):
                yield from flatten(v, p)
            else:
                yield f"{p} : {v}"
    elif isinstance(shape, list):
        if not shape:
            yield f"{prefix}[] : tableau (vide lors du relevé)"
        else:
            yield from flatten(shape[0], prefix + "[]")
    elif shape is not None:
        yield f"{prefix or '·'} : {shape}"


def main():
    documented = load_documented()
    panel = load_panel()
    access = load_access()
    every = sorted(set(documented) | set(panel))

    total = len(every)
    undocumented = [p for p in every if p not in documented]
    reads = [k for k in access if access[k].get("status") == 200 and not access[k].get("erreur_api")]

    lines = []
    w = lines.append
    w("# Référence de l'API MineStrator (documentée et non documentée)")
    w("")
    w("> Généré par `scripts/api-watch/reference.py` — ne pas éditer à la main. Relevé du **2026-11-18**.")
    w("")
    w("La [documentation officielle](https://api.minestrator.com/openapi-public-fr.yaml) ne couvre qu'une")
    w("partie de l'API. Celle-ci recense **tous** les endpoints appelés par le panel web (JavaScript")
    w("public du panel, plus les appels observés en le parcourant en lecture seule), indique lesquels sont")
    w("documentés, et lesquels ont été vérifiés avec une **clé API**.")
    w("")
    w("## En bref")
    w("")
    w(f"- **{total} chemins** recensés, dont **{len(undocumented)} absents de la documentation officielle**.")
    w(f"- **{len(reads)} lectures** testées avec une clé API répondent normalement ; seules les données de")
    w("  paiement et les passkeys sont réservées à la session web (`403 API_FORBIDDEN`). Une clé API a donc,")
    w("  en pratique, le même périmètre que le panel. Les écritures n'ont **pas** été testées (aucune action")
    w("  n'a été déclenchée), mais rien n'indique un traitement différent.")
    w("- Le **mot de passe SFTP** ne figure plus dans `GET /server/{id_server}` pour une clé API, mais")
    w("  `GET /server/{id_server}/sftp` le renvoie toujours.")
    w("- Une **API fichiers REST** complète existe (`/server/{id_server}/files/…` : lister, lire, écrire,")
    w("  créer, déplacer, renommer, supprimer, compresser, extraire, rechercher) : une alternative au SFTP.")
    w("- Autres endpoints utiles : état groupé des serveurs (`POST /servers/live/light/bulk`), tâches")
    w("  planifiées du panel (`/server/{id_server}/schedule…`), statistiques sur une période, joueurs hors")
    w("  ligne et inventaires, sous-utilisateurs et bases de données d'une MyBox.")
    w("")
    w("## Conventions")
    w("")
    w("- Base : `https://mine.sttr.io`. Authentification : `Authorization: Bearer <clé API>` (créée dans le")
    w("  panel : Compte → Clés API). Réponses dans l'enveloppe `{ api: { code, endpoint, data, error } }`.")
    w("- **Doc.** : ✓ si l'endpoint figure dans la documentation officielle.")
    w("- **Clé API** : ✓ méthode testée avec succès avec une clé API (lecture) ; ✗ refusée ; « non testé » sinon.")
    w("- `?` : méthode non déterminée par l'analyse du panel (souvent un `GET` de recherche).")
    w("- Ces endpoints non documentés peuvent changer sans préavis : la surveillance quotidienne")
    w("  (`.github/workflows/api-watch.yml`) vérifie ceux dont l'app dépend.")
    w("")

    for title, pred in SECTIONS:
        paths = [p for p in every if pred(p)]
        if not paths:
            continue
        w(f"## {title}")
        w("")
        groups = {}
        for p in paths:
            groups.setdefault(group_of(p), []).append(p)
        for group in sorted(groups, key=lambda g: (g == "Divers", g)):
            if group:
                w(f"### {group}")
                w("")
            w("| Méthode | Chemin | Doc. | Clé API | Remarque |")
            w("|---|---|---|---|---|")
            for p in groups[group]:
                methods = documented.get(p, set()) | panel.get(p, set())
                shown = [m for m in METHOD_ORDER if m in methods and not (m == "?" and len(methods) > 1)]
                doc = "✓" if p in documented else "—"
                note = NOTES.get(p, "").replace("|", "\\|")
                w(f"| {' / '.join(shown)} | `{pretty(p)}` | {doc} | {access_cell(p, methods, access)} | {note} |")
            w("")

    w("## Structure des réponses (lectures testées)")
    w("")
    w("Noms de champs et types relevés avec une clé API (`str`, `int`, `float`, `bool`, `null`) ; `[]` :")
    w("élément d'un tableau, `{n}` : clé numérique (identifiant). Un tableau vide lors du relevé n'a pas")
    w("révélé la forme de ses éléments.")
    w("")
    for (method, path), r in sorted(access.items(), key=lambda kv: kv[0][1]):
        if r.get("status") != 200 or r.get("erreur_api") or "data" not in r:
            continue
        w(f"<details><summary><code>{method} {pretty(path)}</code></summary>")
        w("")
        w("```")
        for line in flatten(r["data"]):
            w(line)
        w("```")
        w("")
        w("</details>")
        w("")

    out = API / "REFERENCE.md"
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(lines).rstrip() + "\n")
    print(f"{out.relative_to(ROOT)} : {total} chemins, {len(undocumented)} non documentés, {len(reads)} lectures vérifiées")


if __name__ == "__main__":
    main()
