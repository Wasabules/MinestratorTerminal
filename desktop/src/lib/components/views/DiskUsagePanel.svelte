<script lang="ts">
  /**
   * Vue « Espace disque » : ce qui occupe le plus de place sur le serveur, calculé par le serveur
   * (tailles récursives) — là où le SFTP devrait tout parcourir. Navigation dans les dossiers,
   * occupation face à la limite du serveur et au disque partagé de la MyBox, ouverture d'un
   * dossier dans le gestionnaire de fichiers.
   */
  import { onMount } from 'svelte';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';
  import { fmtBytes } from '#lib/copilot/format.ts';
  import type { DiskSpace, DiskUsage } from '#lib/types.ts';
  import Icon from '../Icon.svelte';
  import Gauge from '../Gauge.svelte';

  let {
    serverId,
    startPath = '/',
    onOpen,
    onClose,
  }: {
    serverId: number;
    startPath?: string;
    /** Ouvre un dossier dans le gestionnaire de fichiers (et ferme la vue). */
    onOpen: (path: string) => void;
    onClose: () => void;
  } = $props();

  const LIMIT = 20;
  const LIMIT_MORE = 100;

  let path = $state('/');
  let limit = $state(LIMIT);
  let usage = $state<DiskUsage | null>(null);
  let space = $state<DiskSpace | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  /** Dernière requête lancée : une réponse tardive d'un dossier quitté est ignorée. */
  let seq = 0;

  const crumbs = $derived.by(() => {
    const out = [{ label: '/', path: '/' }];
    let acc = '';
    for (const seg of path.split('/').filter(Boolean)) {
      acc += `/${seg}`;
      out.push({ label: seg, path: acc });
    }
    return out;
  });
  const shown = $derived(usage?.items ?? []);
  const shownSum = $derived(shown.reduce((n, i) => n + i.size, 0));
  const rest = $derived(usage ? Math.max(0, usage.total_size - shownSum) : 0);

  onMount(() => {
    void load(startPath || '/');
    void loadSpace();
  });

  async function load(target: string, newLimit = LIMIT) {
    const mine = ++seq;
    loading = true;
    error = null;
    try {
      const u = await api.diskUsage(serverId, target, newLimit);
      if (mine !== seq) return;
      usage = u;
      path = u.path;
      limit = newLimit;
    } catch (e) {
      if (mine === seq) error = humanizeError(e);
    } finally {
      if (mine === seq) loading = false;
    }
  }

  async function loadSpace() {
    try {
      space = await api.diskSpace(serverId);
    } catch {
      space = null; // l'analyse reste utile sans le résumé
    }
  }

  function parent(p: string): string {
    const up = p.replace(/\/[^/]+\/?$/, '');
    return up === '' ? '/' : up;
  }

  function pct(part: number, whole: number): number {
    return whole > 0 ? (part / whole) * 100 : 0;
  }
</script>

<div class="diskview" role="dialog" aria-label={t('sftp.disk.title')}>
  <div class="abar">
    <span class="fname"><Icon name="hard-drive" size={15} /> {t('sftp.disk.title')}</span>
    <div class="grow"></div>
    <button class="tb" onclick={() => { void load(path, limit); void loadSpace(); }} title={t('common.refresh')}>
      <Icon name="refresh-cw" size={14} />
    </button>
    <button class="close" onclick={onClose} title={t('common.close')}><Icon name="x" size={16} /></button>
  </div>

  <div class="body">
    {#if space}
      <section class="summary">
        <div class="card g">
          <Gauge
            label={t('sftp.disk.server')}
            percent={pct(space.used_bytes, space.limit_bytes)}
            detail={space.limit_bytes > 0
              ? t('sftp.disk.usedOf', { used: fmtBytes(space.used_bytes), limit: fmtBytes(space.limit_bytes) })
              : t('sftp.disk.used', { used: fmtBytes(space.used_bytes) })}
          />
          {#if space.allowance_applied}
            <p class="hint dim">
              {t('sftp.disk.allowance', { raw: fmtBytes(space.raw_bytes), base: fmtBytes(space.base_game_bytes) })}
            </p>
          {/if}
        </div>
        {#if space.mybox}
          <div class="card g">
            <Gauge
              label={t('sftp.disk.mybox')}
              percent={pct(space.mybox.used_bytes, space.mybox.limit_bytes)}
              detail={space.mybox.limit_bytes > 0
                ? t('sftp.disk.usedOf', { used: fmtBytes(space.mybox.used_bytes), limit: fmtBytes(space.mybox.limit_bytes) })
                : t('sftp.disk.used', { used: fmtBytes(space.mybox.used_bytes) })}
            />
            <p class="hint dim">{t('sftp.disk.myboxHint')}</p>
          </div>
        {/if}
      </section>
    {/if}

    <div class="nav">
      <button class="tb" disabled={path === '/'} onclick={() => load(parent(path))} title={t('sftp.up')}>
        <Icon name="arrow-up" size={15} />
      </button>
      <nav class="crumbs">
        {#each crumbs as c, i (c.path)}
          {#if i > 0}<span class="sl">/</span>{/if}
          <button class="crumb" onclick={() => load(c.path)}>{c.label}</button>
        {/each}
      </nav>
      <div class="grow"></div>
      {#if usage}<span class="total">{t('sftp.disk.folderTotal', { size: fmtBytes(usage.total_size) })}</span>{/if}
      <button class="btn small" onclick={() => onOpen(path)}>
        <Icon name="folder" size={14} /> {t('sftp.disk.openFolder')}
      </button>
    </div>

    {#if error}
      <div class="err">{error}</div>
    {:else if loading && !usage}
      <div class="center"><span class="spinner"></span></div>
    {:else if usage}
      <div class="items" class:stale={loading}>
        {#if shown.length === 0}
          <div class="center dim">{t('sftp.empty')}</div>
        {/if}
        {#each shown as it (it.path)}
          <div class="item">
            <button
              class="imain"
              disabled={!it.is_dir}
              onclick={() => it.is_dir && load(it.path)}
              title={it.is_dir ? t('sftp.disk.explore') : it.name}
            >
              <span class="ico" class:dir={it.is_dir}><Icon name={it.is_dir ? 'folder' : 'file'} size={15} /></span>
              <span class="iname">{it.name}</span>
              <span class="ibar"><span class="ifill" style="width: {pct(it.size, usage.total_size)}%"></span></span>
              <span class="isize">{fmtBytes(it.size)}</span>
              <span class="ipct dim">{pct(it.size, usage.total_size).toFixed(0)} %</span>
            </button>
            <button
              class="ra"
              title={it.is_dir ? t('sftp.disk.openFolder') : t('sftp.disk.showFile')}
              aria-label={it.is_dir ? t('sftp.disk.openFolder') : t('sftp.disk.showFile')}
              onclick={() => onOpen(it.is_dir ? it.path : parent(it.path))}
            >
              <Icon name="external-link" size={14} />
            </button>
          </div>
        {/each}
        {#if rest > 0 && shown.length > 0}
          <div class="restline dim">
            {t('sftp.disk.rest', { size: fmtBytes(rest) })}
            {#if limit < LIMIT_MORE && shown.length >= limit}
              <button class="linklike" onclick={() => load(path, LIMIT_MORE)}>{t('sftp.disk.more')}</button>
            {/if}
          </div>
        {/if}
      </div>
    {/if}
    <p class="hint dim foot">{t('sftp.disk.hint')}</p>
  </div>
</div>

<style>
  .diskview {
    position: absolute;
    inset: 20px;
    display: flex;
    flex-direction: column;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
  }
  .abar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 9px 12px;
    border-bottom: 1px solid var(--border);
    background: var(--elevated);
  }
  .fname {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    font-weight: 600;
    font-size: 13.5px;
  }
  .grow {
    flex: 1;
  }
  .close,
  .tb {
    display: inline-flex;
    align-items: center;
    background: none;
    border: none;
    color: var(--text-dim);
    cursor: pointer;
    padding: 5px 6px;
    border-radius: 6px;
  }
  .close:hover,
  .tb:hover:not(:disabled) {
    color: var(--text);
    background: color-mix(in srgb, var(--text) 8%, transparent);
  }
  .tb:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
  .body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 14px 16px 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .summary {
    display: grid;
    gap: 12px;
    grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
  }
  .g {
    padding: 14px 16px;
  }
  .hint {
    margin: 8px 0 0;
    font-size: 12px;
  }
  .foot {
    margin: 0;
  }
  .nav {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .crumbs {
    display: flex;
    align-items: center;
    gap: 2px;
    overflow-x: auto;
    white-space: nowrap;
    scrollbar-width: none;
    min-width: 0;
  }
  .crumb {
    background: none;
    border: none;
    cursor: pointer;
    font: inherit;
    font-family: var(--font-mono);
    font-size: 12.5px;
    color: var(--text-muted);
    padding: 3px 5px;
    border-radius: 5px;
  }
  .crumb:hover {
    color: var(--brand-primary);
  }
  .sl {
    color: var(--text-dim);
  }
  .total {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text-muted);
    white-space: nowrap;
  }
  .btn.small {
    padding: 5px 11px;
    font-size: 12.5px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    white-space: nowrap;
  }
  .err {
    padding: 10px 12px;
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--state-danger) 12%, transparent);
    color: color-mix(in srgb, var(--state-danger) 70%, var(--text));
    font-size: 13px;
  }
  .center {
    display: grid;
    place-items: center;
    padding: 40px;
  }
  .items {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
    transition: opacity 0.15s ease;
  }
  .items.stale {
    opacity: 0.55;
  }
  .item {
    display: flex;
    align-items: center;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 55%, transparent);
  }
  .item:last-child {
    border-bottom: none;
  }
  .item:hover {
    background: color-mix(in srgb, var(--text) 4%, transparent);
  }
  .imain {
    flex: 1;
    min-width: 0;
    display: grid;
    grid-template-columns: 18px minmax(0, 1fr) minmax(80px, 30%) 78px 44px;
    align-items: center;
    gap: 10px;
    background: none;
    border: none;
    cursor: pointer;
    font: inherit;
    color: var(--text);
    text-align: left;
    padding: 8px 6px 8px 12px;
  }
  .imain:disabled {
    cursor: default;
  }
  .ico {
    display: inline-flex;
    color: var(--text-dim);
  }
  .ico.dir {
    color: var(--brand-primary);
  }
  .iname {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13.5px;
  }
  .ibar {
    height: 6px;
    border-radius: 3px;
    background: color-mix(in srgb, var(--text) 10%, transparent);
    overflow: hidden;
  }
  .ifill {
    display: block;
    height: 100%;
    background: var(--brand-primary);
  }
  .isize {
    font-family: var(--font-mono);
    font-size: 12px;
    text-align: right;
  }
  .ipct {
    font-family: var(--font-mono);
    font-size: 11.5px;
    text-align: right;
  }
  .ra {
    background: none;
    border: none;
    cursor: pointer;
    color: var(--text-dim);
    padding: 6px 10px;
    border-radius: 6px;
    display: inline-flex;
  }
  .ra:hover {
    color: var(--text);
    background: color-mix(in srgb, var(--text) 8%, transparent);
  }
  .restline {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    font-size: 12.5px;
  }
  .linklike {
    padding: 0;
    border: 0;
    background: none;
    color: var(--brand-primary);
    font: inherit;
    cursor: pointer;
  }
  .linklike:hover {
    text-decoration: underline;
  }
</style>
