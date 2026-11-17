# %preun du paquet .rpm — service d'arrière-plan (docs/SERVICE.md).
# $1 = 0 à la suppression : arrêt, retrait de l'unité et de la clé API machine.
# $1 >= 1 à la mise à jour : rien. Le %post de la nouvelle version s'exécute AVANT ce %preun et
# a déjà redémarré le service sur le nouveau binaire ; l'arrêter ici le couperait.
# RPM n'a pas de « purge » : l'historique reste dans /var/lib/private/minestrator-daemon
# (`minestrator-daemon uninstall --purge` avant la suppression pour tout effacer).
if [ "$1" = "0" ] && [ -x /usr/bin/minestrator-daemon ]; then
    /usr/bin/minestrator-daemon uninstall || true
fi
exit 0
