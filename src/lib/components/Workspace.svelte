<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { tabs } from '$lib/tabs/tabs.svelte';
  import { t } from '$lib/i18n';
  import { alertEvents, copilotEvents, sftpEvents } from '$lib/events';
  import { addAlert } from '$lib/alerts/alerts.svelte';
  import { addDiagnosis, startRun, progressRun } from '$lib/copilot/diagnoses.svelte';
  import { applyProgress } from '$lib/transfers/transfers.svelte';
  import TabBar from './TabBar.svelte';
  import UserMenu from './UserMenu.svelte';
  import AlertCenter from './AlertCenter.svelte';
  import CopilotCenter from './CopilotCenter.svelte';
  import CopilotContextMenu from './CopilotContextMenu.svelte';
  import HomePanel from './HomePanel.svelte';
  import ServerPanel from './ServerPanel.svelte';
  import SettingsView from './views/SettingsView.svelte';
  import CopilotView from './views/CopilotView.svelte';

  const unlisteners: UnlistenFn[] = [];
  let destroyed = false;
  onMount(async () => {
    unlisteners.push(await alertEvents.new(addAlert));
    unlisteners.push(await copilotEvents.diagnosis(addDiagnosis));
    unlisteners.push(await copilotEvents.started(startRun));
    unlisteners.push(await copilotEvents.progress(progressRun));
    unlisteners.push(await sftpEvents.progress(applyProgress));
    if (destroyed) unlisteners.forEach((u) => u()); // démonté pendant l'enregistrement
  });
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
