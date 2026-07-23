<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { api, humanizeError } from '$lib/ipc';
  import { t } from '$lib/i18n';
  import type { LiveLight, PlayerAction } from '$lib/types';
  import type { ServerTab } from '$lib/tabs/tabs.svelte';
  import PlayerActionsMenu from '../PlayerActionsMenu.svelte';

  let { tab }: { tab: ServerTab } = $props();
  const serverId = $derived(tab.serverId);

  let live = $state<LiveLight | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let manualName = $state('');
  let flash = $state<string | null>(null);
  let flashErr = $state(false);
  let flashTimer: ReturnType<typeof setTimeout> | undefined;

  const players = $derived(live?.players?.list ?? []);
  const count = $derived(live?.players?.current ?? players.length);
  const limit = $derived(live?.players?.limit ?? 0);

  // Actions pertinentes pour un joueur connecté (pas de « débannir »).
  const connectedActions: PlayerAction[] = [
    'kick',
    'ban',
    'op_add',
    'op_remove',
    'whitelist_add',
    'whitelist_remove',
  ];
  // Actions pour un pseudo saisi (inclut « débannir »).
  const manualActions: PlayerAction[] = [
    'ban',
    'unban',
    'op_add',
    'op_remove',
    'whitelist_add',
    'whitelist_remove',
  ];

  onMount(load);
  onDestroy(() => clearTimeout(flashTimer));

  async function load() {
    loading = true;
    error = null;
    try {
      live = await api.liveLight(serverId);
    } catch (e) {
      error = humanizeError(e);
    } finally {
      loading = false;
    }
  }

  async function act(player: string, action: PlayerAction) {
    const name = player.trim();
    if (!name) return;
    try {
      await api.playerAction(serverId, action, name);
      showFlash(t('players.done'), false);
    } catch (e) {
      showFlash(humanizeError(e), true);
    }
  }

  function showFlash(message: string, isErr: boolean) {
    flash = message;
    flashErr = isErr;
    clearTimeout(flashTimer);
    flashTimer = setTimeout(() => (flash = null), 2200);
  }
</script>

<div class="players">
  <header class="head">
    <div>
      <h1>{t('players.title')}</h1>
      {#if live}
        <p class="dim">{t('players.connected')} · {count}{limit ? ` / ${limit}` : ''}</p>
      {/if}
    </div>
    <button class="btn btn--ghost" onclick={load} disabled={loading}>
      {#if loading}<span class="spinner"></span>{/if} {t('common.refresh')}
    </button>
  </header>

  <p class="dim hint">{t('players.offlineHint')}</p>

  <!-- Agir sur un pseudo quelconque (utile pour les joueurs hors ligne). -->
  <div class="card manual">
    <input
      class="input"
      bind:value={manualName}
      placeholder={t('players.namePlaceholder')}
      spellcheck="false"
      autocomplete="off"
    />
    <PlayerActionsMenu
      actions={manualActions}
      label={`${t('players.manual')} ▾`}
      disabled={manualName.trim().length === 0}
      onPick={(a) => act(manualName, a)}
    />
  </div>

  <section class="connected">
    <h2>{t('players.connected')}</h2>
    {#if loading && !live}
      <div class="state"><span class="spinner"></span></div>
    {:else if error}
      <div class="state">
        <p class="alert">{error}</p>
        <button class="btn" onclick={load}>{t('common.retry')}</button>
      </div>
    {:else if players.length === 0}
      <p class="dim empty">{t('players.none')}</p>
    {:else}
      <div class="list">
        {#each players as name (name)}
          <div class="row">
            <span class="avatar" aria-hidden="true">{name.charAt(0).toUpperCase()}</span>
            <span class="name">{name}</span>
            <PlayerActionsMenu actions={connectedActions} onPick={(a) => act(name, a)} />
          </div>
        {/each}
      </div>
    {/if}
  </section>
</div>

{#if flash}
  <div class="toast" class:err={flashErr}>{flash}</div>
{/if}

<style>
  .players {
    max-width: 760px;
    margin: 0 auto;
    padding: 26px 24px 48px;
    display: flex;
    flex-direction: column;
    gap: 18px;
  }
  .head {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    gap: 16px;
  }
  .head h1 {
    margin: 0;
    font-size: 24px;
    letter-spacing: -0.02em;
  }
  .head p {
    margin: 4px 0 0;
    font-size: 13px;
  }
  .hint {
    margin: 0;
    font-size: 12.5px;
  }
  .manual {
    display: flex;
    gap: 10px;
    padding: 12px;
    align-items: center;
  }
  .manual .input {
    flex: 1;
  }
  .connected {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .connected h2 {
    margin: 4px 0 0;
    font-size: 13px;
    text-transform: uppercase;
    letter-spacing: 0.09em;
    color: var(--text-muted);
    font-family: var(--font-mono);
  }
  .state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 14px;
    padding: 32px 0;
    color: var(--text-muted);
  }
  .empty {
    font-size: 13px;
    font-style: italic;
  }
  .list {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
  }
  .row:last-child {
    border-bottom: none;
  }
  .row:hover {
    background: color-mix(in srgb, var(--text) 4%, transparent);
  }
  .avatar {
    width: 30px;
    height: 30px;
    border-radius: 8px;
    flex: none;
    display: grid;
    place-items: center;
    font-family: var(--font-mono);
    font-weight: 700;
    font-size: 13px;
    color: #fff;
    background: var(--brand-gradient);
  }
  .name {
    flex: 1;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .toast {
    position: absolute;
    bottom: 20px;
    left: 50%;
    transform: translateX(-50%);
    background: var(--elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 10px 16px;
    font-size: 13px;
    box-shadow: var(--shadow);
    color: var(--brand-primary);
    z-index: 5;
  }
  .toast.err {
    color: var(--state-danger);
  }
</style>
