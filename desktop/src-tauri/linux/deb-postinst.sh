#!/bin/sh
# postinst du paquet .deb — service d'arrière-plan (docs/SERVICE.md).
#
# Installe ou met à jour l'unité systemd `minestrator-daemon.service` et la (re)démarre. Le
# service attend ensuite son activation depuis l'app (clé API transmise par IPC). Les comptes
# déjà autorisés sont conservés d'une version à l'autre. Un échec ne bloque pas le paquet.
set -e

case "$1" in
    configure)
        if [ -x /usr/bin/minestrator-daemon ] && [ -d /run/systemd/system ]; then
            /usr/bin/minestrator-daemon install \
                || echo "minestrator-daemon : service non installé (activable depuis l'application)." >&2
        fi
        ;;
esac

exit 0
