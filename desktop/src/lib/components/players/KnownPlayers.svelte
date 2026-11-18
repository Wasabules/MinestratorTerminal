<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';
  import type { KnownPlayer, PlayerAction, PlayerFilter, PlayerSort, PlayersPage } from '#lib/types.ts';
  import PlayerActionsMenu from '../PlayerActionsMenu.svelte';
  import { fmtInt, fmtLastSeen, fmtPlayTime, headUrl } from '#lib/players/format.ts';

  /**
   * Joueurs déjà venus (Minecraft Java) : statistiques, recherche, filtres, tri et pagination.
   * Tout le calcul est fait côté Rust sur la liste complète (gardée une minute) ; ce composant
   * ne fait qu'afficher une page.
   */
  let {
    serverId,
    actionsFor,
    onAct,
    onOpen,
  }: {
    serverId: number;
    /** Actions de modération pertinentes pour un joueur (selon ses drapeaux). */
    actionsFor: (p: KnownPlayer) => PlayerAction[];
    /** Action de modération ; le parent relit la liste ensuite (via `refresh`). */
    onAct: (p: KnownPlayer, action: PlayerAction) => void;
    onOpen: (p: KnownPlayer) => void;
  } = $props();

  const PAGE = 50;
  const SORTS: PlayerSort[] = ['last_seen', 'play_time', 'name'];
  const FILTERS: PlayerFilter[] = ['all', 'online', 'op', 'whitelisted', 'banned'];

  let page = $state<PlayersPage | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let search = $state('');
  let sort = $state<PlayerSort>('last_seen');
  let filter = $state<PlayerFilter>('all');
  let offset = $state(0);
  /** Numéro de la dernière requête : une réponse plus ancienne est ignorée. */
  let seq = 0;
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  const failedHeads = $state<Record<string, true>>({});

  const pages = $derived(page ? Math.max(1, Math.ceil(page.total / PAGE)) : 1);
  const current = $derived(Math.floor(offset / PAGE) + 1);
  const stats = $derived(page?.stats ?? null);

  onMount(() => void load(false));
  onDestroy(() => clearTimeout(searchTimer));

  async function load(refresh: boolean) {
    const mine = ++seq;
    loading = true;
    try {
      const res = await api.knownPlayers(serverId, { search, sort, filter, offset, limit: PAGE }, refresh);
      if (mine !== seq) return;
      page = res;
      offset = res.offset;
      error = null;
    } catch (e) {
      if (mine === seq) error = humanizeError(e);
    } finally {
      if (mine === seq) loading = false;
    }
  }

  /** Relit la liste (cache Rust bypassé) : bouton Actualiser du parent. */
  export function refresh() {
    void load(true);
  }

  function onSearch(value: string) {
    search = value;
    offset = 0;
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => void load(false), 250);
  }

  function setSort(v: PlayerSort) {
    sort = v;
    offset = 0;
    void load(false);
  }

  function setFilter(v: PlayerFilter) {
    filter = v;
    offset = 0;
    void load(false);
  }

  function go(delta: number) {
    offset = Math.max(0, offset + delta * PAGE);
    void load(false);
  }

  function filterCount(f: PlayerFilter): number | null {
    if (!stats) return null;
    switch (f) {
      case 'all':
        return stats.total;
      case 'online':
        return stats.online;
      case 'op':
        return stats.op;
      case 'whitelisted':
        return stats.whitelisted;
      case 'banned':
        return stats.banned;
    }
  }

  function display(p: KnownPlayer): string {
    return p.name ?? t('players.unknownName');
  }
</script>

<section class="known">
  <div class="title-row">
    <div>
      <h2>{t('players.all')}</h2>
      <p class="dim hint">{t('players.allHint')}</p>
    </div>
    {#if loading && page}<span class="spinner"></span>{/if}
  </div>

  {#if stats && stats.total > 0}
    <div class="tiles">
      <div class="tile"><span class="v">{fmtInt(stats.total)}</span><span class="k">{t('players.stats.unique')}</span></div>
      <div class="tile"><span class="v">{fmtInt(stats.active_24h)}</span><span class="k">{t('players.stats.today')}</span></div>
      <div class="tile"><span class="v">{fmtInt(stats.active_7d)}</span><span class="k">{t('players.stats.active7d')}</span></div>
      <div class="tile"><span class="v">{fmtInt(stats.active_30d)}</span><span class="k">{t('players.stats.active30d')}</span></div>
      <div class="tile">
        <span class="v">{fmtPlayTime(stats.total_play_time_secs)}</span><span class="k">{t('players.stats.totalTime')}</span>
      </div>
    </div>
    {#if stats.top_play_time.length > 0}
      <div class="top">
        <span class="top-k dim">{t('players.stats.top')}</span>
        {#each stats.top_play_time as p, i (p.uuid)}
          <button class="top-item" onclick={() => onOpen(p)}>
            <span class="rank">{i + 1}</span>
            <span class="top-name">{display(p)}</span>
            <span class="dim">{fmtPlayTime(p.play_time_secs)}</span>
          </button>
        {/each}
      </div>
    {/if}
  {/if}

  {#if page && !page.playerdata_found}
    <p class="note">{t('players.noPlayerdata')}</p>
  {/if}
  {#if page?.other_worlds}<p class="note">{t('players.otherWorlds')}</p>{/if}
  {#if page?.truncated}<p class="note">{t('players.truncated', { n: fmtInt(page.stats.total) })}</p>{/if}
  {#if page && !page.online_known && page.stats.total > 0}<p class="note dim">{t('players.onlineUnknown')}</p>{/if}

  <div class="controls">
    <input
      class="input search"
      value={search}
      oninput={(e) => onSearch(e.currentTarget.value)}
      placeholder={t('players.search')}
      spellcheck="false"
      autocomplete="off"
    />
    <label class="sort">
      <span class="dim">{t('players.sortBy')}</span>
      <select value={sort} onchange={(e) => setSort(e.currentTarget.value as PlayerSort)}>
        {#each SORTS as s (s)}<option value={s}>{t(`players.sort.${s}`)}</option>{/each}
      </select>
    </label>
  </div>
  <div class="chips" role="group">
    {#each FILTERS as f (f)}
      <button class="chip" class:active={filter === f} onclick={() => setFilter(f)}>
        {t(`players.filter.${f}`)}
        {#if filterCount(f) !== null}<span class="n">{filterCount(f)}</span>{/if}
      </button>
    {/each}
  </div>

  {#if loading && !page}
    <div class="state"><span class="spinner"></span></div>
  {:else if error}
    <div class="state">
      <p class="alert">{error}</p>
      <button class="btn" onclick={() => load(true)}>{t('common.retry')}</button>
    </div>
  {:else if page && page.players.length === 0}
    <p class="dim empty">{page.stats.total === 0 ? t('players.empty') : t('players.noMatch')}</p>
  {:else if page}
    <div class="list">
      <div class="row headrow dim">
        <span></span>
        <span>{t('players.sort.name')}</span>
        <span class="num">{t('players.playTime')}</span>
        <span class="num">{t('players.lastSeen')}</span>
        <span></span>
      </div>
      {#each page.players as p (p.uuid)}
        <div class="row">
          {#if p.name && !failedHeads[p.uuid]}
            <img class="avatar" src={headUrl(p.name)} alt="" loading="lazy" onerror={() => (failedHeads[p.uuid] = true)} />
          {:else}
            <span class="avatar fallback" aria-hidden="true">{display(p).charAt(0).toUpperCase()}</span>
          {/if}
          <button class="who" onclick={() => onOpen(p)} title={t('players.details')}>
            <span class="name" class:unknown={!p.name}>{display(p)}</span>
            {#if p.online}<span class="tag on">{t('players.badge.online')}</span>{/if}
            {#if p.flags.op}<span class="tag op">{t('players.badge.op')}</span>{/if}
            {#if p.flags.whitelisted}<span class="tag wl">{t('players.badge.whitelisted')}</span>{/if}
            {#if p.flags.banned}<span class="tag ban">{t('players.badge.banned')}</span>{/if}
          </button>
          <span class="num">{fmtPlayTime(p.play_time_secs)}</span>
          <span class="num dim">{fmtLastSeen(p.last_seen, p.online)}</span>
          <span class="acts">
            {#if p.name}
              <PlayerActionsMenu actions={actionsFor(p)} onPick={(a) => onAct(p, a)} />
            {/if}
          </span>
        </div>
      {/each}
    </div>
    <div class="pager">
      <span class="dim">{t('players.count', { total: fmtInt(page.total) })}</span>
      {#if pages > 1}
        <span class="pg">
          <button class="btn btn--ghost" disabled={current <= 1 || loading} onclick={() => go(-1)}>{t('players.prev')}</button>
          <span class="dim">{t('players.page', { page: current, pages })}</span>
          <button class="btn btn--ghost" disabled={current >= pages || loading} onclick={() => go(1)}>{t('players.next')}</button>
        </span>
      {/if}
    </div>
  {/if}
</section>

<style>
  .known {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .title-row {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    gap: 12px;
  }
  h2 {
    margin: 4px 0 0;
    font-size: 16px;
    color: var(--text);
    font-weight: 900;
  }
  .hint {
    margin: 4px 0 0;
    font-size: 12.5px;
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(130px, 1fr));
    gap: 10px;
  }
  .tile {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: 12px 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
  }
  .tile .v {
    font-size: 20px;
    font-weight: 800;
    font-variant-numeric: tabular-nums;
  }
  .tile .k {
    font-size: 12px;
    color: var(--text-muted);
  }
  .top {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 10px;
    font-size: 12.5px;
  }
  .top-k {
    font-family: var(--font-mono);
    font-size: 11.5px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .top-item {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font: inherit;
    color: var(--text);
    background: color-mix(in srgb, var(--text) 5%, transparent);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 3px 10px 3px 4px;
    cursor: pointer;
  }
  .top-item:hover {
    border-color: var(--brand-primary);
  }
  .rank {
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    font-size: 11px;
    font-weight: 800;
    color: var(--on-accent);
    background: var(--accent);
  }
  .top-name {
    font-weight: 800;
  }
  .note {
    margin: 0;
    font-size: 12.5px;
    color: var(--state-pending);
  }
  .note.dim {
    color: var(--text-dim);
  }
  .controls {
    display: flex;
    gap: 10px;
    align-items: center;
  }
  .search {
    flex: 1;
    font-family: var(--font-sans);
  }
  .sort {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    white-space: nowrap;
  }
  .sort select {
    font: inherit;
    color: var(--text);
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 8px 10px;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font: inherit;
    font-size: 12.5px;
    color: var(--text-muted);
    background: none;
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 4px 11px;
    cursor: pointer;
  }
  .chip:hover {
    color: var(--text);
  }
  .chip.active {
    color: var(--brand-primary);
    border-color: var(--brand-primary);
    background: color-mix(in srgb, var(--brand-primary) 10%, transparent);
  }
  .chip .n {
    font-variant-numeric: tabular-nums;
    font-size: 11.5px;
    opacity: 0.8;
  }
  .state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 14px;
    padding: 28px 0;
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
    display: grid;
    grid-template-columns: 30px minmax(0, 1fr) 90px 110px 64px;
    align-items: center;
    gap: 12px;
    padding: 8px 12px;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
  }
  .row:last-child {
    border-bottom: none;
  }
  .row:not(.headrow):hover {
    background: color-mix(in srgb, var(--text) 4%, transparent);
  }
  .headrow {
    font-size: 11.5px;
    font-family: var(--font-mono);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding-top: 7px;
    padding-bottom: 7px;
  }
  .avatar {
    width: 30px;
    height: 30px;
    border-radius: 7px;
    image-rendering: pixelated;
  }
  .avatar.fallback {
    display: grid;
    place-items: center;
    font-family: var(--font-mono);
    font-weight: 800;
    font-size: 13px;
    color: var(--on-accent);
    background: var(--accent);
  }
  .who {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    font: inherit;
    color: inherit;
    background: none;
    border: none;
    padding: 0;
    text-align: left;
    cursor: pointer;
  }
  .who:hover .name {
    color: var(--brand-primary);
  }
  .name {
    font-weight: 800;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .name.unknown {
    font-style: italic;
    color: var(--text-muted);
  }
  .tag {
    flex: none;
    font-size: 10.5px;
    font-weight: 800;
    padding: 1px 6px;
    border-radius: 999px;
    border: 1px solid currentColor;
  }
  .tag.on {
    color: var(--state-running);
  }
  .tag.op {
    color: var(--brand-primary);
  }
  .tag.wl {
    color: var(--state-pending);
  }
  .tag.ban {
    color: var(--state-danger);
  }
  .num {
    text-align: right;
    font-size: 13px;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .acts {
    display: flex;
    justify-content: flex-end;
  }
  .pager {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    font-size: 13px;
  }
  .pg {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .pg .btn {
    padding: 6px 12px;
  }
</style>
