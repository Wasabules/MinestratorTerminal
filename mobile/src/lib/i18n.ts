/**
 * i18n fr/en réactif : `t(key)` lit la locale du store de réglages (rune), donc changer la
 * langue dans les Réglages met à jour l'UI en direct.
 */
import { settings } from "./stores/settings.svelte";

type Dict = Record<string, string>;

const fr: Dict = {
  "app.title": "Minestrator Terminal",

  "onboarding.title": "Connexion",
  "onboarding.subtitle": "Colle ta clé API MineStrator (Panel → Compte → Clés API).",
  "onboarding.placeholder": "Clé API",
  "onboarding.submit": "Se connecter",
  "onboarding.validating": "Validation…",
  "onboarding.getKey": "Ouvrir MineStrator pour récupérer ma clé",

  "servers.title": "Serveurs",
  "servers.empty": "Aucun serveur.",
  "servers.refresh": "Rafraîchir",

  "nav.overview": "Aperçu",
  "nav.console": "Console",
  "nav.players": "Joueurs",
  "nav.files": "Fichiers",
  "nav.backups": "Sauvegardes",

  "backups.snapshots": "Snapshots",
  "backups.daily": "Backups quotidiens",
  "backups.none": "Aucune sauvegarde.",
  "backups.create": "Créer un snapshot",
  "backups.name": "Nom du snapshot",
  "backups.restore": "Restaurer",
  "backups.delete": "Supprimer",
  "backups.confirmRestore": "Restaurer cette sauvegarde ? (écrase les données actuelles)",
  "backups.confirmDelete": "Supprimer ce snapshot ?",
  "backups.done": "Demande envoyée.",
  "backups.cancel": "Annuler",
  "backups.ok": "OK",

  "status.running": "En ligne",
  "status.offline": "Hors ligne",
  "status.starting": "Démarrage…",
  "status.stopping": "Arrêt…",
  "status.hibernation": "En veille",
  "status.unknown": "Inconnu",

  "overview.cpu": "CPU",
  "overview.ram": "RAM",
  "overview.disk": "Disque",
  "overview.players": "Joueurs",
  "overview.version": "Version",
  "overview.uptime": "Uptime",

  "power.start": "Démarrer",
  "power.restart": "Redémarrer",
  "power.stop": "Arrêter",
  "power.kill": "Kill",

  "console.placeholder": "Commande…",
  "console.connecting": "Connexion…",
  "console.filter": "Filtrer",
  "console.all": "Tout",
  "console.error": "Erreurs",
  "console.warn": "Alertes",
  "console.info": "Infos",

  "players.online": "En ligne",
  "players.byName": "Par pseudo (même hors-ligne)",
  "players.none": "Aucun joueur en ligne.",
  "players.pseudo": "Pseudo",
  "players.done": "Fait.",
  "action.kick": "Kick",
  "action.ban": "Ban",
  "action.unban": "Unban",
  "action.op": "OP",
  "action.deop": "Retirer OP",
  "action.wl_add": "Whitelist +",
  "action.wl_remove": "Whitelist −",

  "files.empty": "Dossier vide.",
  "files.newFolder": "Nouveau dossier",
  "files.folderName": "Nom du dossier",
  "files.save": "Enregistrer",
  "files.saved": "Enregistré.",
  "files.delete": "Supprimer",
  "files.rename": "Renommer",
  "files.confirmDelete": "Supprimer",
  "files.newName": "Nouveau nom",
  "files.binary": "Fichier binaire — aperçu indisponible.",
  "files.tooBig": "Fichier trop volumineux pour l'éditeur.",
  "files.create": "Créer",
  "files.cancel": "Annuler",

  "settings.title": "Réglages",
  "settings.version": "Version",
  "settings.theme": "Thème",
  "settings.language": "Langue",
  "settings.system": "Système",
  "settings.dark": "Sombre",
  "settings.light": "Clair",
  "settings.french": "Français",
  "settings.english": "English",
  "settings.about": "Client tiers de l'API MineStrator.",
  "settings.website": "Site MineStrator",
  "settings.checkUpdate": "Rechercher des mises à jour",
  "settings.checking": "Recherche…",
  "settings.upToDate": "À jour.",

  "update.available": "Nouvelle version",
  "update.now": "Mettre à jour",
  "update.downloading": "Téléchargement…",
  "update.error": "Échec — réessaie plus tard.",
  "update.later": "Plus tard",

  "error.generic": "Une erreur est survenue.",
  "error.unauthorized": "Clé API invalide ou expirée.",
  "error.rate_limited": "Trop de requêtes, réessaie dans un instant.",
  "error.network": "Problème de réseau.",
  "error.no_key": "Aucune clé API enregistrée.",
  "logout": "Se déconnecter",
};

const en: Dict = {
  ...fr,
  "onboarding.title": "Sign in",
  "onboarding.subtitle": "Paste your MineStrator API key (Panel → Account → API keys).",
  "onboarding.placeholder": "API key",
  "onboarding.submit": "Connect",
  "onboarding.validating": "Validating…",
  "onboarding.getKey": "Open MineStrator to get my key",

  "servers.title": "Servers",
  "servers.empty": "No server.",
  "servers.refresh": "Refresh",

  "nav.overview": "Overview",
  "nav.console": "Console",
  "nav.players": "Players",
  "nav.files": "Files",

  "status.running": "Online",
  "status.offline": "Offline",
  "status.starting": "Starting…",
  "status.stopping": "Stopping…",
  "status.hibernation": "Hibernated",
  "status.unknown": "Unknown",

  "overview.disk": "Disk",

  "power.start": "Start",
  "power.restart": "Restart",
  "power.stop": "Stop",

  "console.placeholder": "Command…",
  "console.connecting": "Connecting…",
  "console.filter": "Filter",
  "console.all": "All",
  "console.error": "Errors",
  "console.warn": "Warnings",
  "console.info": "Info",

  "players.online": "Online",
  "players.byName": "By name (even offline)",
  "players.none": "No player online.",
  "players.pseudo": "Username",
  "players.done": "Done.",
  "action.deop": "Remove OP",

  "files.empty": "Empty folder.",
  "files.newFolder": "New folder",
  "files.folderName": "Folder name",
  "files.save": "Save",
  "files.saved": "Saved.",
  "files.delete": "Delete",
  "files.rename": "Rename",
  "files.confirmDelete": "Delete",
  "files.newName": "New name",
  "files.binary": "Binary file — no preview.",
  "files.tooBig": "File too large for the editor.",
  "files.create": "Create",
  "files.cancel": "Cancel",

  "settings.title": "Settings",
  "settings.version": "Version",
  "settings.theme": "Theme",
  "settings.language": "Language",
  "settings.system": "System",
  "settings.dark": "Dark",
  "settings.light": "Light",
  "settings.about": "Third-party client for the MineStrator API.",
  "settings.website": "MineStrator website",

  "error.generic": "Something went wrong.",
  "error.unauthorized": "Invalid or expired API key.",
  "error.rate_limited": "Too many requests, try again shortly.",
  "error.network": "Network problem.",
  "error.no_key": "No API key stored.",
  "logout": "Log out",
};

export function t(key: string): string {
  const dict = settings.locale === "en" ? en : fr;
  return dict[key] ?? key;
}
