<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';
  import type { KnownPlayer, PlayerAction, PlayerInventory, PlayerProfile } from '#lib/types.ts';
  import Icon from '../Icon.svelte';
  import PlayerActionsMenu from '../PlayerActionsMenu.svelte';
  import ItemSlot from './ItemSlot.svelte';
  import {
    fmtDateTime,
    fmtDistance,
    fmtInt,
    fmtLastSeen,
    fmtPlayTime,
    headUrl,
    playerDataError,
  } from '#lib/players/format.ts';

  /**
   * Fiche d'un joueur déjà venu : activité, état, combats, distances et inventaire, en lecture
   * seule (l'édition d'inventaire du panel n'est pas reprise). Ouverte en panneau latéral.
   */
  let {
    serverId,
    player,
    actions,
    onAct,
    onClose,
  }: {
    serverId: number;
    player: KnownPlayer;
    actions: PlayerAction[];
    onAct: (action: PlayerAction) => void;
    onClose: () => void;
  } = $props();

  let profile = $state<PlayerProfile | null>(null);
  let profileError = $state<string | null>(null);
  let inventory = $state<PlayerInventory | null>(null);
  let inventoryError = $state<string | null>(null);
  let loading = $state(true);
  let headFailed = $state(false);
  let copied = $state(false);

  const name = $derived(profile?.name ?? player.name ?? t('players.unknownName'));
  const flags = $derived(profile?.flags ?? player.flags);

  const ARMOR_LABELS = ['helmet', 'chestplate', 'leggings', 'boots'];
  const inventoryEmpty = $derived(
    inventory !== null &&
      inventory.offhand === null &&
      [...inventory.hotbar, ...inventory.main, ...inventory.armor].every((i) => i === null),
  );

  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    void load();
    return () => window.removeEventListener('keydown', onKey);
  });

  async function load() {
    loading = true;
    const [p, inv] = await Promise.allSettled([
      api.playerProfile(serverId, player.uuid),
      api.playerInventory(serverId, player.uuid),
    ]);
    if (p.status === 'fulfilled') profile = p.value;
    else profileError = playerDataError(p.reason);
    if (inv.status === 'fulfilled') inventory = inv.value;
    else inventoryError = playerDataError(inv.reason);
    loading = false;
  }

  async function copyUuid() {
    try {
      await navigator.clipboard.writeText(player.uuid);
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch {
      /* presse-papiers indisponible */
    }
  }

  /** Vie : « 9,5 / 10 ❤ » (2 points par cœur). */
  function hearts(health: number, max: number | null): string {
    const fmt = (n: number) => (n / 2).toLocaleString(undefined, { maximumFractionDigits: 1 });
    return `${fmt(health)} / ${fmt(max ?? 20)} ❤`;
  }

  type Row = [label: string, value: string];

  const activity = $derived.by<Row[]>(() => {
    const p = profile;
    const rows: Row[] = [[t('players.playTime'), fmtPlayTime(p?.play_time_secs ?? player.play_time_secs)]];
    if (p?.sessions != null) rows.push([t('players.sheet.sessions'), fmtInt(p.sessions)]);
    if (p?.first_played != null) rows.push([t('players.sheet.firstPlayed'), fmtDateTime(p.first_played)]);
    if (p?.last_login != null) rows.push([t('players.sheet.lastLogin'), fmtDateTime(p.last_login)]);
    const seen = p?.last_seen ?? player.last_seen;
    rows.push([t('players.lastSeen'), `${fmtLastSeen(seen, player.online)} · ${fmtDateTime(seen)}`]);
    return rows;
  });

  const condition = $derived.by<Row[]>(() => {
    const p = profile;
    if (!p) return [];
    const rows: Row[] = [];
    if (p.game_mode) rows.push([t('players.sheet.gameMode'), t(`players.gameMode.${p.game_mode}`)]);
    if (p.health != null) rows.push([t('players.sheet.health'), hearts(p.health, p.max_health)]);
    if (p.food_level != null) rows.push([t('players.sheet.food'), `${p.food_level} / 20`]);
    if (p.xp_level != null) rows.push([t('players.sheet.xpLevel'), fmtInt(p.xp_level)]);
    return rows;
  });

  const combat = $derived.by<Row[]>(() => {
    const p = profile;
    if (!p) return [];
    const rows: Row[] = [];
    if (p.deaths != null) rows.push([t('players.sheet.deaths'), fmtInt(p.deaths)]);
    if (p.mob_kills != null) rows.push([t('players.sheet.mobKills'), fmtInt(p.mob_kills)]);
    if (p.player_kills != null) rows.push([t('players.sheet.playerKills'), fmtInt(p.player_kills)]);
    return rows;
  });
</script>

<button class="backdrop" aria-label={t('common.close')} onclick={onClose}></button>
<div class="sheet" role="dialog" aria-modal="true" aria-labelledby="ps-title">
  <header class="head">
    {#if player.name && !headFailed}
      <img class="head-img" src={headUrl(name)} alt="" onerror={() => (headFailed = true)} />
    {:else}
      <span class="head-img fallback" aria-hidden="true">{name.charAt(0).toUpperCase()}</span>
    {/if}
    <div class="who">
      <h2 id="ps-title">{name}</h2>
      <button class="uuid" title={t('players.sheet.copyUuid')} onclick={copyUuid}>
        {copied ? t('players.sheet.copied') : player.uuid}
      </button>
      <div class="badges">
        {#if player.online}<span class="tag on">{t('players.badge.online')}</span>{/if}
        {#if flags.op}<span class="tag op">{t('players.badge.op')}</span>{/if}
        {#if flags.whitelisted}<span class="tag wl">{t('players.badge.whitelisted')}</span>{/if}
        {#if flags.banned}<span class="tag ban">{t('players.badge.banned')}</span>{/if}
      </div>
    </div>
    {#if player.name && actions.length > 0}
      <PlayerActionsMenu {actions} onPick={onAct} />
    {/if}
    <button class="x" aria-label={t('common.close')} onclick={onClose}><Icon name="x" size={16} /></button>
  </header>

  <div class="body">
    {#if loading}
      <div class="center"><span class="spinner"></span></div>
    {:else}
      <section>
        <h3>{t('players.sheet.activity')}</h3>
        <dl>
          {#each activity as [k, v] (k)}<dt>{k}</dt><dd>{v}</dd>{/each}
        </dl>
        {#if profileError}<p class="alert small">{profileError}</p>{/if}
      </section>

      {#if condition.length > 0}
        <section>
          <h3>{t('players.sheet.state')}</h3>
          <dl>
            {#each condition as [k, v] (k)}<dt>{k}</dt><dd>{v}</dd>{/each}
          </dl>
        </section>
      {/if}

      {#if combat.length > 0}
        <section>
          <h3>{t('players.sheet.combat')}</h3>
          <dl>
            {#each combat as [k, v] (k)}<dt>{k}</dt><dd>{v}</dd>{/each}
          </dl>
        </section>
      {/if}

      {#if profile && profile.distances.length > 0}
        <section>
          <h3>{t('players.sheet.distances')}</h3>
          <dl>
            {#each profile.distances as d (d.kind)}
              <dt>{t(`players.distance.${d.kind}`)}</dt>
              <dd>{fmtDistance(d.blocks)}</dd>
            {/each}
          </dl>
        </section>
      {/if}

      <section>
        <h3>{t('players.sheet.inventory')}</h3>
        {#if inventoryError}
          <p class="alert small">{inventoryError}</p>
        {:else if inventory}
          {#if inventory.online}
            <p class="note warn"><Icon name="info" size={13} /> {t('players.sheet.onlineWarn')}</p>
          {/if}
          {#if inventoryEmpty}
            <p class="dim small">{t('players.sheet.emptyInventory')}</p>
          {:else}
            <div class="inv">
              <div class="equip">
                <span class="cap dim">{t('players.sheet.armor')}</span>
                <div class="col">
                  {#each inventory.armor as item, i (i)}
                    <ItemSlot {item} label={t(`players.sheet.armorSlot.${ARMOR_LABELS[i]}`)} />
                  {/each}
                </div>
                <span class="cap dim">{t('players.sheet.offhand')}</span>
                <ItemSlot item={inventory.offhand} label={t('players.sheet.offhand')} />
              </div>
              <div class="grids">
                <span class="cap dim">{t('players.sheet.bag')}</span>
                <div class="grid">
                  {#each inventory.main as item, i (i)}<ItemSlot {item} />{/each}
                </div>
                <span class="cap dim">{t('players.sheet.hotbar')}</span>
                <div class="grid hotbar">
                  {#each inventory.hotbar as item, i (i)}<ItemSlot {item} />{/each}
                </div>
              </div>
            </div>
          {/if}
          <p class="note dim">
            {#if inventory.saved_at}{t('players.sheet.savedAt', { date: fmtDateTime(inventory.saved_at) })} · {/if}{t(
              'players.sheet.readOnly',
            )}
          </p>
        {/if}
      </section>
    {/if}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 80;
    border: none;
    cursor: default;
    background: color-mix(in srgb, #000 40%, transparent);
  }
  .sheet {
    position: fixed;
    z-index: 81;
    top: 0;
    right: 0;
    bottom: 0;
    width: min(560px, calc(100vw - 40px));
    background: var(--surface);
    border-left: 1px solid var(--border);
    box-shadow: var(--shadow);
    display: flex;
    flex-direction: column;
  }
  .head {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    padding: 18px 18px 14px;
    border-bottom: 1px solid var(--border);
  }
  .head-img {
    width: 48px;
    height: 48px;
    border-radius: 8px;
    flex: none;
    image-rendering: pixelated;
  }
  .head-img.fallback {
    display: grid;
    place-items: center;
    font-family: var(--font-mono);
    font-weight: 700;
    font-size: 20px;
    color: #fff;
    background: var(--brand-gradient);
  }
  .who {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  h2 {
    margin: 0;
    font-size: 17px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .uuid {
    align-self: flex-start;
    max-width: 100%;
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
    font-family: var(--font-mono);
    font-size: 11.5px;
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .uuid:hover {
    color: var(--text-muted);
  }
  .badges {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
  }
  .tag {
    font-size: 11px;
    font-weight: 700;
    padding: 2px 7px;
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
  .x {
    flex: none;
    background: none;
    border: none;
    color: var(--text-dim);
    cursor: pointer;
    padding: 4px;
    border-radius: 6px;
  }
  .x:hover {
    color: var(--text);
    background: color-mix(in srgb, var(--text) 8%, transparent);
  }
  .body {
    flex: 1;
    overflow-y: auto;
    padding: 6px 18px 24px;
  }
  .center {
    display: grid;
    place-items: center;
    padding: 40px 0;
  }
  section {
    padding: 14px 0 4px;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
  }
  section:last-child {
    border-bottom: none;
  }
  h3 {
    margin: 0 0 10px;
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--text-muted);
    font-family: var(--font-mono);
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 6px 16px;
    margin: 0 0 10px;
    font-size: 13px;
  }
  dt {
    color: var(--text-muted);
  }
  dd {
    margin: 0;
    text-align: right;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }
  .small {
    font-size: 12.5px;
  }
  .note {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    margin: 8px 0;
  }
  .note.warn {
    color: var(--state-pending);
  }
  .inv {
    display: flex;
    gap: 16px;
    align-items: flex-start;
    flex-wrap: wrap;
  }
  .equip,
  .grids {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .col {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .cap {
    font-size: 11px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(9, 40px);
    gap: 4px;
  }
  .grid.hotbar {
    padding-top: 4px;
    border-top: 1px dashed var(--border);
  }
</style>
