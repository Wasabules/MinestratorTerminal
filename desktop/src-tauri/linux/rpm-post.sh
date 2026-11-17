# %post du paquet .rpm — service d'arrière-plan (docs/SERVICE.md).
# $1 = 1 à l'installation, 2 à la mise à jour : dans les deux cas, (ré)installer et redémarrer.
# Les comptes déjà autorisés sont conservés. Un échec ne bloque pas le paquet.
if [ -x /usr/bin/minestrator-daemon ] && [ -d /run/systemd/system ]; then
    /usr/bin/minestrator-daemon install \
        || echo "minestrator-daemon : service non installé (activable depuis l'application)." >&2
fi
exit 0
