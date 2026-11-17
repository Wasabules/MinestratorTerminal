#!/bin/sh
# prerm du paquet .deb — service d'arrière-plan (docs/SERVICE.md).
#
# - suppression : arrêt et retrait de l'unité, effacement de la clé API machine (l'historique
#   reste jusqu'à un `purge`, voir postrm) ;
# - mise à jour : arrêt seulement, le postinst de la nouvelle version redémarre le service.
set -e

if [ -x /usr/bin/minestrator-daemon ]; then
    case "$1" in
        remove|deconfigure)
            /usr/bin/minestrator-daemon uninstall || true
            ;;
        upgrade|failed-upgrade)
            /usr/bin/minestrator-daemon stop || true
            ;;
    esac
fi

exit 0
