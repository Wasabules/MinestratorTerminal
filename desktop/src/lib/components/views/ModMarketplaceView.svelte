<script lang="ts">
  /**
   * Marketplace de mods GÉNÉRIQUE, une seule vue pour toutes les sources « push SFTP »
   * (Thunderstore/Factorio/uMod…). La source + le jeu viennent des capacités du serveur ; ajouter
   * un jeu = 1 mapping dans `games.rs` + 1 module client backend, cette vue ne change pas.
   * Read-path pour toutes les sources ; installation « façon panier » pour les sources installables
   * (Factorio aujourd'hui), en réutilisant le panier + le gestionnaire globaux (`installs.svelte.ts`).
   */
  import { onDestroy, onMount, untrack } from 'svelte';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { confirm } from '@tauri-apps/plugin-dialog';
  import { api, humanizeError } from '$lib/ipc';
  import { modEvents } from '$lib/events';
  import { t } from '$lib/i18n';
  import { serverCaps } from '$lib/games/capabilities.svelte';
  import { isDebug } from '$lib/debug.svelte';
  import { uid } from '$lib/util/id';
  import {
    addToCart,
    removeFromCart,
    clearCart,
    cartItems,
    inCart,
    hasActiveRun,
    startRun,
  } from '$lib/mods/installs.svelte';
  import type { MarketInstalledMod, MarketMod, MarketModVersion } from '$lib/types';
  import type { ServerTab } from '$lib/tabs/tabs.svelte';
  import Icon from '../Icon.svelte';

  let { tab }: { tab: ServerTab } = $props();
  const serverId = $derived(tab.serverId);
  const caps = $derived(serverCaps(serverId));
  const source = $derived(caps?.mods ?? '');
  const family = $derived(caps?.family ?? '');

  const LIMIT = 20;

  // Sources dont le chemin d'installation est implémenté (déposent + gèrent les mods via SFTP).
  const INSTALLABLE = new Set(['factorio']);

  // Métadonnées par source : libellé, lien, options de tri honnêtes (capacités réelles de l'API).
  type Sort = { key: string; label: string };
  const SOURCE_META: Record<string, { label: string; url: string; sorts: Sort[] }> = {
    thunderstore: {
      label: 'Thunderstore',
      url: 'https://thunderstore.io',
      sorts: [
        { key: 'popular', label: t('mods.sortPopular') },
        { key: 'recent', label: t('mods.sortRecent') },
        { key: 'rating', label: t('mods.sortRating') },
      ],
    },
    factorio: {
      label: 'Factorio Mods',
      url: 'https://mods.factorio.com',
      sorts: [
        { key: 'popular', label: t('mods.sortUpdated') },
        { key: 'recent', label: t('mods.sortNew') },
        { key: 'name', label: t('mods.sortName') },
      ],
    },
    umod: {
      label: 'uMod',
      url: 'https://umod.org',
      sorts: [{ key: 'recent', label: t('mods.sortRecent') }],
    },
  };
  const meta = $derived(
    SOURCE_META[source] ?? { label: source, url: '', sorts: [{ key: 'popular', label: t('mods.sortPopular') }] }
  );

  let sortKey = $state('popular');
  let query = $state('');
  let page = $state(0);
  let mods = $state<MarketMod[]>([]);
  let count = $state(0);
  let hasMore = $state(false);
  let loading = $state(false);
  let error = $state('');
  let factorioTokenSet = $state(true); // vrai par défaut → pas d'avertissement avant vérification

  // Factorio requiert une clé factorio.com valide (sinon parcours seul, boutons guidés vers Réglages).
  const installable = $derived(INSTALLABLE.has(source) && (source !== 'factorio' || factorioTokenSet));

  // Mods installés (sources installables) + panier/occupation (réactifs via le store global).
  let installed = $state<MarketInstalledMod[]>([]);
  let showInstalled = $state(true);
  let busyRef = $state('');
  const cart = $derived(cartItems(serverId));
  const busy = $derived(hasActiveRun(serverId));

  // Modale d'inspection d'un mod (versions).
  let modal = $state<MarketMod | null>(null);
  let versions = $state<MarketModVersion[]>([]);
  let selectedVersion = $state('');
  let vLoading = $state(false);
  let vError = $state('');

  let flash = $state('');
  let flashTimer: ReturnType<typeof setTimeout> | undefined;
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  let unlisten: UnlistenFn | undefined;

  // Charge dès que la source (capacités du serveur) est connue — robuste au démarrage à froid.
  // `untrack` évite que les lectures internes de load() (query/sortKey/page) deviennent des
  // dépendances de l'effet (sinon rechargement à chaque frappe).
  let loadedFor = '';
  $effect(() => {
    if (source && source !== loadedFor) {
      untrack(() => {
        loadedFor = source;
        sortKey = meta.sorts[0]?.key ?? 'popular';
        void load();
        if (INSTALLABLE.has(source)) void loadInstalled();
        // Détection : token Factorio requis pour installer.
        if (source === 'factorio') {
          void api
            .hasFactorioToken()
            .then((b) => (factorioTokenSet = b))
            .catch(() => {});
        }
      });
    }
  });
  onMount(async () => {
    // Rafraîchit la liste des installés en fin d'installation (la progression, elle, est suivie
    // globalement dans le bandeau ModInstallCenter).
    unlisten = await modEvents.installProgress((p) => {
      if (p.status === 'done' && INSTALLABLE.has(source)) void loadInstalled();
    });
  });
  onDestroy(() => {
    clearTimeout(searchTimer);
    clearTimeout(flashTimer);
    unlisten?.();
  });

  async function loadInstalled() {
    try {
      installed = await api.modsInstalled(source, serverId);
    } catch {
      installed = [];
    }
  }
  function isInstalled(ref: string): boolean {
    return installed.some((m) => m.reference === ref);
  }
  function flashMsg(msg: string) {
    flash = msg;
    clearTimeout(flashTimer);
    flashTimer = setTimeout(() => (flash = ''), 3500);
  }

  async function load() {
    if (!source) return;
    loading = true;
    error = '';
    try {
      const p = await api.modsSearch(source, family, query.trim(), sortKey, page + 1);
      mods = p.mods;
      count = p.count;
      hasMore = p.has_more;
    } catch (e) {
      error = humanizeError(e);
      mods = [];
    } finally {
      loading = false;
    }
  }

  function onSearchInput() {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      page = 0;
      void load();
    }, 400);
  }
  function onSortChange() {
    page = 0;
    void load();
  }
  function goPage(delta: number) {
    const next = page + delta;
    if (next < 0) return;
    page = next;
    void load();
  }

  function fmtDownloads(n: number): string {
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    if (n >= 1000) return `${(n / 1000).toFixed(0)}k`;
    return `${n}`;
  }

  async function inspect(mod: MarketMod) {
    modal = mod;
    versions = [];
    selectedVersion = '';
    vError = '';
    vLoading = true;
    try {
      versions = await api.modsVersions(source, mod.reference);
      if (versions.length > 0) selectedVersion = versions[0].version;
      else vError = t('mods.noVersions');
    } catch (e) {
      vError = humanizeError(e);
    } finally {
      vLoading = false;
    }
  }
  const current = $derived(versions.find((v) => v.version === selectedVersion));

  // --- Installation (façon panier) : sources installables uniquement ---
  /** Lance une installation par lot et l'enregistre dans le store global (suivi dans le bandeau). */
  function launch(items: { reference: string; version: string }[], label: string) {
    const tid = uid();
    startRun(tid, serverId, tab.serverName, label, 0);
    void api.modsInstall(serverId, source, items, tid);
  }
  function addToQueue() {
    if (!modal || !selectedVersion) return;
    addToCart(serverId, { reference: modal.reference, versionId: selectedVersion, name: modal.name || modal.reference });
    modal = null;
  }
  function installNow() {
    if (!modal || !selectedVersion) return;
    const item = { reference: modal.reference, version: selectedVersion };
    // Une install déjà en cours sur ce serveur → on met au panier (regroupé ensuite).
    if (busy) {
      addToCart(serverId, { reference: item.reference, versionId: selectedVersion, name: modal.name || modal.reference });
      flashMsg(t('mods.queuedBusy'));
      modal = null;
      return;
    }
    launch([item], modal.name || modal.reference);
    modal = null;
  }
  function checkoutCart() {
    const items = cartItems(serverId);
    if (items.length === 0 || hasActiveRun(serverId)) return;
    const label = items.length === 1 ? items[0].name : t('mods.nMods', { n: items.length });
    launch(
      items.map((c) => ({ reference: c.reference, version: c.versionId })),
      label
    );
    clearCart(serverId);
  }

  async function toggle(m: MarketInstalledMod) {
    busyRef = m.reference;
    try {
      await api.modsSetEnabled(source, serverId, m.reference, !m.enabled);
      await loadInstalled();
    } catch (e) {
      error = humanizeError(e);
    } finally {
      busyRef = '';
    }
  }
  async function remove(m: MarketInstalledMod) {
    const ok = await confirm(t('mods.removeConfirm', { name: m.name }), { kind: 'warning' });
    if (!ok) return;
    busyRef = m.reference;
    try {
      await api.modsRemove(source, serverId, m.reference);
      await loadInstalled();
    } catch (e) {
      error = humanizeError(e);
    } finally {
      busyRef = '';
    }
  }
</script>

<div class="market">
  <header class="bar">
    <div class="left">
      <span class="ico"><Icon name="package" size={18} /></span>
      <span class="ttl">{meta.label}</span>
      <span class="srv dim">{tab.serverName}</span>
      {#if isDebug()}<span class="dbg dim">src={source} · game={family}</span>{/if}
    </div>
    {#if meta.url}
      <a class="src dim" href={meta.url} target="_blank" rel="noopener noreferrer">
        {meta.url.replace('https://', '')} <Icon name="external-link" size={12} />
      </a>
    {/if}
  </header>

  <div class="filters">
    <div class="search">
      <Icon name="search" size={15} />
      <input
        type="text"
        bind:value={query}
        oninput={onSearchInput}
        placeholder={t('mods.searchPlaceholder')}
      />
    </div>
    {#if meta.sorts.length > 1}
      <select bind:value={sortKey} onchange={onSortChange} aria-label={t('mods.sort')}>
        {#each meta.sorts as s (s.key)}
          <option value={s.key}>{s.label}</option>
        {/each}
      </select>
    {/if}
    <button class="refresh" title={t('common.refresh')} onclick={load}>
      <Icon name="refresh-cw" size={15} />
    </button>
  </div>

  <div class="note" class:warn={source === 'thunderstore' || source === 'umod' || (source === 'factorio' && !factorioTokenSet)}>
    <Icon name="info" size={14} />
    <span>
      {#if source === 'thunderstore'}
        {t('mods.warnThunderstore')}
      {:else if source === 'umod'}
        {t('mods.warnUmod')}
      {:else if source === 'factorio' && !factorioTokenSet}
        {t('mods.warnFactorioToken')}
      {:else if source === 'factorio'}
        {t('mods.warnFactorio')}
      {:else}
        {t('mods.readOnlyBanner')}
      {/if}
    </span>
  </div>

  {#if installable && cart.length > 0}
    <div class="cart">
      <span class="cart-lbl"><Icon name="package" size={14} /> {t('mods.cart')} · {cart.length}</span>
      <div class="cart-chips">
        {#each cart as c (c.reference)}
          <span class="qchip">
            {c.name}
            <button class="qx" aria-label={t('mods.remove')} onclick={() => removeFromCart(serverId, c.reference)}>
              <Icon name="x" size={11} />
            </button>
          </span>
        {/each}
      </div>
      <div class="cart-actions">
        <button class="btn ghost sm" onclick={() => clearCart(serverId)}>{t('mods.emptyCart')}</button>
        <button class="btn sm" disabled={busy} onclick={checkoutCart}>
          {t('mods.installCart', { n: cart.length })}
        </button>
      </div>
      {#if busy}<span class="busy-note dim">{t('mods.busyNote')}</span>{/if}
    </div>
  {/if}

  {#if flash}<div class="flash"><Icon name="info" size={14} /> {flash}</div>{/if}

  <div class="content">
    <!-- Installés (sources installables uniquement) -->
    {#if installable}
      <section class="installed">
        <button class="sec-head" onclick={() => (showInstalled = !showInstalled)}>
          <span class="chev">{showInstalled ? '▾' : '▸'}</span>
          <span>{t('mods.installed')}</span>
          <span class="count">{installed.length}</span>
        </button>
        {#if showInstalled}
          {#if installed.length === 0}
            <div class="empty dim">{t('mods.installedEmpty')}</div>
          {:else}
            <ul class="inst-list">
              {#each installed as m (m.reference)}
                <li class="inst" class:off={!m.enabled}>
                  <span class="inm">{m.name}</span>
                  {#if !m.enabled}<span class="disabled">{t('mods.disabled')}</span>{/if}
                  <span class="iactions">
                    <button class="lnk" disabled={busyRef === m.reference} onclick={() => toggle(m)}>
                      {m.enabled ? t('mods.disable') : t('mods.enable')}
                    </button>
                    <button class="lnk danger" disabled={busyRef === m.reference} onclick={() => remove(m)}>
                      {t('mods.remove')}
                    </button>
                  </span>
                </li>
              {/each}
            </ul>
          {/if}
        {/if}
      </section>
    {/if}

    <!-- Catalogue -->
    <section class="catalog">
      {#if installable}
        <div class="sec-head static">
          <span>{t('mods.catalog')}</span>
          {#if count}<span class="count">{count}</span>{/if}
        </div>
      {/if}

      {#if error}
        <div class="err">{error}</div>
      {:else if loading}
        <div class="center"><span class="spinner"></span></div>
      {:else if mods.length === 0}
        <div class="empty dim">{t('mods.noResults')}</div>
      {:else}
        <div class="grid">
          {#each mods as mod (mod.reference)}
            <div class="card" class:open={modal?.reference === mod.reference}>
              <div class="top">
                {#if mod.icon_url}
                  <img class="ic" src={mod.icon_url} alt="" loading="lazy" />
                {:else}
                  <span class="ic ph"><Icon name="package" size={20} /></span>
                {/if}
                <div class="meta">
                  <div class="nm">{mod.name || mod.reference}</div>
                  <div class="dl dim">
                    <Icon name="download" size={12} /> {fmtDownloads(mod.downloads)}
                  </div>
                </div>
              </div>
              <p class="tag">{mod.description}</p>
              <div class="actions">
                {#if installable && isInstalled(mod.reference)}
                  <span class="done"><Icon name="check" size={14} /> {t('mods.installedOne')}</span>
                {:else if installable && inCart(serverId, mod.reference)}
                  <span class="queued"><Icon name="check" size={14} /> {t('mods.inQueue')}</span>
                {:else if installable}
                  <button class="btn install" onclick={() => inspect(mod)}>
                    <Icon name="download" size={14} /> {t('mods.install')}
                  </button>
                {:else}
                  <button class="btn ghost install" onclick={() => inspect(mod)}>
                    <Icon name="package" size={14} /> {t('mods.versions')}
                  </button>
                {/if}
              </div>
            </div>
          {/each}
        </div>

        <div class="pager">
          <button class="btn ghost" disabled={page <= 0} onclick={() => goPage(-1)}>
            {t('market.prev')}
          </button>
          <span class="pg dim">{t('market.page')} {page + 1}</span>
          <button class="btn ghost" disabled={!hasMore} onclick={() => goPage(1)}>
            {t('market.next')}
          </button>
        </div>
      {/if}
    </section>
  </div>
</div>

<!-- Modale : versions + dépendances (installation à venir) -->
{#if modal}
  <button class="cd-backdrop" aria-label={t('common.close')} onclick={() => (modal = null)}></button>
  <div class="cd-modal card" role="dialog" aria-modal="true" aria-labelledby="mm-title">
    <div class="cd-head">
      <span class="cd-ico"><Icon name="package" size={18} /></span>
      <div class="cd-text">
        <h2 id="mm-title">{modal.name || modal.reference}</h2>
        <p class="cd-ref dim">{modal.reference}</p>
      </div>
      <button class="cd-x" aria-label={t('common.close')} onclick={() => (modal = null)}>
        <Icon name="x" size={16} />
      </button>
    </div>

    {#if vLoading}
      <div class="center sm"><span class="spinner"></span></div>
    {:else if vError}
      <div class="err">{vError}</div>
    {:else}
      <label class="fld">
        <span class="flab dim">{t('mods.version')}</span>
        <select bind:value={selectedVersion}>
          {#each versions as v (v.version)}
            <option value={v.version}>{v.version}</option>
          {/each}
        </select>
      </label>
      {#if current}
        <div class="vmeta">
          {#if current.game_version}
            <span class="kv"
              ><span class="k dim">{t('mods.gameVersion')}</span><span class="mono">{current.game_version}</span></span
            >
          {/if}
          <span class="kv deps">
            <span class="k dim">{t('mods.dependencies')}</span>
            {#if current.dependencies.length === 0}
              <span class="dim small">{t('mods.depsNone')}</span>
            {:else}
              <span class="depchips">
                {#each current.dependencies as d (d)}<span class="chip">{d}</span>{/each}
              </span>
            {/if}
          </span>
        </div>
      {/if}
      {#if installable}
        <p class="cd-note dim"><Icon name="info" size={13} /> {t('mods.restartWarn')}</p>
        <div class="cd-actions">
          <button class="btn btn--ghost" onclick={() => (modal = null)}>{t('common.cancel')}</button>
          <span class="cd-spacer"></span>
          <button class="btn btn--ghost" disabled={!selectedVersion} onclick={addToQueue}>
            {t('mods.addToQueue')}
          </button>
          <button class="btn" disabled={!selectedVersion} onclick={installNow}>
            <Icon name="download" size={14} /> {t('mods.installNow')}
          </button>
        </div>
      {:else}
        <p class="cd-note dim"><Icon name="info" size={13} /> {t('mods.installSoon')}</p>
        <div class="cd-actions">
          <button class="btn btn--ghost" onclick={() => (modal = null)}>{t('common.close')}</button>
          <span class="cd-spacer"></span>
          <button class="btn" disabled title={t('mods.installSoon')}>
            <Icon name="download" size={14} /> {t('mods.install')}
          </button>
        </div>
      {/if}
    {/if}
  </div>
{/if}

<style>
  .market {
    height: 100%;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 16px;
    border-bottom: 1px solid var(--border);
    flex: none;
  }
  .left {
    display: flex;
    align-items: center;
    gap: 9px;
    min-width: 0;
  }
  .ico {
    display: inline-flex;
    align-items: center;
    color: var(--brand-primary);
  }
  .ttl {
    font-weight: 700;
    font-size: 14px;
  }
  .srv {
    font-size: 12.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dbg {
    font-family: var(--font-mono);
    font-size: 11px;
    flex: none;
  }
  .src {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: 12px;
    text-decoration: none;
    flex: none;
  }
  .src:hover {
    color: var(--text);
  }
  .filters {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 16px;
    border-bottom: 1px solid var(--border);
    flex: none;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 7px;
    flex: 1;
    min-width: 160px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 0 10px;
    color: var(--text-dim);
  }
  .search input {
    flex: 1;
    background: none;
    border: none;
    outline: none;
    font: inherit;
    font-size: 13px;
    color: var(--text);
    padding: 8px 0;
  }
  select {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--text);
    font: inherit;
    font-size: 12.5px;
    padding: 7px 9px;
    cursor: pointer;
  }
  .refresh {
    display: inline-flex;
    align-items: center;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--text-muted);
    cursor: pointer;
    padding: 7px 9px;
  }
  .refresh:hover {
    color: var(--text);
  }
  .note {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 10px 16px 0;
    padding: 8px 12px;
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--brand-primary) 10%, transparent);
    color: var(--text-muted);
    font-size: 12.5px;
  }
  .note.warn {
    background: color-mix(in srgb, var(--state-pending) 14%, transparent);
    color: color-mix(in srgb, var(--state-pending) 70%, var(--text));
  }
  .content {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 14px 16px 24px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(250px, 1fr));
    gap: 12px;
  }
  .card {
    display: flex;
    flex-direction: column;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    padding: 12px;
  }
  .card.open {
    border-color: var(--brand-primary);
  }
  .top {
    display: flex;
    gap: 10px;
    align-items: center;
  }
  .ic {
    width: 40px;
    height: 40px;
    border-radius: 9px;
    object-fit: cover;
    flex: none;
    background: var(--bg);
  }
  .ic.ph {
    display: grid;
    place-items: center;
    color: var(--text-dim);
    border: 1px solid var(--border);
  }
  .meta {
    min-width: 0;
    flex: 1;
  }
  .nm {
    font-weight: 600;
    font-size: 13.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dl {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 11.5px;
    margin-top: 2px;
  }
  .tag {
    margin: 9px 0;
    font-size: 12px;
    line-height: 1.45;
    color: var(--text-muted);
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
    flex: 1;
  }
  .actions {
    display: flex;
  }
  .btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    background: var(--brand-primary);
    color: #fff;
    border: none;
    border-radius: 7px;
    cursor: pointer;
    font: inherit;
    font-size: 12.5px;
    font-weight: 600;
    padding: 8px 13px;
  }
  .btn:hover:not(:disabled) {
    filter: brightness(1.08);
  }
  .btn:disabled {
    opacity: 0.55;
    cursor: default;
  }
  .btn--ghost,
  .btn.ghost {
    background: none;
    border: 1px solid var(--border);
    color: var(--text-muted);
    font-weight: 500;
  }
  .btn.sm {
    padding: 6px 11px;
    font-size: 12px;
  }
  .btn.install {
    width: 100%;
  }
  .pager {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 14px;
    margin-top: 18px;
  }
  .pg {
    font-size: 12.5px;
    font-variant-numeric: tabular-nums;
  }
  .center {
    display: grid;
    place-items: center;
    padding: 40px;
  }
  .center.sm {
    padding: 24px;
  }
  .spinner {
    width: 22px;
    height: 22px;
    border: 2px solid var(--border);
    border-top-color: var(--brand-primary);
    border-radius: 50%;
    animation: spin 0.7s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .empty {
    padding: 16px 4px;
    font-size: 13px;
  }
  .err {
    padding: 12px;
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--state-danger) 12%, transparent);
    color: var(--state-danger);
    font-size: 13px;
  }
  .chip {
    font-size: 10.5px;
    font-family: var(--font-mono);
    color: var(--text-dim);
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 1px 6px;
  }

  /* Panier */
  .cart {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    padding: 10px 16px;
    border-bottom: 1px solid var(--border);
    background: color-mix(in srgb, var(--brand-primary) 7%, transparent);
  }
  .cart-lbl {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12.5px;
    font-weight: 600;
    flex: none;
  }
  .cart-chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    flex: 1;
    min-width: 120px;
  }
  .qchip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: 11.5px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 2px 5px 2px 10px;
  }
  .qx {
    display: inline-flex;
    align-items: center;
    background: none;
    border: none;
    color: var(--text-dim);
    cursor: pointer;
    padding: 1px;
    border-radius: 50%;
  }
  .qx:hover {
    color: var(--text);
    background: color-mix(in srgb, var(--text) 10%, transparent);
  }
  .cart-actions {
    display: flex;
    gap: 8px;
    flex: none;
  }
  .busy-note {
    width: 100%;
    font-size: 11.5px;
  }
  .flash {
    display: flex;
    align-items: center;
    gap: 7px;
    margin: 10px 16px 0;
    padding: 8px 12px;
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--brand-primary) 10%, transparent);
    color: var(--text-muted);
    font-size: 12.5px;
  }

  /* Section installés + entêtes de section */
  .sec-head {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    background: none;
    border: none;
    cursor: pointer;
    font: inherit;
    font-size: 13px;
    font-weight: 700;
    color: var(--text);
    padding: 4px 0;
    text-align: left;
  }
  .sec-head.static {
    cursor: default;
    margin-bottom: 10px;
  }
  .chev {
    font-size: 11px;
    color: var(--text-dim);
    width: 12px;
  }
  .count {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-dim);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 1px 8px;
  }
  .installed {
    margin-bottom: 18px;
  }
  .inst-list {
    list-style: none;
    margin: 8px 0 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .inst {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 8px;
    border-radius: 6px;
    font-size: 12.5px;
  }
  .inst:nth-child(even) {
    background: color-mix(in srgb, var(--text) 3%, transparent);
  }
  .inst.off {
    opacity: 0.6;
  }
  .inm {
    font-weight: 500;
  }
  .disabled {
    font-size: 10.5px;
    color: var(--state-pending);
  }
  .iactions {
    margin-left: auto;
    display: flex;
    gap: 10px;
  }
  .lnk {
    background: none;
    border: none;
    color: var(--brand-primary);
    cursor: pointer;
    font: inherit;
    font-size: 12px;
    padding: 0;
    text-decoration: underline;
  }
  .lnk.danger {
    color: var(--state-danger);
  }
  .lnk:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .done,
  .queued {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12.5px;
    font-weight: 600;
    padding: 8px 0;
  }
  .done {
    color: var(--state-running);
  }
  .queued {
    color: var(--brand-primary);
  }

  /* Modale */
  .cd-backdrop {
    position: fixed;
    inset: 0;
    z-index: 90;
    border: none;
    cursor: default;
    background: color-mix(in srgb, #000 45%, transparent);
    backdrop-filter: blur(2px);
  }
  .cd-modal {
    position: fixed;
    z-index: 91;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: min(460px, calc(100vw - 40px));
    padding: 18px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .cd-head {
    display: flex;
    gap: 12px;
    align-items: flex-start;
  }
  .cd-ico {
    flex: none;
    display: grid;
    place-items: center;
    width: 34px;
    height: 34px;
    border-radius: 9px;
    color: var(--brand-primary);
    background: color-mix(in srgb, var(--brand-primary) 14%, transparent);
  }
  .cd-text {
    min-width: 0;
    flex: 1;
  }
  h2 {
    margin: 1px 0 0;
    font-size: 15.5px;
  }
  .cd-ref {
    margin: 3px 0 0;
    font-family: var(--font-mono);
    font-size: 11.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cd-x {
    flex: none;
    background: none;
    border: none;
    color: var(--text-dim);
    cursor: pointer;
    padding: 2px;
    border-radius: 6px;
  }
  .cd-x:hover {
    color: var(--text);
    background: color-mix(in srgb, var(--text) 8%, transparent);
  }
  .fld {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .flab {
    font-size: 12px;
  }
  .fld select {
    width: 100%;
  }
  .vmeta {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .kv {
    display: flex;
    align-items: baseline;
    gap: 8px;
    font-size: 12.5px;
  }
  .kv .k {
    min-width: 110px;
    flex: none;
  }
  .mono {
    font-family: var(--font-mono);
    font-size: 12px;
  }
  .deps {
    align-items: flex-start;
  }
  .depchips {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
  }
  .small {
    font-size: 11.5px;
  }
  .cd-note {
    margin: 0;
    font-size: 12px;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .cd-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .cd-spacer {
    flex: 1;
  }
</style>
