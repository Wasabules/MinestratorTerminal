<script lang="ts">
  /**
   * Mods Satisfactory (ficsit.app / SMR) pour un serveur : parcours + tri du catalogue, gestion des
   * mods installés (activer/désactiver/supprimer via SFTP) et installation façon « panier » : la
   * modale d'un mod permet de choisir la version puis « Installer maintenant » ou « Ajouter à la
   * file ». La file se lance en UN lot (un seul arrêt/redémarrage). La progression est suivie
   * globalement dans le bandeau (ModInstallCenter) via le store `installs.svelte.ts`.
   */
  import { onDestroy, onMount } from 'svelte';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { confirm } from '@tauri-apps/plugin-dialog';
  import { api, humanizeError } from '$lib/ipc';
  import { modEvents } from '$lib/events';
  import { t } from '$lib/i18n';
  import { fmtBytes } from '$lib/copilot/format';
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
  import type { FicsitInstalledMod, FicsitMod, FicsitVersion } from '$lib/types';
  import type { ServerTab } from '$lib/tabs/tabs.svelte';
  import Icon from '../Icon.svelte';

  let { tab }: { tab: ServerTab } = $props();
  const serverId = $derived(tab.serverId);

  const LIMIT = 20;

  const SORTS = [
    { key: 'popularity', orderBy: 'popularity', order: 'desc' },
    { key: 'hotness', orderBy: 'hotness', order: 'desc' },
    { key: 'downloads', orderBy: 'downloads', order: 'desc' },
    { key: 'updated', orderBy: 'last_version_date', order: 'desc' },
    { key: 'name', orderBy: 'name', order: 'asc' },
  ] as const;
  let sortKey = $state<string>('popularity');
  const sort = $derived(SORTS.find((s) => s.key === sortKey) ?? SORTS[0]);

  let query = $state('');
  let page = $state(0);
  let mods = $state<FicsitMod[]>([]);
  let count = $state(0);
  let loading = $state(false);
  let error = $state('');

  let installed = $state<FicsitInstalledMod[]>([]);
  let showInstalled = $state(true);
  let busyRef = $state('');

  // Panier + état d'occupation (réactifs via le store global).
  const cart = $derived(cartItems(serverId));
  const busy = $derived(hasActiveRun(serverId));

  // Modale (choix de version d'un mod).
  let modal = $state<FicsitMod | null>(null);
  let versions = $state<FicsitVersion[]>([]);
  let selectedVersion = $state('');
  let vLoading = $state(false);
  let vError = $state('');

  let flash = $state('');
  let flashTimer: ReturnType<typeof setTimeout> | undefined;
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  let unlisten: UnlistenFn | undefined;

  onMount(async () => {
    // Écoute uniquement pour rafraîchir la liste des installés à la fin d'une install (la
    // progression, elle, est affichée globalement dans le bandeau).
    unlisten = await modEvents.installProgress((p) => {
      if (p.status === 'done') void loadInstalled();
    });
    void load();
    void loadInstalled();
  });
  onDestroy(() => {
    clearTimeout(searchTimer);
    clearTimeout(flashTimer);
    unlisten?.();
  });

  async function load() {
    loading = true;
    error = '';
    try {
      const p = await api.ficsitSearch(query.trim(), page * LIMIT, LIMIT, sort.orderBy, sort.order);
      mods = p.mods;
      count = p.count;
    } catch (e) {
      error = humanizeError(e);
      mods = [];
    } finally {
      loading = false;
    }
  }

  async function loadInstalled() {
    try {
      installed = await api.ficsitInstalled(serverId);
    } catch {
      installed = [];
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
  const hasMore = $derived((page + 1) * LIMIT < count);

  function fmtDownloads(n: number): string {
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    if (n >= 1000) return `${(n / 1000).toFixed(0)}k`;
    return `${n}`;
  }
  function isInstalled(ref: string): boolean {
    return installed.some((m) => m.reference === ref);
  }
  function flashMsg(msg: string) {
    flash = msg;
    clearTimeout(flashTimer);
    flashTimer = setTimeout(() => (flash = ''), 3500);
  }

  // --- Modale ---
  async function openInstall(mod: FicsitMod) {
    modal = mod;
    versions = [];
    selectedVersion = '';
    vError = '';
    vLoading = true;
    try {
      versions = await api.ficsitModVersions(mod.id);
      if (versions.length > 0) selectedVersion = versions[0].id;
      else vError = t('ficsit.noVersions');
    } catch (e) {
      vError = humanizeError(e);
    } finally {
      vLoading = false;
    }
  }
  const current = $derived(versions.find((v) => v.id === selectedVersion));
  const deps = $derived((current?.dependencies ?? []).filter((d) => d.mod_id !== 'SML'));
  const serverSize = $derived(
    current?.targets.find((tg) => tg.target_name === 'LinuxServer')?.size ?? 0
  );

  /** Lance une installation par lot et l'enregistre dans le store global (suivi dans le bandeau). */
  function launch(items: { reference: string; versionId: string }[], label: string) {
    const tid = uid();
    startRun(tid, serverId, tab.serverName, label, 0);
    void api.ficsitInstall(serverId, items, tid);
  }

  function addToQueue() {
    if (!modal || !selectedVersion) return;
    addToCart(serverId, {
      reference: modal.mod_reference,
      versionId: selectedVersion,
      name: modal.name,
    });
    modal = null;
  }
  function installNow() {
    if (!modal || !selectedVersion) return;
    const item = { reference: modal.mod_reference, versionId: selectedVersion };
    // Une install déjà en cours sur ce serveur → on met simplement au panier (regroupé ensuite).
    if (busy) {
      addToCart(serverId, { ...item, name: modal.name });
      flashMsg(t('ficsit.queuedBusy'));
      modal = null;
      return;
    }
    launch([item], modal.name);
    modal = null;
  }

  function checkoutCart() {
    const items = cartItems(serverId);
    if (items.length === 0 || hasActiveRun(serverId)) return;
    const label = items.length === 1 ? items[0].name : t('ficsit.nMods', { n: items.length });
    launch(
      items.map((c) => ({ reference: c.reference, versionId: c.versionId })),
      label
    );
    clearCart(serverId);
  }

  // --- Installés ---
  async function toggle(m: FicsitInstalledMod) {
    busyRef = m.reference;
    try {
      await api.ficsitSetEnabled(serverId, m.reference, !m.enabled);
      await loadInstalled();
    } catch (e) {
      error = humanizeError(e);
    } finally {
      busyRef = '';
    }
  }
  async function remove(m: FicsitInstalledMod) {
    const ok = await confirm(t('ficsit.removeConfirm', { name: m.name }), { kind: 'warning' });
    if (!ok) return;
    busyRef = m.reference;
    try {
      await api.ficsitRemove(serverId, m.reference);
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
      <span class="ttl">{t('ficsit.title')}</span>
      <span class="srv dim">{tab.serverName}</span>
    </div>
    <a class="src dim" href="https://ficsit.app" target="_blank" rel="noopener noreferrer">
      ficsit.app <Icon name="external-link" size={12} />
    </a>
  </header>

  <div class="filters">
    <div class="search">
      <Icon name="search" size={15} />
      <input
        type="text"
        bind:value={query}
        oninput={onSearchInput}
        placeholder={t('ficsit.searchPlaceholder')}
      />
    </div>
    <select bind:value={sortKey} onchange={onSortChange} aria-label={t('ficsit.sort')}>
      {#each SORTS as s (s.key)}
        <option value={s.key}>{t(`ficsit.sort_${s.key}`)}</option>
      {/each}
    </select>
    <button
      class="refresh"
      title={t('common.refresh')}
      onclick={() => {
        void load();
        void loadInstalled();
      }}
    >
      <Icon name="refresh-cw" size={15} />
    </button>
  </div>

  {#if cart.length > 0}
    <div class="cart">
      <span class="cart-lbl"><Icon name="package" size={14} /> {t('ficsit.cart')} · {cart.length}</span>
      <div class="cart-chips">
        {#each cart as c (c.reference)}
          <span class="qchip">
            {c.name}
            <button
              class="qx"
              aria-label={t('ficsit.remove')}
              onclick={() => removeFromCart(serverId, c.reference)}><Icon name="x" size={11} /></button
            >
          </span>
        {/each}
      </div>
      <div class="cart-actions">
        <button class="btn ghost sm" onclick={() => clearCart(serverId)}>{t('ficsit.emptyCart')}</button>
        <button class="btn sm" disabled={busy} onclick={checkoutCart}>
          {t('ficsit.installCart', { n: cart.length })}
        </button>
      </div>
      {#if busy}<span class="busy-note dim">{t('ficsit.busyNote')}</span>{/if}
    </div>
  {/if}

  {#if flash}<div class="flash"><Icon name="info" size={14} /> {flash}</div>{/if}

  <div class="content">
    <!-- Installés -->
    <section class="installed">
      <button class="sec-head" onclick={() => (showInstalled = !showInstalled)}>
        <span class="chev">{showInstalled ? '▾' : '▸'}</span>
        <span>{t('ficsit.installed')}</span>
        <span class="count">{installed.length}</span>
      </button>
      {#if showInstalled}
        {#if installed.length === 0}
          <div class="empty dim">{t('ficsit.installedEmpty')}</div>
        {:else}
          <ul class="inst-list">
            {#each installed as m (m.reference)}
              <li class="inst" class:off={!m.enabled}>
                <span class="inm">{m.name}</span>
                {#if !m.enabled}<span class="disabled">{t('ficsit.disabled')}</span>{/if}
                <span class="iactions">
                  <button class="lnk" disabled={busyRef === m.reference} onclick={() => toggle(m)}>
                    {m.enabled ? t('ficsit.disable') : t('ficsit.enable')}
                  </button>
                  <button class="lnk danger" disabled={busyRef === m.reference} onclick={() => remove(m)}>
                    {t('ficsit.remove')}
                  </button>
                </span>
              </li>
            {/each}
          </ul>
        {/if}
      {/if}
    </section>

    <!-- Catalogue -->
    <section class="catalog">
      <div class="sec-head static">
        <span>{t('ficsit.catalog')}</span>
        {#if count}<span class="count">{count}</span>{/if}
      </div>

      {#if error}
        <div class="err">{error}</div>
      {:else if loading}
        <div class="center"><span class="spinner"></span></div>
      {:else if mods.length === 0}
        <div class="empty dim">{t('ficsit.noResults')}</div>
      {:else}
        <div class="grid">
          {#each mods as mod (mod.id)}
            <div class="card">
              <div class="top">
                {#if mod.logo}
                  <img class="ic" src={mod.logo} alt="" loading="lazy" />
                {:else}
                  <span class="ic ph"><Icon name="package" size={20} /></span>
                {/if}
                <div class="meta">
                  <div class="nm">{mod.name}</div>
                  <div class="dl dim">
                    <Icon name="download" size={12} /> {fmtDownloads(mod.downloads)}
                  </div>
                </div>
              </div>
              <p class="tag">{mod.short_description}</p>
              <div class="chips"><span class="chip">{mod.mod_reference}</span></div>
              <div class="actions">
                {#if isInstalled(mod.mod_reference)}
                  <span class="done"><Icon name="check" size={14} /> {t('ficsit.installedOne')}</span>
                {:else if inCart(serverId, mod.mod_reference)}
                  <span class="queued"><Icon name="check" size={14} /> {t('ficsit.inQueue')}</span>
                {:else}
                  <button class="btn install" onclick={() => openInstall(mod)}>
                    <Icon name="download" size={14} /> {t('ficsit.install')}
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

<!-- Modale : choix de version + panier / installer -->
{#if modal}
  <button class="cd-backdrop" aria-label={t('common.close')} onclick={() => (modal = null)}></button>
  <div class="cd-modal card" role="dialog" aria-modal="true" aria-labelledby="fi-title">
    <div class="cd-head">
      <span class="cd-ico"><Icon name="download" size={18} /></span>
      <div class="cd-text">
        <h2 id="fi-title">{modal.name}</h2>
        <p class="cd-ref dim">{modal.mod_reference}</p>
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
        <span class="flab dim">{t('ficsit.version')}</span>
        <select bind:value={selectedVersion}>
          {#each versions as v (v.id)}
            <option value={v.id}>{v.version}</option>
          {/each}
        </select>
      </label>
      {#if current}
        <div class="vmeta">
          <span class="kv"><span class="k dim">{t('ficsit.sml')}</span><span class="mono">{current.sml_version || '—'}</span></span>
          {#if serverSize > 0}
            <span class="kv"><span class="k dim">{t('ficsit.serverBuild')}</span><span class="mono">{fmtBytes(serverSize)}</span></span>
          {/if}
          <span class="kv deps">
            <span class="k dim">{t('ficsit.dependencies')}</span>
            {#if deps.length === 0}
              <span class="dim small">{t('ficsit.depsNone')}</span>
            {:else}
              <span class="depchips">
                {#each deps as d (d.mod_id)}<span class="chip" title={d.condition}>{d.mod_id}</span>{/each}
              </span>
            {/if}
          </span>
        </div>
      {/if}
      <p class="cd-note dim"><Icon name="info" size={13} /> {t('ficsit.stopWarn')}</p>
      <div class="cd-actions">
        <button class="btn btn--ghost" onclick={() => (modal = null)}>{t('common.cancel')}</button>
        <span class="cd-spacer"></span>
        <button class="btn btn--ghost" disabled={!selectedVersion} onclick={addToQueue}>
          {t('ficsit.addToQueue')}
        </button>
        <button class="btn" disabled={!selectedVersion} onclick={installNow}>
          <Icon name="download" size={14} /> {t('ficsit.installNow')}
        </button>
      </div>
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
  .content {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 14px 16px 24px;
  }
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
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
    margin-bottom: 10px;
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
  .btn.sm {
    padding: 6px 11px;
    font-size: 12px;
  }
  .btn.install {
    width: 100%;
  }
  .btn--ghost,
  .btn.ghost {
    background: none;
    border: 1px solid var(--border);
    color: var(--text-muted);
    font-weight: 500;
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
    min-width: 96px;
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
    flex-wrap: wrap;
  }
  .cd-spacer {
    flex: 1;
  }
</style>
