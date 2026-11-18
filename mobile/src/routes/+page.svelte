<script lang="ts">
  import { api, ensureNotificationPermission } from "#lib/ipc.ts";
  import { auth } from "#lib/stores/auth.svelte.ts";
  import { alerts } from "#lib/stores/alerts.svelte.ts";
  import { settings } from "#lib/stores/settings.svelte.ts";
  import { initFake, isFake } from "#lib/fake.svelte.ts";
  import { t } from "#lib/i18n.ts";
  import Onboarding from "#lib/components/Onboarding.svelte";
  import ServersList from "#lib/components/ServersList.svelte";
  import ServerView from "#lib/components/ServerView.svelte";
  import SettingsView from "#lib/components/SettingsView.svelte";
  import NotificationsView from "#lib/components/NotificationsView.svelte";
  import AlertsView from "#lib/components/AlertsView.svelte";
  import UpdateBanner from "#lib/components/UpdateBanner.svelte";
  import type { MyBoxSummary, ServerListItem } from "#lib/types.ts";

  let selected = $state<{ server: ServerListItem; box: MyBoxSummary | null } | null>(null);
  let showSettings = $state(false);
  let showNotifications = $state(false);
  let showAlerts = $state(false);

  /**
   * Écran courant mémorisé, pour le retrouver après un rechargement du webview.
   *
   * Android peut tuer le *renderer* d'une WebView passée en arrière-plan (pression mémoire) ;
   * la page repart alors de zéro et l'utilisateur se retrouve sur la liste des serveurs sans
   * comprendre pourquoi — typiquement au retour du sélecteur de fichiers. Le même effet se
   * produit en développement quand le client Vite se reconnecte.
   */
  const LAST_SERVER = "mnstr-last-server";

  function rememberServer(id: number | null): void {
    try {
      if (id === null) localStorage.removeItem(LAST_SERVER);
      else localStorage.setItem(LAST_SERVER, String(id));
    } catch {
      /* stockage indisponible : la restauration est un confort, pas une exigence */
    }
  }

  /** Rouvre le dernier serveur consulté, si la liste le contient encore. */
  async function restoreServer(): Promise<void> {
    let wanted: number | null = null;
    try {
      const raw = localStorage.getItem(LAST_SERVER);
      wanted = raw ? Number(raw) : null;
    } catch {
      return;
    }
    if (!wanted || selected) return;
    try {
      const { servers, myboxes } = await api.listServers();
      const server = servers.find((s) => s.id === wanted);
      if (!server) {
        rememberServer(null); // serveur disparu : on n'insiste pas
        return;
      }
      selected = { server, box: myboxes.find((m) => m.id === server.mybox_id) ?? null };
    } catch {
      /* liste indisponible : on reste sur l'accueil */
    }
  }

  // Thème : applique au boot + suit le système quand le préréglage est "system".
  $effect(() => {
    initFake();
    settings.applyTheme();
    if (typeof matchMedia === "undefined") return;
    const mq = matchMedia("(prefers-color-scheme: light)");
    const onChange = () => settings.theme === "system" && settings.applyTheme();
    mq.addEventListener?.("change", onChange);
    return () => mq.removeEventListener?.("change", onChange);
  });

  // Boot auth.
  $effect(() => {
    (async () => {
      try {
        if (await api.hasStoredKey()) {
          auth.setUser(await api.getUser());
        }
      } catch {
        auth.setUser(null);
      } finally {
        auth.booted = true;
      }
    })();
  });

  // Restauration de l'écran après un rechargement (voir `LAST_SERVER`).
  $effect(() => {
    if (!auth.isAuthed) return;
    void restoreServer();
  });

  // Notifications : demande la permission une fois connecté (Android 13+ l'exige à l'exécution,
  // sinon les alertes du superviseur sont bloquées silencieusement). No-op si déjà accordée.
  // Ré-arme aussi le service de surveillance si l'utilisateur l'avait activé (le process a pu
  // être tué depuis la dernière session).
  $effect(() => {
    if (!auth.isAuthed) return;
    ensureNotificationPermission();
    if (settings.bgMonitoring) api.setBackgroundMonitoring(true).catch(() => {});
  });

  // Centre d'alertes : relit le journal local et suit les nouvelles alertes (pastille de la liste,
  // alertes récentes de l'Aperçu). Une seule souscription pour toute la vie de la page.
  $effect(() => {
    if (!auth.isAuthed) return;
    void alerts.start();
  });
</script>

{#if auth.isAuthed}
  <UpdateBanner />
{/if}

{#if isFake()}
  <!-- Repère permanent : sans lui, on peut publier une capture en croyant le mode actif. -->
  <div class="fakebar">{t("settings.fakeMode")}</div>
{/if}

{#if !auth.booted}
  <div class="splash"><div class="logo"></div></div>
{:else if !auth.isAuthed}
  <Onboarding />
{:else if showSettings}
  <SettingsView onBack={() => (showSettings = false)} />
{:else if showNotifications}
  <NotificationsView onBack={() => (showNotifications = false)} />
{:else if showAlerts}
  <AlertsView
    onBack={() => (showAlerts = false)}
    onNotifications={() => (showNotifications = true)}
  />
{:else if selected}
  <ServerView
    server={selected.server}
    mybox={selected.box}
    onBack={() => {
      selected = null;
      rememberServer(null);
    }}
  />
{:else}
  <ServersList
    onOpen={(s, box) => {
      selected = { server: s, box };
      rememberServer(s.id);
    }}
    onSettings={() => (showSettings = true)}
    onAlerts={() => (showAlerts = true)}
  />
{/if}

<style>
  .fakebar {
    position: fixed;
    z-index: 999;
    left: 50%;
    bottom: calc(var(--safe-bottom) + 6px);
    transform: translateX(-50%);
    padding: 3px 10px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--brand-tertiary) 85%, black);
    color: #fff;
    font-size: 10px;
    pointer-events: none;
    opacity: 0.85;
  }

  .splash {
    height: 100dvh;
    display: grid;
    place-items: center;
  }
  .logo {
    width: 76px;
    height: 76px;
    border-radius: 22px;
    background: var(--brand-gradient);
  }
</style>
