#!/usr/bin/env bash
# Issue d'alerte d'une surveillance automatique (une seule ouverte par étiquette).
#
#   alerte-issue.sh ouvrir <étiquette> <titre> <fichier-corps>
#       Ouvre l'issue, ou commente celle déjà ouverte — sauf si ce rapport exact y figure déjà
#       (empreinte cachée dans le texte) : un écart qui dure ne commente pas chaque jour.
#   alerte-issue.sh fermer <étiquette> <message>
#       Ferme l'issue ouverte, s'il y en a une, avec ce message.
#
# Nécessite `gh` authentifié (GH_TOKEN) et la permission `issues: write`.
set -euo pipefail

action=$1
label=$2
existing=$(gh issue list --label "$label" --state open --json number -q '.[0].number // empty')

case "$action" in
  ouvrir)
    title=$3
    body=$4
    digest=$(sha256sum "$body" | cut -c1-16)
    marker="<!-- rapport:$digest -->"
    printf '\n%s\n' "$marker" >> "$body"
    gh label create "$label" --color D93F0B \
      --description "Alerte ouverte par une surveillance automatique" 2>/dev/null || true
    if [ -z "$existing" ]; then
      gh issue create --title "$title" --label "$label" --body-file "$body"
    elif gh issue view "$existing" --json body,comments -q '.body, .comments[].body' | grep -qF "$marker"; then
      echo "Issue #$existing : rapport inchangé, pas de nouveau commentaire."
    else
      gh issue comment "$existing" --body-file "$body"
    fi
    ;;
  fermer)
    message=$3
    if [ -n "$existing" ]; then
      gh issue close "$existing" --comment "$message"
    fi
    ;;
  *)
    echo "action inconnue : $action" >&2
    exit 2
    ;;
esac
