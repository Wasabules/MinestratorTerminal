<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';
  import { fmtBytes } from '#lib/copilot/format.ts';
  import type { ServerTab } from '#lib/tabs/tabs.svelte.ts';
  import type {
    ContentUpdate,
    InstalledItem,
    MarketItem,
    MarketKind,
    MarketSource,
    MarketVersion,
    UpdateReport,
  } from '#lib/types.ts';
  import { applyUpdates, loadUpdates, serverUpdates, updateFor } from '#lib/mods/updates.svelte.ts';
  import Icon from '../Icon.svelte';

  let { tab }: { tab: ServerTab } = $props();
  const serverId = $derived(tab.serverId);

  // Sources disponibles selon le type de contenu, et loaders proposés.
  const SOURCES: Record<MarketKind, MarketSource[]> = {
    mods: ['modrinth', 'curseforge'],
    plugins: ['modrinth', 'spigot'],
  };
  const LOADERS: Record<MarketKind, string[]> = {
    mods: ['fabric', 'neoforge', 'forge', 'quilt'],
    plugins: ['paper', 'spigot', 'purpur', 'folia', 'bukkit'],
  };
  const SOURCE_LABEL: Record<string, string> = {
    modrinth: 'Modrinth',
    curseforge: 'CurseForge',
    spigot: 'SpigotMC',
  };

  let kind = $state<MarketKind>('mods');
  let source = $state<MarketSource>('modrinth');
  let loader = $state('fabric');
  let gameVersion = $state('');
  let query = $state('');
  let page = $state(1);

  let items = $state<MarketItem[]>([]);
  let totalHits = $state<number | null>(null);
  let loading = $state(false);
  let error = $state('');

  let mcVersions = $state<string[]>([]);
  let installed = $state<InstalledItem[]>([]);
  let showInstalled = $state(true);

  // Sélecteur de version + installation (un seul item à la fois).
  let openId = $state<string | null>(null);
  let versions = $state<MarketVersion[]>([]);
  let selectedVersion = $state('');
  let vLoading = $state(false);
  let vError = $state('');
  let installing = $state(false);
  let installedIds = $state<Set<string>>(new Set());
  let flash = $state('');

  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  let flashTimer: ReturnType<typeof setTimeout> | undefined;

  // Les timers différés (debounce de recherche, effacement du flash) ne doivent pas survivre au
  // démontage : sinon ils écrivent dans un état détruit.
  onDestroy(() => {
    clearTimeout(searchTimer);
    clearTimeout(flashTimer);
  });

  onMount(async () => {
    // Vérification des mises à jour en parallèle (résultat partagé avec l'Aperçu, gardé 15 min).
    void loadUpdates(serverId);
    // Détection auto mods/plugins + loader depuis ce qui est déjà installé.
    const [mods, plugins, versionsList, packs] = await Promise.all([
      api.installedMods(serverId).catch(() => [] as InstalledItem[]),
      api.installedPlugins(serverId).catch(() => [] as InstalledItem[]),
      api.marketMinecraftVersions().catch(() => [] as string[]),
      api.installedDatapacks(serverId).catch(() => [] as InstalledItem[]),
    ]);
    mcVersions = versionsList;
    datapacks = packs;
    if (mods.length > 0) {
      kind = 'mods';
      loader = normalizeLoader(mods[0].loader) ?? 'fabric';
    } else if (plugins.length > 0) {
      kind = 'plugins';
      loader = 'paper';
    }
    source = 'modrinth';
    installed = kind === 'mods' ? mods : plugins;
    await load();
  });

  function normalizeLoader(l: string): string | null {
    const x = l.toLowerCase();
    return LOADERS.mods.includes(x) || LOADERS.plugins.includes(x) ? x : null;
  }

  async function loadInstalled() {
    try {
      installed = kind === 'mods' ? await api.installedMods(serverId) : await api.installedPlugins(serverId);
    } catch {
      installed = [];
    }
  }

  // --- Mises à jour (mods, plugins, datapacks) -----------------------------

  let datapacks = $state<InstalledItem[]>([]);
  let showDatapacks = $state(false);
  const upd = $derived(serverUpdates(serverId));
  const updates = $derived(upd.data?.updates ?? []);
  /** « Tout mettre à jour » : les éléments actifs (un désactivé serait réinstallé actif). */
  const bulk = $derived(updates.filter((u) => u.enabled));
  /** Mises à jour en attente de confirmation. */
  let confirming = $state<ContentUpdate[] | null>(null);
  let applying = $state(false);
  let report = $state<UpdateReport | null>(null);
  let pendingRestart = $state(false);
  let restarting = $state(false);
  let restartError = $state('');
  let applyError = $state('');

  const checkedAt = $derived(
    upd.data
      ? new Date(upd.data.checked_at * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
      : ''
  );
  /** Bilan à détailler : tout sauf les réussites (résumées en une ligne). */
  const problems = $derived(report?.outcomes.filter((o) => o.status !== 'updated') ?? []);
  const updatedCount = $derived(report?.outcomes.filter((o) => o.status === 'updated').length ?? 0);

  async function confirmApply() {
    if (!confirming || applying) return;
    applying = true;
    report = null;
    applyError = '';
    try {
      const items = confirming.map((u) => ({ kind: u.kind, key: u.key }));
      report = await applyUpdates(serverId, items);
      if (report.restart_needed) pendingRestart = true;
      confirming = null;
      void loadInstalled();
      if (items.some((i) => i.kind === 'datapacks')) {
        datapacks = await api.installedDatapacks(serverId).catch(() => datapacks);
      }
    } catch (e) {
      applyError = humanizeError(e);
    } finally {
      applying = false;
    }
  }

  async function restartServer() {
    restarting = true;
    restartError = '';
    try {
      await api.powerAction(serverId, 'restart');
      pendingRestart = false;
    } catch (e) {
      restartError = humanizeError(e);
    } finally {
      restarting = false;
    }
  }

  async function load() {
    loading = true;
    error = '';
    openId = null;
    try {
      const p = await api.marketList(kind, source, page, query.trim(), loader, gameVersion);
      items = p.items;
      totalHits = p.total_hits;
    } catch (e) {
      error = humanizeError(e);
      items = [];
    } finally {
      loading = false;
    }
  }

  function setKind(k: MarketKind) {
    if (k === kind) return;
    kind = k;
    source = 'modrinth';
    loader = LOADERS[k][0];
    page = 1;
    void loadInstalled();
    void load();
  }

  function onFilterChange() {
    page = 1;
    void load();
  }

  function onSearchInput() {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      page = 1;
      void load();
    }, 400);
  }

  function goPage(delta: number) {
    const next = page + delta;
    if (next < 1) return;
    page = next;
    void load();
  }

  const hasMore = $derived(
    totalHits != null ? page * 18 < totalHits : items.length >= 18
  );

  async function openInstall(item: MarketItem) {
    if (openId === item.id) {
      openId = null;
      return;
    }
    openId = item.id;
    versions = [];
    selectedVersion = '';
    vError = '';
    vLoading = true;
    try {
      const key = source === 'modrinth' ? item.slug || item.id : item.id;
      versions = await api.marketVersions(source, key, loader, gameVersion);
      if (versions.length > 0) selectedVersion = versions[0].id;
      else vError = t('market.noVersions');
    } catch (e) {
      vError = humanizeError(e);
    } finally {
      vLoading = false;
    }
  }

  async function confirmInstall(item: MarketItem) {
    if (!selectedVersion) return;
    installing = true;
    vError = '';
    try {
      await api.installMod(
        serverId,
        source,
        kind === 'plugins' ? 'plugin' : 'mod',
        item.slug || item.id,
        selectedVersion,
        loader
      );
      installedIds = new Set(installedIds).add(item.id);
      openId = null;
      flash = t('market.installOk', { name: item.name });
      clearTimeout(flashTimer);
      flashTimer = setTimeout(() => (flash = ''), 4000);
      void loadInstalled();
    } catch (e) {
      vError = humanizeError(e);
    } finally {
      installing = false;
    }
  }

  function fmtDownloads(n: number): string {
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    if (n >= 1000) return `${(n / 1000).toFixed(0)}k`;
    return `${n}`;
  }

  // Modrinth & SpigotMC : installation directe. CurseForge : navigation + lien externe.
  const canInstall = $derived(source === 'modrinth' || source === 'spigot');
</script>

<div class="market">
  <header class="bar">
    <div class="left">
      <span class="ico"><Icon name="package" size={18} /></span>
      <span class="ttl">{t('market.title')}</span>
      <span class="srv dim">{tab.serverName}</span>
    </div>
    <div class="seg" role="group" aria-label={t('market.kind')}>
      <button class:on={kind === 'mods'} onclick={() => setKind('mods')}>{t('market.mods')}</button>
      <button class:on={kind === 'plugins'} onclick={() => setKind('plugins')}>{t('market.plugins')}</button>
    </div>
  </header>

  <div class="filters">
    <div class="search">
      <Icon name="search" size={15} />
      <input
        type="text"
        bind:value={query}
        oninput={onSearchInput}
        placeholder={t('market.searchPlaceholder')}
      />
    </div>
    <select bind:value={source} onchange={onFilterChange} aria-label={t('market.source')}>
      {#each SOURCES[kind] as s (s)}
        <option value={s}>{SOURCE_LABEL[s]}</option>
      {/each}
    </select>
    <select bind:value={loader} onchange={onFilterChange} aria-label={t('market.loader')}>
      {#each LOADERS[kind] as l (l)}
        <option value={l}>{l}</option>
      {/each}
    </select>
    <select bind:value={gameVersion} onchange={onFilterChange} aria-label={t('market.gameVersion')}>
      <option value="">{t('market.allVersions')}</option>
      {#each mcVersions as v (v)}
        <option value={v}>{v}</option>
      {/each}
    </select>
    <button class="refresh" title={t('common.refresh')} onclick={load}><Icon name="refresh-cw" size={15} /></button>
  </div>

  {#if flash}
    <div class="flash"><Icon name="check" size={15} /> {flash}</div>
  {/if}

  <div class="content">
    {#if pendingRestart}
      <div class="notice" role="status">
        <Icon name="info" size={16} />
        <span>{t('market.updates.pendingRestart')}</span>
        <button class="btn" onclick={restartServer} disabled={restarting}>
          {restarting ? t('market.updates.restarting') : t('market.updates.restartNow')}
        </button>
      </div>
    {/if}
    {#if restartError}<div class="err">{restartError}</div>{/if}

    <!-- Mises à jour des mods, plugins et datapacks installés -->
    <section class="updates">
      <div class="sec-head static">
        <span>{t('market.updates.title')}</span>
        {#if updates.length > 0}<span class="count accent">{updates.length}</span>{/if}
        {#if upd.data?.mc_version}
          <span class="upd-meta dim">
            {t('market.updates.checkedFor', { version: upd.data.mc_version, time: checkedAt })}
          </span>
        {/if}
        <span class="grow"></span>
        <button class="btn ghost sm" onclick={() => loadUpdates(serverId, true)} disabled={upd.loading || applying}>
          <Icon name="refresh-cw" size={13} />
          {upd.loading ? t('market.updates.checking') : t('market.updates.check')}
        </button>
        {#if bulk.length > 0}
          <button class="btn sm" onclick={() => (confirming = bulk)} disabled={applying}>
            <Icon name="download" size={13} />
            {t('market.updates.updateAll', { count: bulk.length })}
          </button>
        {/if}
      </div>

      {#if confirming}
        <div class="confirm" role="alertdialog" aria-label={t('market.updates.confirmTitle')}>
          <p class="c-title">
            {confirming.length === 1
              ? t('market.updates.confirmOne', { name: confirming[0].name, version: confirming[0].new_version })
              : t('market.updates.confirmMany', { count: confirming.length })}
          </p>
          {#if confirming.length > 1}
            <ul class="c-list">
              {#each confirming as u (u.kind + u.key)}
                <li>{u.name} <span class="dim mono">{u.current_version || '?'} → {u.new_version}</span></li>
              {/each}
            </ul>
          {/if}
          <p class="dim small">{t('market.updates.confirmHint')}</p>
          {#if confirming.some((u) => !u.enabled)}
            <p class="warn small">{t('market.updates.reenableWarn')}</p>
          {/if}
          <div class="c-actions">
            <button class="btn confirm-btn" onclick={confirmApply} disabled={applying}>
              {applying ? t('market.updates.applying') : t('market.updates.confirm')}
            </button>
            <button class="btn ghost" onclick={() => (confirming = null)} disabled={applying}>{t('common.cancel')}</button>
          </div>
        </div>
      {/if}

      {#if applyError}<div class="err">{applyError}</div>{/if}
      {#if report}
        <div class="report">
          {#if updatedCount > 0}
            <p class="ok small"><Icon name="check" size={14} /> {t('market.updates.done', { count: updatedCount })}</p>
          {/if}
          {#each problems as o (o.kind + o.key)}
            <p class="small" class:warn={o.status === 'old_kept'} class:errtxt={o.status === 'failed'}>
              {#if o.status === 'old_kept'}
                {t('market.updates.oldKept', { name: o.name, file: o.key })}
              {:else if o.status === 'failed'}
                {t('market.updates.failed', { name: o.name })}{o.error ? ` — ${humanizeError(o.error)}` : ''}
              {:else}
                {t('market.updates.skipped', { name: o.name })} — {t(`market.updates.reason.${o.reason ?? 'up_to_date'}`)}
              {/if}
            </p>
          {/each}
        </div>
      {/if}

      {#if upd.error}
        <div class="err">{upd.error}</div>
      {:else if !upd.data}
        <div class="empty dim">{upd.loading ? t('market.updates.checking') : ''}</div>
      {:else if !upd.data.mc_version}
        <div class="empty dim">{t('market.updates.noVersion')}</div>
      {:else}
        {#if updates.length === 0}
          <div class="empty dim">{t('market.updates.none')}</div>
        {:else}
          <ul class="upd-list">
            {#each updates as u (u.kind + u.key)}
              <li class="upd" class:off={!u.enabled}>
                <span class="chip">{t(`market.updates.kind.${u.kind}`)}</span>
                <span class="inm">{u.name}</span>
                <span class="iv dim">{u.current_version || '?'} → <strong>{u.new_version}</strong></span>
                {#if !u.enabled}<span class="disabled">{t('market.disabled')}</span>{/if}
                <button class="btn sm push" onclick={() => (confirming = [u])} disabled={applying}>
                  {t('market.updates.update')}
                </button>
              </li>
            {/each}
          </ul>
        {/if}
        {#if upd.data.ignored.length > 0}
          <details class="ignored">
            <summary class="dim small">{t('market.updates.ignored', { count: upd.data.ignored.length })}</summary>
            <ul>
              {#each upd.data.ignored as i (i.kind + i.key)}
                <li class="small">
                  {i.name} <span class="dim mono">{i.current_version || '?'} → {i.new_version}</span>
                  — {t(`market.updates.reason.${i.reason}`)}
                </li>
              {/each}
            </ul>
          </details>
        {/if}
        {#if upd.data.untracked > 0}
          <p class="dim small">{t('market.updates.untracked', { count: upd.data.untracked })}</p>
        {/if}
        {#each upd.data.errors as e (e.kind)}
          <p class="errtxt small">{t(`market.updates.kind.${e.kind}`)} — {humanizeError(e.error)}</p>
        {/each}
      {/if}
    </section>

    <!-- Installés -->
    <section class="installed">
      <button class="sec-head" onclick={() => (showInstalled = !showInstalled)}>
        <span class="chev">{showInstalled ? '▾' : '▸'}</span>
        <span>{t('market.installed')}</span>
        <span class="count">{installed.length}</span>
      </button>
      {#if showInstalled}
        {#if installed.length === 0}
          <div class="empty dim">{t('market.installedEmpty')}</div>
        {:else}
          <ul class="inst-list">
            {#each installed as m (m.filename || m.name)}
              {@const u = updateFor(serverId, kind, m.filename)}
              <li class="inst" class:off={!m.enabled}>
                <span class="inm">{m.name}</span>
                {#if m.version}<span class="iv dim">{m.version}</span>{/if}
                {#if m.loader}<span class="il dim">{m.loader}</span>{/if}
                {#if !m.enabled}<span class="disabled">{t('market.disabled')}</span>{/if}
                {#if u}
                  <button
                    class="upbadge"
                    title={t('market.updates.available', { version: u.new_version })}
                    onclick={() => (confirming = [u])}
                    disabled={applying}
                  >
                    ↑ {u.new_version}
                  </button>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
      {/if}
    </section>

    {#if datapacks.length > 0}
      <!-- Datapacks du monde actif (pas de catalogue ici : seulement leurs mises à jour). -->
      <section class="installed">
        <button class="sec-head" onclick={() => (showDatapacks = !showDatapacks)}>
          <span class="chev">{showDatapacks ? '▾' : '▸'}</span>
          <span>{t('market.updates.datapacks')}</span>
          <span class="count">{datapacks.length}</span>
        </button>
        {#if showDatapacks}
          <ul class="inst-list">
            {#each datapacks as d (d.filename)}
              {@const u = updateFor(serverId, 'datapacks', d.filename)}
              <li class="inst">
                <span class="inm">{d.name}</span>
                {#if u}
                  <button
                    class="upbadge"
                    title={t('market.updates.available', { version: u.new_version })}
                    onclick={() => (confirming = [u])}
                    disabled={applying}
                  >
                    ↑ {u.new_version}
                  </button>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    {/if}

    <!-- Catalogue -->
    <section class="catalog">
      <div class="sec-head static">
        <span>{t('market.catalog')}</span>
        {#if totalHits != null}<span class="count">{totalHits}</span>{/if}
      </div>

      {#if error}
        <div class="err">{error}</div>
      {:else if loading}
        <div class="center"><span class="spinner"></span></div>
      {:else if items.length === 0}
        <div class="empty dim">{t('market.noResults')}</div>
      {:else}
        <div class="grid">
          {#each items as item (item.source + item.id)}
            <div class="card" class:open={openId === item.id}>
              <div class="top">
                {#if item.icon_url}
                  <img class="ic" src={item.icon_url} alt="" loading="lazy" />
                {:else}
                  <span class="ic ph"><Icon name="package" size={20} /></span>
                {/if}
                <div class="meta">
                  <div class="nm">
                    {item.name}
                    {#if item.premium}<span class="prem">{t('market.premium')}</span>{/if}
                  </div>
                  <div class="dl dim"><Icon name="download" size={12} /> {fmtDownloads(item.downloads)}</div>
                </div>
              </div>
              <p class="tag">{item.tag}</p>
              <div class="chips">
                {#each item.loaders.slice(0, 3) as l (l)}<span class="chip">{l}</span>{/each}
                {#if item.game_versions.length > 0}<span class="chip gv">{item.game_versions[0]}</span>{/if}
              </div>

              <div class="actions">
                {#if canInstall && !item.premium}
                  {#if installedIds.has(item.id)}
                    <span class="done"><Icon name="check" size={14} /> {t('market.installedOne')}</span>
                  {:else}
                    <button class="btn install" onclick={() => openInstall(item)}>
                      <Icon name="download" size={14} /> {t('market.install')}
                    </button>
                  {/if}
                {:else if item.external_url}
                  <a class="btn ext" href={item.external_url} target="_blank" rel="noopener noreferrer">
                    <Icon name="external-link" size={14} /> {t('market.viewOn')} {SOURCE_LABEL[item.source] ?? item.source}
                  </a>
                {:else}
                  <span class="dim small">{t('market.installOnlyModrinth')}</span>
                {/if}
              </div>

              {#if openId === item.id}
                <div class="picker">
                  {#if vLoading}
                    <span class="dim small">{t('market.loadingVersions')}</span>
                  {:else if vError}
                    <span class="err small">{vError}</span>
                  {:else}
                    <select bind:value={selectedVersion} aria-label={t('market.version')}>
                      {#each versions as v (v.id)}
                        <option value={v.id}>{v.version_number || v.name || v.id}{v.release_type ? ` · ${v.release_type}` : ''}{v.file_size ? ` · ${fmtBytes(v.file_size)}` : ''}</option>
                      {/each}
                    </select>
                    <button class="btn confirm" disabled={installing || !selectedVersion} onclick={() => confirmInstall(item)}>
                      {installing ? t('market.installing') : t('market.confirm')}
                    </button>
                    <button class="btn ghost" onclick={() => (openId = null)}>{t('common.cancel')}</button>
                  {/if}
                </div>
              {/if}
            </div>
          {/each}
        </div>

        <div class="pager">
          <button class="btn ghost" disabled={page <= 1} onclick={() => goPage(-1)}>{t('market.prev')}</button>
          <span class="pg dim">{t('market.page')} {page}</span>
          <button class="btn ghost" disabled={!hasMore} onclick={() => goPage(1)}>{t('market.next')}</button>
        </div>
      {/if}
    </section>
  </div>
</div>

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
  .seg {
    display: inline-flex;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 2px;
    flex: none;
  }
  .seg button {
    background: none;
    border: none;
    cursor: pointer;
    font: inherit;
    font-size: 12.5px;
    color: var(--text-muted);
    padding: 5px 12px;
    border-radius: 6px;
  }
  .seg button.on {
    background: var(--brand-primary);
    color: #fff;
    font-weight: 600;
  }
  .filters {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 16px;
    border-bottom: 1px solid var(--border);
    flex: none;
    flex-wrap: wrap;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 7px;
    flex: 1;
    min-width: 180px;
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
  .flash {
    display: flex;
    align-items: center;
    gap: 7px;
    margin: 10px 16px 0;
    padding: 8px 12px;
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--state-ok, #3fb950) 14%, transparent);
    color: var(--state-ok, #3fb950);
    font-size: 13px;
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
  .notice {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 14px;
    padding: 10px 12px;
    border-radius: var(--radius);
    border: 1px solid color-mix(in srgb, var(--brand-primary) 45%, var(--border));
    background: color-mix(in srgb, var(--brand-primary) 10%, transparent);
    font-size: 13px;
  }
  .notice span {
    flex: 1;
  }
  .updates {
    margin-bottom: 18px;
  }
  .updates .sec-head {
    flex-wrap: wrap;
  }
  .grow {
    flex: 1;
  }
  .upd-meta {
    font-size: 11.5px;
    font-weight: 500;
  }
  .count.accent {
    color: #fff;
    background: var(--brand-primary);
    border-color: var(--brand-primary);
  }
  .btn.sm {
    font-size: 12px;
    padding: 5px 10px;
  }
  .upd-list {
    list-style: none;
    margin: 8px 0 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .upd {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 5px 8px;
    border-radius: 6px;
    font-size: 12.5px;
  }
  .upd:nth-child(even) {
    background: color-mix(in srgb, var(--text) 3%, transparent);
  }
  .upd.off .inm {
    opacity: 0.6;
  }
  .upd .push {
    margin-left: auto;
  }
  .upd .disabled {
    margin-left: 0;
  }
  .mono {
    font-family: var(--font-mono);
  }
  .upbadge {
    margin-left: auto;
    border: 1px solid color-mix(in srgb, var(--brand-primary) 50%, var(--border));
    background: color-mix(in srgb, var(--brand-primary) 12%, transparent);
    color: var(--brand-primary);
    border-radius: 999px;
    font: inherit;
    font-size: 10.5px;
    font-weight: 600;
    font-family: var(--font-mono);
    padding: 1px 8px;
    cursor: pointer;
  }
  .inst .disabled + .upbadge {
    margin-left: 8px;
  }
  .confirm {
    margin: 8px 0 10px;
    padding: 12px;
    border: 1px solid var(--brand-primary);
    border-radius: var(--radius);
    background: var(--surface);
  }
  .c-title {
    margin: 0 0 6px;
    font-weight: 600;
    font-size: 13px;
  }
  .c-list {
    margin: 0 0 8px;
    padding-left: 18px;
    font-size: 12.5px;
  }
  .confirm p {
    margin: 0 0 6px;
  }
  .c-actions {
    display: flex;
    gap: 8px;
    margin-top: 8px;
  }
  .btn.confirm-btn {
    background: var(--state-ok, #3fb950);
  }
  .report {
    margin: 6px 0 8px;
  }
  .report p {
    margin: 2px 0;
  }
  .ok {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--state-ok, #3fb950);
  }
  .warn {
    color: var(--state-pending);
  }
  .errtxt {
    color: var(--state-danger);
  }
  .ignored {
    margin-top: 6px;
  }
  .ignored summary {
    cursor: pointer;
  }
  .ignored ul {
    margin: 6px 0 0;
    padding-left: 18px;
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
    align-items: baseline;
    gap: 10px;
    padding: 5px 8px;
    border-radius: 6px;
    font-size: 12.5px;
  }
  .inst:nth-child(even) {
    background: color-mix(in srgb, var(--text) 3%, transparent);
  }
  .inst.off {
    opacity: 0.55;
  }
  .inm {
    font-weight: 500;
  }
  .iv,
  .il {
    font-size: 11.5px;
    font-family: var(--font-mono);
  }
  .disabled {
    margin-left: auto;
    font-size: 10.5px;
    color: var(--state-pending);
  }
  .catalog .sec-head {
    margin-bottom: 10px;
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
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .prem {
    font-size: 9.5px;
    font-weight: 700;
    text-transform: uppercase;
    color: #fff;
    background: var(--state-pending);
    border-radius: 4px;
    padding: 1px 5px;
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
  .chip.gv {
    color: var(--brand-primary);
  }
  .actions {
    display: flex;
  }
  .btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    background: var(--brand-primary);
    color: #fff;
    border: none;
    border-radius: 7px;
    cursor: pointer;
    font: inherit;
    font-size: 12.5px;
    font-weight: 600;
    padding: 7px 12px;
    text-decoration: none;
  }
  .btn:hover:not(:disabled) {
    filter: brightness(1.08);
  }
  .btn:disabled {
    opacity: 0.55;
    cursor: default;
  }
  .btn.install {
    width: 100%;
    justify-content: center;
  }
  .btn.ext {
    background: var(--surface);
    border: 1px solid var(--border);
    color: var(--text);
    width: 100%;
    justify-content: center;
  }
  .btn.ghost {
    background: none;
    border: 1px solid var(--border);
    color: var(--text-muted);
    font-weight: 500;
  }
  .done {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12.5px;
    font-weight: 600;
    color: var(--state-ok, #3fb950);
    padding: 7px 0;
  }
  .picker {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 10px;
    padding-top: 10px;
    border-top: 1px solid var(--border);
    align-items: center;
  }
  .picker select {
    flex: 1;
    min-width: 140px;
  }
  .btn.confirm {
    background: var(--state-ok, #3fb950);
  }
  .small {
    font-size: 11.5px;
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
    padding: 20px 4px;
    font-size: 13px;
  }
  .err {
    padding: 12px;
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--state-danger) 12%, transparent);
    color: var(--state-danger);
    font-size: 13px;
  }
</style>
