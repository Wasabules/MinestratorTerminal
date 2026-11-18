<script lang="ts">
  /**
   * Onglet Notifications — vue plein écran des messages de l'hébergeur.
   *
   * Les notifications voyagent avec `GET /user` : il n'existe pas d'endpoint dédié accessible
   * avec une clé du panel, ni de marquage « lu » (vérifié le 2026-08-19). La vue est donc en
   * **lecture seule** ; le filtrage et la recherche se font entièrement côté client sur la
   * liste reçue, ce qui évite tout aller-retour réseau au moindre changement de filtre.
   */
  import { onMount } from 'svelte';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';
  import type { UserNotification } from '#lib/types.ts';
  import Icon from '../Icon.svelte';

  let all = $state<UserNotification[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let query = $state('');
  /** `null` = toutes les catégories. */
  let kind = $state<string | null>(null);

  async function load() {
    loading = true;
    error = null;
    try {
      all = await api.userNotifications();
    } catch (e) {
      error = humanizeError(e);
    } finally {
      loading = false;
    }
  }

  onMount(load);

  /** Catégories réellement présentes, avec leur compte — l'hébergeur en invente au fil du temps. */
  const kinds = $derived.by(() => {
    const counts = new Map<string, number>();
    for (const n of all) counts.set(n.kind, (counts.get(n.kind) ?? 0) + 1);
    return [...counts.entries()].sort((a, b) => b[1] - a[1]);
  });

  const shown = $derived.by(() => {
    const q = query.trim().toLowerCase();
    return all.filter(
      (n) =>
        (kind === null || n.kind === kind) &&
        (q === '' || n.message.toLowerCase().includes(q) || n.kind.toLowerCase().includes(q))
    );
  });

  /** Sévérité au vocabulaire du panel → token de la charte (voir DESIGN.md). */
  function color(c: string): string {
    switch (c) {
      case 'error':
        return 'var(--state-danger)';
      case 'warning':
        return 'var(--state-pending)';
      case 'success':
        return 'var(--state-running)';
      default:
        return 'var(--brand-primary)';
    }
  }
</script>

<div class="notifs">
  <header class="head">
    <div>
      <h1>{t('notifications.title')}</h1>
      <p class="dim">
        {#if !loading && !error}{t('notifications.summary', { count: all.length })}{/if}
      </p>
    </div>
    <button class="btn btn--ghost" onclick={load} disabled={loading}>
      {#if loading}<span class="spinner"></span>{/if} {t('common.refresh')}
    </button>
  </header>

  {#if loading && all.length === 0}
    <div class="state"><span class="spinner"></span> {t('common.loading')}</div>
  {:else if error}
    <div class="state">
      <p class="alert">{error}</p>
      <button class="btn" onclick={load}>{t('common.retry')}</button>
    </div>
  {:else if all.length === 0}
    <div class="state dim">{t('notifications.empty')}</div>
  {:else}
    <div class="filters">
      <div class="search">
        <Icon name="search" size={15} />
        <input
          type="search"
          bind:value={query}
          placeholder={t('notifications.search')}
          aria-label={t('notifications.search')}
        />
      </div>
      <div class="chips">
        <button class="fchip" class:on={kind === null} onclick={() => (kind = null)}>
          {t('notifications.allKinds')} <span class="n">{all.length}</span>
        </button>
        {#each kinds as [k, n] (k)}
          <button class="fchip" class:on={kind === k} onclick={() => (kind = k)}>
            {k} <span class="n">{n}</span>
          </button>
        {/each}
      </div>
    </div>

    {#if shown.length === 0}
      <div class="state dim">{t('notifications.noMatch')}</div>
    {:else}
      <ul class="list">
        {#each shown as n (n.id)}
          <li class="item">
            <span class="bar" style="background: {color(n.color)}"></span>
            <div class="body">
              <div class="top">
                <span class="kind" style="color: {color(n.color)}">{n.kind}</span>
                <span class="date dim">{n.date.slice(0, 16)}</span>
              </div>
              <p class="msg selectable">{n.message}</p>
            </div>
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</div>

<style>
  .notifs {
    height: 100%;
    overflow-y: auto;
    padding: 24px 28px 40px;
  }
  .head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
    max-width: 900px;
  }
  h1 {
    margin: 0 0 4px;
    font-size: 22px;
  }
  .head p {
    margin: 0;
    font-size: 13px;
  }
  .state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    padding: 48px 0;
    font-size: 14px;
  }

  .filters {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 10px 14px;
    margin: 20px 0 14px;
    max-width: 900px;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 6px 10px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    color: var(--text-dim);
    flex: 1;
    min-width: 200px;
  }
  .search input {
    border: 0;
    background: none;
    color: var(--text);
    font: inherit;
    font-size: 13px;
    outline: none;
    width: 100%;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .fchip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 5px 10px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: none;
    color: var(--text-muted);
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }
  .fchip:hover {
    background: color-mix(in srgb, var(--text) 5%, transparent);
  }
  .fchip.on {
    border-color: color-mix(in srgb, var(--brand-primary) 60%, transparent);
    background: color-mix(in srgb, var(--brand-primary) 14%, transparent);
    color: var(--text);
  }
  .fchip .n {
    font-variant-numeric: tabular-nums;
    color: var(--text-dim);
  }

  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-width: 900px;
  }
  .item {
    display: flex;
    gap: 12px;
    padding: 12px 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
  }
  .bar {
    width: 3px;
    border-radius: 999px;
    flex: none;
  }
  .body {
    min-width: 0;
    flex: 1;
  }
  .top {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 3px;
  }
  .kind {
    font-weight: 800;
    font-size: 13px;
  }
  .date {
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    flex: none;
  }
  .msg {
    margin: 0;
    font-size: 13px;
    line-height: 1.5;
    color: var(--text-muted);
    /* Les messages de l'hébergeur peuvent être longs : on les rend en entier, pas tronqués. */
    overflow-wrap: anywhere;
  }
</style>
