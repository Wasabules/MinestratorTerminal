<script lang="ts">
  import { onMount } from 'svelte';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';
  import { rememberCaps } from '#lib/games/capabilities.svelte.ts';
  import { tabs } from '#lib/tabs/tabs.svelte.ts';
  import type { MyBoxDetails, MyBoxSummary, ServerListItem, ServersOverview } from '#lib/types.ts';
  import ServerCard from './ServerCard.svelte';

  let data = $state<ServersOverview | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  /** Joueurs en ligne par serveur, remplis en UN appel groupé après le chargement de la liste. */
  let players = $state<Record<number, { current: number; limit: number }>>({});
  /** Détail par MyBox (`GET /mybox/{id}`) : renouvellement auto, ressources libres, suppression. */
  let details = $state<Record<number, MyBoxDetails>>({});

  async function load() {
    loading = true;
    error = null;
    try {
      data = await api.listServers();
      rememberCaps(data.servers);
      void loadPlayers(data.servers);
      void loadDetails(data.myboxes);
    } catch (e) {
      error = humanizeError(e);
    } finally {
      loading = false;
    }
  }

  /**
   * Un seul appel pour tous les serveurs actifs. Best-effort : le compte de joueurs est un
   * bonus d'affichage, un échec ne doit pas dégrader la page (ni afficher d'erreur).
   */
  async function loadPlayers(servers: ServerListItem[]) {
    const ids = servers.filter((s) => s.status === 'active').map((s) => s.id);
    if (ids.length === 0) return;
    try {
      const rows = await api.livePlayersBulk(ids);
      players = Object.fromEntries(
        rows.map((r) => [r.server_id, { current: r.current, limit: r.limit }])
      );
    } catch {
      players = {};
    }
  }

  /**
   * Un GET par MyBox (en parallèle ; un compte en a rarement plus de quelques-unes). Best-effort,
   * comme les joueurs : sans détail, la carte garde ses informations de base.
   */
  async function loadDetails(boxes: MyBoxSummary[]) {
    const rows = await Promise.all(
      boxes.map((b) => api.myboxDetails(b.id).then((d) => [b.id, d] as const, () => null))
    );
    details = Object.fromEntries(rows.filter((r) => r !== null));
  }

  /** Nombre lisible : entier tel quel, sinon une décimale (ressources fractionnaires). */
  function num(n: number): string {
    return Number.isInteger(n) ? String(n) : n.toFixed(1);
  }

  onMount(load);

  function serversOf(mybox: MyBoxSummary): ServerListItem[] {
    return data ? data.servers.filter((s) => s.mybox_id === mybox.id) : [];
  }

  const orphans = $derived(
    data ? data.servers.filter((s) => !data!.myboxes.some((m) => m.id === s.mybox_id)) : []
  );
  const totalServers = $derived(data?.servers.length ?? 0);
</script>

<div class="home">
  <header class="head">
    <div>
      <h1>{t('home.title')}</h1>
      <p class="dim">
        {#if data}{t('home.summary', { myboxes: data.myboxes.length, servers: totalServers })}{/if}
      </p>
    </div>
    <div class="head-actions">
      <button class="btn btn--ghost" onclick={() => tabs.openMyboxes()}>{t('mybox.manage')}</button>
      <button class="btn btn--ghost" onclick={load} disabled={loading}>
        {#if loading}<span class="spinner"></span>{/if} {t('common.refresh')}
      </button>
    </div>
  </header>

  {#if loading && !data}
    <div class="state"><span class="spinner"></span> {t('common.loading')}</div>
  {:else if error}
    <div class="state">
      <p class="alert">{error}</p>
      <button class="btn" onclick={load}>{t('common.retry')}</button>
    </div>
  {:else if data}
    {#each data.myboxes as mybox (mybox.id)}
      <section class="group">
        <div class="group-head">
          <div class="g-titles">
            <h2>{mybox.name}</h2>
            <span class="dim g-sub">
              {mybox.offer} · {t('home.resources', { cpu: mybox.cpu, ram: mybox.ram, disk: mybox.disk })}
              {#if details[mybox.id] && !mybox.expired}
                · {t('home.free', {
                  cpu: num(details[mybox.id].mybox.cpu_remaining),
                  ram: num(details[mybox.id].mybox.ram_remaining),
                })}
              {/if}
            </span>
          </div>
          <div class="g-badges">
            {#if mybox.pro}<span class="chip pro">{t('home.pro')}</span>{/if}
            {#if mybox.expired}
              {@const deletion = details[mybox.id]?.mybox.deletion_days}
              <span class="chip danger">
                {deletion != null ? t('home.deletionIn', { days: deletion }) : t('home.expired')}
              </span>
            {:else if mybox.suspended}<span class="chip danger">{t('home.suspended')}</span>
            {:else}
              {#if details[mybox.id]?.mybox.auto_renew}
                <span class="chip ok" title={t('home.autoRenewHint')}>{t('home.autoRenew')}</span>
              {/if}
              <span class="chip">{t('home.daysLeft', { days: mybox.days_left })}</span>
            {/if}
          </div>
        </div>

        {#if serversOf(mybox).length === 0}
          <p class="dim empty">{t('home.emptyMybox')}</p>
        {:else}
          <div class="grid">
            {#each serversOf(mybox) as server (server.id)}
              <ServerCard {server} players={players[server.id] ?? null} />
            {/each}
          </div>
        {/if}
      </section>
    {/each}

    {#if orphans.length > 0}
      <section class="group">
        <div class="group-head"><h2>{t('home.others')}</h2></div>
        <div class="grid">
          {#each orphans as server (server.id)}
            <ServerCard {server} players={players[server.id] ?? null} />
          {/each}
        </div>
      </section>
    {/if}

    {#if data.myboxes.length === 0 && totalServers === 0}
      <div class="state dim">{t('home.noServers')}</div>
    {/if}
  {/if}
</div>

<style>
  .home {
    padding: 30px 40px 48px;
    display: flex;
    flex-direction: column;
    gap: 30px;
  }
  .head {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 16px;
  }
  .head-actions {
    display: flex;
    gap: 8px;
  }
  .head h1 {
    margin: 0;
    font-size: 34px;
    line-height: 1.05;
  }
  .head p {
    margin: 6px 0 0;
    font-size: 14px;
  }
  .state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 14px;
    padding: 48px 0;
    color: var(--text-muted);
  }
  .group {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .group-head {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 8px 12px;
  }
  .group-head h2 {
    margin: 0;
    font-size: 20px;
    line-height: 1.2;
  }
  .g-sub {
    font-size: 13px;
  }
  .g-badges {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chip {
    font-size: 12px;
    font-weight: 800;
    color: var(--text-muted);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 3px 10px;
    white-space: nowrap;
  }
  .chip.pro {
    color: var(--state-hibernate);
    background: color-mix(in srgb, var(--state-hibernate) 14%, var(--surface));
    border-color: transparent;
  }
  .chip.ok {
    color: var(--state-running-text);
    background: color-mix(in srgb, var(--state-running) 12%, var(--surface));
    border-color: transparent;
  }
  .chip.danger {
    color: var(--danger-ink);
    background: var(--danger-bg);
    border-color: transparent;
  }
  .grid {
    display: grid;
    gap: 16px;
    grid-template-columns: repeat(auto-fill, minmax(min(300px, 100%), 1fr));
  }
  .empty {
    margin: 0;
    font-size: 13.5px;
    color: var(--text-dim);
  }
  @media (max-width: 760px) {
    .home {
      padding: 22px 16px 40px;
    }
  }
</style>
