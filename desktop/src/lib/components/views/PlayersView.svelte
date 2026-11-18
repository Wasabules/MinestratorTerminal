<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';
  import type { KnownPlayer, LiveLight, PlayerAction } from '#lib/types.ts';
  import type { ServerTab } from '#lib/tabs/tabs.svelte.ts';
  import { serverCaps } from '#lib/games/capabilities.svelte.ts';
  import PlayerActionsMenu from '../PlayerActionsMenu.svelte';
  import KnownPlayers from '../players/KnownPlayers.svelte';
  import PlayerSheet from '../players/PlayerSheet.svelte';

  let { tab }: { tab: ServerTab } = $props();
  const serverId = $derived(tab.serverId);
  /** Joueurs déjà venus, fiche et inventaire : Minecraft Java seulement (`GameCapabilities`). */
  const playerData = $derived(serverCaps(serverId)?.player_data ?? false);

  let live = $state<LiveLight | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let manualName = $state('');
  let flash = $state<string | null>(null);
  let flashErr = $state(false);
  let flashTimer: ReturnType<typeof setTimeout> | undefined;
  let reloadTimer: ReturnType<typeof setTimeout> | undefined;
  /** Joueur dont la fiche est ouverte. */
  let sheet = $state<KnownPlayer | null>(null);
  let known = $state<KnownPlayers | undefined>();

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

  /** Actions d'un joueur déjà venu, selon son état : on ne propose que ce qui changerait quelque chose. */
  function actionsFor(p: KnownPlayer): PlayerAction[] {
    const out: PlayerAction[] = [];
    if (p.online) out.push('kick');
    out.push(p.flags.banned ? 'unban' : 'ban');
    out.push(p.flags.op ? 'op_remove' : 'op_add');
    out.push(p.flags.whitelisted ? 'whitelist_remove' : 'whitelist_add');
    return out;
  }

  onMount(load);
  onDestroy(() => {
    clearTimeout(flashTimer);
    clearTimeout(reloadTimer);
  });

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

  function refreshAll() {
    void load();
    known?.refresh();
  }

  async function act(player: string, action: PlayerAction): Promise<boolean> {
    const name = player.trim();
    if (!name) return false;
    try {
      await api.playerAction(serverId, action, name);
      showFlash(t('players.done'), false);
      // Drapeaux relus par l'hébergeur dans ops.json & co. : on lui laisse le temps de les écrire.
      if (playerData) {
        clearTimeout(reloadTimer);
        reloadTimer = setTimeout(() => known?.refresh(), 1500);
      }
      return true;
    } catch (e) {
      showFlash(humanizeError(e), true);
      return false;
    }
  }

  /** Action sur un joueur déjà venu (liste ou fiche). */
  async function actOn(p: KnownPlayer, action: PlayerAction) {
    if (!p.name) return;
    const ok = await act(p.name, action);
    // La fiche ouverte montre les drapeaux d'avant l'action : on la ferme plutôt que de mentir.
    if (ok && sheet?.uuid === p.uuid) sheet = null;
  }

  /** Fiche d'un joueur connecté : retrouvé par son pseudo parmi les joueurs déjà venus. */
  async function openByName(name: string) {
    try {
      const res = await api.knownPlayers(serverId, {
        search: name,
        sort: 'name',
        filter: 'all',
        offset: 0,
        limit: 20,
      });
      const found = res.players.find((p) => p.name?.toLowerCase() === name.toLowerCase());
      if (found) sheet = found;
      else showFlash(t('players.dataErrors.NOT_FOUND'), true);
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
    <button class="btn btn--ghost" onclick={refreshAll} disabled={loading}>
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
            {#if playerData}
              <button class="link" onclick={() => openByName(name)}>{t('players.details')}</button>
            {/if}
            <PlayerActionsMenu actions={connectedActions} onPick={(a) => act(name, a)} />
          </div>
        {/each}
      </div>
    {/if}
  </section>

  {#if playerData}
    <KnownPlayers bind:this={known} {serverId} {actionsFor} onAct={actOn} onOpen={(p) => (sheet = p)} />
  {/if}
</div>

{#if sheet}
  {#key sheet.uuid}
    <PlayerSheet
      {serverId}
      player={sheet}
      actions={actionsFor(sheet)}
      onAct={(a) => sheet && actOn(sheet, a)}
      onClose={() => (sheet = null)}
    />
  {/key}
{/if}

{#if flash}
  <div class="toast" class:err={flashErr}>{flash}</div>
{/if}

<style>
  .players {
    max-width: 900px;
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
    font-size: 16px;
    color: var(--text);
    font-weight: 900;
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
    font-weight: 800;
    font-size: 13px;
    color: var(--on-accent);
    background: var(--accent);
  }
  .name {
    flex: 1;
    font-weight: 800;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .link {
    background: none;
    border: none;
    font: inherit;
    font-size: 13px;
    color: var(--brand-primary);
    cursor: pointer;
    padding: 4px 6px;
    border-radius: 6px;
  }
  .link:hover {
    background: color-mix(in srgb, var(--brand-primary) 10%, transparent);
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
    z-index: 90;
  }
  .toast.err {
    color: var(--state-danger);
  }
</style>
