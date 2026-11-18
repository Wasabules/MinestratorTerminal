<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { tabs } from '#lib/tabs/tabs.svelte.ts';
  import { t } from '#lib/i18n/index.ts';
  import {
    accountEvents,
    alertEvents,
    copilotEvents,
    sftpEvents,
    consoleEvents,
    modEvents,
  } from '#lib/events.ts';
  import { applyAccountSwitch } from '#lib/stores/accounts.svelte.ts';
  import { addAlert, addBacklog } from '#lib/alerts/alerts.svelte.ts';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { addDiagnosis, startRun, progressRun } from '#lib/copilot/diagnoses.svelte.ts';
  import { applyProgress } from '#lib/transfers/transfers.svelte.ts';
  import { applyInstallProgress } from '#lib/mods/installs.svelte.ts';
  import { setServerRuntime } from '#lib/servers/runtime.svelte.ts';
  import TabBar from './TabBar.svelte';
  import UserMenu from './UserMenu.svelte';
  import AlertCenter from './AlertCenter.svelte';
  import CopilotCenter from './CopilotCenter.svelte';
  import ModInstallCenter from './ModInstallCenter.svelte';
  import CopilotContextMenu from './CopilotContextMenu.svelte';
  import CommandPalette from './CommandPalette.svelte';
  import HomePanel from './HomePanel.svelte';
  import ServerPanel from './ServerPanel.svelte';
  import SettingsView from './views/SettingsView.svelte';
  import CopilotView from './views/CopilotView.svelte';
  import NotificationsView from './views/NotificationsView.svelte';

  const unlisteners: UnlistenFn[] = [];
  let destroyed = false;

  // Résout un serveur depuis un `conn_id` console : moniteur du superviseur (`mon:{id}`) ou
  // connexion d'une vue ouverte (conn_id = id d'onglet). Alimente la pastille d'état des onglets.
  function serverIdFor(connId: string): number | null {
    if (connId.startsWith('mon:')) {
      const n = Number(connId.slice(4));
      return Number.isFinite(n) ? n : null;
    }
    const tab = tabs.tabs.find((x) => x.id === connId);
    return tab && tab.kind === 'server' ? tab.serverId : null;
  }
  function onRuntime(connId: string, state: string) {
    if (!state) return;
    const id = serverIdFor(connId);
    if (id != null) setServerRuntime(id, state);
  }

  onMount(async () => {
    unlisteners.push(await alertEvents.new(addAlert));
    unlisteners.push(await copilotEvents.diagnosis(addDiagnosis));
    unlisteners.push(await copilotEvents.started(startRun));
    unlisteners.push(await copilotEvents.progress(progressRun));
    unlisteners.push(await sftpEvents.progress(applyProgress));
    unlisteners.push(await modEvents.installProgress(applyInstallProgress));
    unlisteners.push(await consoleEvents.status((p) => onRuntime(p.conn_id, p.state)));
    unlisteners.push(await consoleEvents.stats((p) => onRuntime(p.conn_id, p.state)));
    // Changement de compte (depuis cette fenêtre ou une autre) : onglets serveurs fermés, profil
    // rechargé ; le workspace est ensuite remonté pour le nouveau compte.
    unlisteners.push(await accountEvents.switched((s) => void applyAccountSwitch(s.current)));
    if (destroyed) unlisteners.forEach((u) => u()); // démonté pendant l'enregistrement
    // Alertes détectées par le service d'arrière-plan pendant que l'app était fermée (24 h).
    // Vide si aucun service ne supervise ce compte.
    api
      .serviceAlerts(24 * 3600)
      .then(addBacklog)
      .catch(() => {
        /* service absent ou inaccessible : rien à rattraper */
      });
    void runPendingServiceSetup();
  });

  /**
   * Supervision en arrière-plan choisie dans l'installeur (Windows). L'installeur ne demande aucun
   * droit : l'activation (élévation UAC + transmission de la clé) a lieu ici, une fois connecté,
   * car le service a besoin de la clé API. Le choix est consommé : jamais redemandé.
   */
  async function runPendingServiceSetup() {
    let pending = false;
    try {
      pending = await api.serviceTakePendingSetup();
    } catch {
      return;
    }
    if (!pending) return;
    const note = (severity: string, message: string) =>
      addAlert({
        server_id: 0,
        server_name: t('service.title'),
        kind: 'service',
        severity,
        message,
        ts: Math.floor(Date.now() / 1000),
      });
    try {
      await api.serviceEnable();
      note('info', t('service.setupDone'));
    } catch (e) {
      note('warning', `${t('service.setupFailed')} (${humanizeError(e)})`);
    }
  }
  onDestroy(() => {
    destroyed = true;
    unlisteners.forEach((u) => u());
  });
</script>

<div class="workspace">
  <header class="topbar">
    <button class="brand" onclick={() => tabs.focusHome()} title={t('common.appName')}>
      <span class="mark" aria-hidden="true">◧</span>
    </button>
    <TabBar />
    <div class="controls">
      <!-- Bouton + palette de commandes (Ctrl+K / ⌘K), dialog modal en couche supérieure. -->
      <CommandPalette />
      <ModInstallCenter />
      <CopilotCenter />
      <AlertCenter />
      <UserMenu />
    </div>
  </header>

  <div class="panels">
    <!-- Panneaux montés en permanence ; seul l'actif est affiché (keep-alive). -->
    {#each tabs.tabs as tab (tab.id)}
      <div class="panel" class:active={tab.id === tabs.activeId}>
        {#if tab.kind === 'home'}
          <HomePanel />
        {:else if tab.kind === 'settings'}
          <SettingsView />
        {:else if tab.kind === 'copilot'}
          <CopilotView />
        {:else if tab.kind === 'notifications'}
          <NotificationsView />
        {:else}
          <ServerPanel {tab} />
        {/if}
      </div>
    {/each}
  </div>
</div>

<CopilotContextMenu />

<style>
  .workspace {
    height: 100vh;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .topbar {
    display: flex;
    align-items: stretch;
    gap: 8px;
    padding: 0 10px;
    background: var(--bg);
    border-bottom: 1px solid var(--border);
    flex: none;
  }
  .controls {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: none;
  }
  .brand {
    display: flex;
    align-items: center;
    background: none;
    border: none;
    cursor: pointer;
    padding: 0 4px;
    flex: none;
  }
  .mark {
    width: 26px;
    height: 26px;
    border-radius: 7px;
    display: grid;
    place-items: center;
    color: #fff;
    background: var(--brand-gradient);
    font-size: 14px;
  }
  .panels {
    position: relative;
    flex: 1;
    min-height: 0;
  }
  .panel {
    position: absolute;
    inset: 0;
    display: none;
    overflow: auto;
  }
  .panel.active {
    display: block;
  }
</style>
