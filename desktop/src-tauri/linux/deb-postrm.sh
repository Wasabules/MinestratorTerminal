#!/bin/sh
# postrm du paquet .deb — service d'arrière-plan (docs/SERVICE.md).
#
# `apt purge` : efface aussi l'historique du service. Avec `DynamicUser=yes`, systemd range
# l'état dans /var/lib/private/… et n'expose qu'un lien dans /var/lib.
set -e

case "$1" in
    purge)
        rm -rf /var/lib/minestrator-daemon /var/lib/private/minestrator-daemon
        ;;
esac

exit 0
