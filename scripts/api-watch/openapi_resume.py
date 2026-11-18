#!/usr/bin/env python3
"""Résumé structurel de la documentation OpenAPI publique de MineStrator.

Produit une ligne par endpoint, paramètre et champ (requêtes et réponses), avec son type, triée :
un `diff` entre deux résumés montre exactement ce qui a été ajouté, retiré ou retypé. Les textes
descriptifs sont ignorés (ils changent sans conséquence pour l'app). Le résumé de référence est
versionné dans `docs/api/openapi-resume.txt` ; `.github/workflows/api-watch.yml` le compare chaque
jour à celui de la documentation en ligne.

Usage : `python3 scripts/api-watch/openapi_resume.py [fichier.yaml | URL] > resume.txt`
Dépendance : PyYAML.
"""
import sys
import urllib.request

import yaml

DEFAULT_URL = "https://api.minestrator.com/openapi-public-fr.yaml"
MAX_DEPTH = 12
METHODS = ("get", "post", "put", "patch", "delete")


def load(source):
    if source.startswith(("http://", "https://")):
        req = urllib.request.Request(source, headers={"User-Agent": "minestrator-terminal-api-watch"})
        with urllib.request.urlopen(req, timeout=60) as resp:
            return yaml.safe_load(resp.read())
    with open(source, encoding="utf-8") as f:
        return yaml.safe_load(f)


def resolve(doc, node, seen):
    """Suit les `$ref` internes (`#/components/...`) ; `seen` évite les cycles."""
    while isinstance(node, dict) and "$ref" in node:
        ref = node["$ref"]
        if ref in seen or not ref.startswith("#/"):
            return {"type": f"→ {ref}"}, seen
        seen = seen | {ref}
        target = doc
        for part in ref[2:].split("/"):
            target = target.get(part, {}) if isinstance(target, dict) else {}
        node = target
    return node, seen


def type_of(schema):
    t = schema.get("type")
    if isinstance(t, list):
        t = "|".join(str(x) for x in t)
    fmt = schema.get("format")
    enum = schema.get("enum")
    out = str(t) if t else ("object" if "properties" in schema else "?")
    if fmt:
        out += f"({fmt})"
    if enum is not None:
        out += " enum[" + ",".join(str(e) for e in enum) + "]"
    if schema.get("nullable"):
        out += "?"
    return out


def walk(doc, schema, path, out, seen=frozenset(), depth=0):
    schema, seen = resolve(doc, schema, seen)
    if not isinstance(schema, dict) or depth > MAX_DEPTH:
        return
    for combo in ("allOf", "oneOf", "anyOf"):
        for i, sub in enumerate(schema.get(combo, []) or []):
            tag = "" if combo == "allOf" else f"<{combo}{i}>"
            walk(doc, sub, path + tag, out, seen, depth + 1)
    if "items" in schema:
        out.append(f"{path}[] : {type_of(resolve(doc, schema['items'], seen)[0])}")
        walk(doc, schema["items"], path + "[]", out, seen, depth + 1)
    required = set(schema.get("required", []) or [])
    for name, sub in sorted((schema.get("properties") or {}).items()):
        resolved, _ = resolve(doc, sub, seen)
        mark = " (requis)" if name in required else ""
        child = f"{path}.{name}" if path else name
        out.append(f"{child} : {type_of(resolved) if isinstance(resolved, dict) else '?'}{mark}")
        walk(doc, sub, child, out, seen, depth + 1)


def summarize(doc):
    lines = []
    for route, item in sorted((doc.get("paths") or {}).items()):
        for method in METHODS:
            op = (item or {}).get(method)
            if not isinstance(op, dict):
                continue
            head = f"{method.upper()} {route}"
            lines.append(head)
            params = list(item.get("parameters", []) or []) + list(op.get("parameters", []) or [])
            for p in params:
                p, _ = resolve(doc, p, frozenset())
                schema, _ = resolve(doc, p.get("schema", {}), frozenset())
                req = " (requis)" if p.get("required") else ""
                lines.append(f"{head} :: paramètre {p.get('in')}.{p.get('name')} : {type_of(schema)}{req}")
            body = (op.get("requestBody") or {}).get("content", {})
            for ctype, media in sorted(body.items()):
                fields = []
                walk(doc, media.get("schema", {}), "", fields)
                lines.extend(f"{head} :: corps {ctype} {f}" for f in fields)
            for status, resp in sorted((op.get("responses") or {}).items(), key=lambda kv: str(kv[0])):
                resp, _ = resolve(doc, resp, frozenset())
                lines.append(f"{head} :: réponse {status}")
                for ctype, media in sorted(((resp or {}).get("content") or {}).items()):
                    fields = []
                    walk(doc, media.get("schema", {}), "", fields)
                    lines.extend(f"{head} :: réponse {status} {f}" for f in fields)
    return lines


def main():
    source = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_URL
    doc = load(source)
    sys.stdout.reconfigure(encoding="utf-8", newline="\n")
    print(f"# Résumé structurel de {DEFAULT_URL.rsplit('/', 1)[-1]} (version {doc.get('info', {}).get('version')})")
    print("# Généré par scripts/api-watch/openapi_resume.py — ne pas éditer à la main.")
    for line in summarize(doc):
        print(line)


if __name__ == "__main__":
    main()
