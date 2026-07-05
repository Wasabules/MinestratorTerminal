<script lang="ts">
  import { api, humanizeError } from '$lib/ipc';
  import { t } from '$lib/i18n';
  import type { PowerAction } from '$lib/types';

  let {
    serverId,
    running,
    disabled = false,
    compact = false,
  }: { serverId: number; running: boolean; disabled?: boolean; compact?: boolean } = $props();

  let busy = $state(false);
  let menuOpen = $state(false);
  let error = $state<string | null>(null);

  const RUNNING_ACTIONS: PowerAction[] = ['restart', 'restart10', 'stop10', 'kill'];
  const STOPPED_ACTIONS: PowerAction[] = ['restart'];

  // Actions secondaires regroupées dans le menu « ⋯ ».
  const secondary = $derived(running ? RUNNING_ACTIONS : STOPPED_ACTIONS);

  async function act(action: PowerAction) {
    menuOpen = false;
    busy = true;
    error = null;
    try {
      await api.powerAction(serverId, action);
    } catch (e) {
      error = humanizeError(e);
    } finally {
      busy = false;
    }
  }

  const primary = $derived(running ? 'stop' : 'start');
</script>

<div class="power">
  <div class="group" class:danger={running} class:compact>
    <button class="main" disabled={disabled || busy} onclick={() => act(primary)}>
      {#if busy}<span class="spinner"></span>{/if}
      {t(`power.${primary}`)}
    </button>
    <button
      class="caret"
      disabled={disabled || busy}
      aria-label={t('power.more')}
      onclick={() => (menuOpen = !menuOpen)}
    >
      ⋯
    </button>
  </div>

  {#if menuOpen}
    <button class="backdrop" aria-label={t('common.close')} onclick={() => (menuOpen = false)}
    ></button>
    <div class="menu" role="menu">
      {#each secondary as action (action)}
        <button class="item" role="menuitem" onclick={() => act(action)}>
          {t(`power.${action}`)}
        </button>
      {/each}
    </div>
  {/if}

  {#if error}<span class="err">{error}</span>{/if}
</div>

<style>
  .power {
    position: relative;
    display: inline-flex;
    flex-direction: column;
    gap: 6px;
    align-items: flex-start;
  }
  .group {
    display: inline-flex;
    border-radius: var(--radius);
    overflow: hidden;
    box-shadow: 0 1px 0 rgba(0, 0, 0, 0.2);
  }
  .main {
    border: none;
    cursor: pointer;
    font: inherit;
    font-weight: 600;
    color: #fff;
    background: var(--brand-primary);
    padding: 9px 16px;
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
  .group.danger .main {
    background: var(--brand-pro);
  }
  .caret {
    border: none;
    border-left: 1px solid rgba(255, 255, 255, 0.18);
    cursor: pointer;
    color: #fff;
    background: var(--brand-primary);
    padding: 0 12px;
    font-size: 15px;
  }
  .group.danger .caret {
    background: var(--brand-pro);
  }
  .main:hover:not(:disabled),
  .caret:hover:not(:disabled) {
    filter: brightness(1.1);
  }
  .main:disabled,
  .caret:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }
  .group.compact {
    border-radius: 7px;
  }
  .group.compact .main {
    padding: 3px 11px;
    font-size: 11.5px;
    gap: 6px;
  }
  .group.compact .caret {
    padding: 0 9px;
    font-size: 12px;
  }
  .backdrop {
    position: fixed;
    inset: 0;
    background: none;
    border: none;
    z-index: 20;
    cursor: default;
  }
  .menu {
    position: absolute;
    top: calc(100% + 6px);
    left: 0;
    z-index: 21;
    min-width: 190px;
    background: var(--elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: var(--shadow);
    padding: 5px;
    display: flex;
    flex-direction: column;
  }
  .item {
    text-align: left;
    background: none;
    border: none;
    cursor: pointer;
    font: inherit;
    color: var(--text);
    padding: 8px 11px;
    border-radius: 7px;
  }
  .item:hover {
    background: color-mix(in srgb, var(--text) 8%, transparent);
  }
  .err {
    font-size: 12px;
    color: var(--state-danger);
  }
</style>
