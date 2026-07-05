<script lang="ts">
  import { onMount } from 'svelte';
  import { api, humanizeError } from '$lib/ipc';
  import { t } from '$lib/i18n';
  import type { MyBoxSummary, ServerListItem, ServersOverview } from '$lib/types';
  import ServerCard from './ServerCard.svelte';

  let data = $state<ServersOverview | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);

  async function load() {
    loading = true;
    error = null;
    try {
      data = await api.listServers();
    } catch (e) {
      error = humanizeError(e);
    } finally {
      loading = false;
    }
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
    <button class="btn btn--ghost" onclick={load} disabled={loading}>
      {#if loading}<span class="spinner"></span>{/if} {t('common.refresh')}
    </button>
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
            </span>
          </div>
          <div class="g-badges">
            {#if mybox.pro}<span class="chip pro">{t('home.pro')}</span>{/if}
            {#if mybox.expired}<span class="chip danger">{t('home.expired')}</span>
            {:else if mybox.suspended}<span class="chip danger">{t('home.suspended')}</span>
            {:else}<span class="chip">{t('home.daysLeft', { days: mybox.days_left })}</span>{/if}
          </div>
        </div>

        {#if serversOf(mybox).length === 0}
          <p class="dim empty">{t('home.emptyMybox')}</p>
        {:else}
          <div class="grid">
            {#each serversOf(mybox) as server (server.id)}
              <ServerCard {server} />
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
            <ServerCard {server} />
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
    max-width: 980px;
    margin: 0 auto;
    padding: 28px 24px 48px;
    display: flex;
    flex-direction: column;
    gap: 26px;
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
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    padding-bottom: 10px;
    border-bottom: 1px solid var(--border);
  }
  .group-head h2 {
    margin: 0;
    font-size: 16px;
  }
  .g-sub {
    font-size: 12.5px;
  }
  .g-badges {
    display: flex;
    gap: 6px;
    flex: none;
  }
  .chip {
    font-size: 11.5px;
    color: var(--text-muted);
    background: var(--elevated);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 3px 10px;
    white-space: nowrap;
  }
  .chip.pro {
    color: #fff;
    background: var(--brand-pro);
    border-color: transparent;
  }
  .chip.danger {
    color: var(--state-danger);
    border-color: color-mix(in srgb, var(--state-danger) 40%, var(--border));
  }
  .grid {
    display: grid;
    gap: 14px;
    grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
  }
  .empty {
    font-size: 13px;
    font-style: italic;
  }
</style>
