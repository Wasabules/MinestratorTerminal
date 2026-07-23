/**
 * i18n minimal fr/en pour le scaffold mobile. À aligner ensuite sur l'i18n du desktop
 * (factorisé dans un package partagé). `t(key)` renvoie la chaîne de la locale courante.
 */

type Dict = Record<string, string>;

const fr: Dict = {
  "app.title": "Minestrator Terminal",
  "onboarding.title": "Connexion",
  "onboarding.subtitle": "Colle ta clé API MineStrator (Panel → Compte → Clés API).",
  "onboarding.placeholder": "Clé API",
  "onboarding.submit": "Se connecter",
  "onboarding.validating": "Validation…",
  "servers.title": "Serveurs",
  "servers.empty": "Aucun serveur.",
  "servers.refresh": "Rafraîchir",
  "nav.overview": "Aperçu",
  "nav.console": "Console",
  "nav.players": "Joueurs",
  "overview.cpu": "CPU",
  "overview.ram": "RAM",
  "overview.disk": "Disque",
  "overview.players": "Joueurs",
  "overview.version": "Version",
  "power.start": "Démarrer",
  "power.restart": "Redémarrer",
  "power.stop": "Arrêter",
  "power.kill": "Kill",
  "console.placeholder": "Commande…",
  "console.send": "Envoyer",
  "console.connecting": "Connexion…",
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
  "error.generic": "Une erreur est survenue.",
  "error.Unauthorized": "Clé API invalide ou expirée.",
  "error.RateLimited": "Trop de requêtes, réessaie dans un instant.",
  "error.Network": "Problème de réseau.",
  "logout": "Se déconnecter",
};

// Fallback en anglais partiel (complété plus tard).
const en: Dict = {
  ...fr,
  "onboarding.title": "Sign in",
  "onboarding.subtitle": "Paste your MineStrator API key (Panel → Account → API keys).",
  "onboarding.submit": "Connect",
  "servers.title": "Servers",
  "nav.overview": "Overview",
  "nav.console": "Console",
  "nav.players": "Players",
  "logout": "Log out",
};

const locale: "fr" | "en" =
  typeof navigator !== "undefined" && navigator.language?.startsWith("en") ? "en" : "fr";

const dict = locale === "en" ? en : fr;

export function t(key: string): string {
  return dict[key] ?? key;
}
