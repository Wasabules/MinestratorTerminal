<script lang="ts">
  import { api, humanizeError } from "../ipc";
  import { auth } from "../stores/auth.svelte";
  import { t } from "../i18n";
  import Icon from "./Icon.svelte";
  import type { ServerListItem } from "../types";

  let {
    onOpen,
    onSettings,
  }: { onOpen: (server: ServerListItem) => void; onSettings: () => void } = $props();

  let servers = $state<ServerListItem[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);

  async function load() {
    loading = true;
    error = null;
    try {
      const overview = await api.listServers();
      servers = overview.servers;
    } catch (err) {
      error = humanizeError(err);
    } finally {
      loading = false;
    }
  }

  async function doLogout() {
    try {
      await api.logout();
    } finally {
      auth.setUser(null);
    }
  }

  function statusColor(status: string): string {
    switch (status) {
      case "active":
        return "var(--state-running)";
      case "hibernation":
        return "var(--state-hibernate)";
      case "suspended":
      case "expired":
        return "var(--state-danger)";
      default:
        return "var(--state-offline)";
    }
  }

  $effect(() => {
    load();
  });
</script>

<header class="top">
  <div class="titles">
    <h1>{t("servers.title")}</h1>
    {#if auth.user}<span class="who selectable">{auth.user.pseudo}</span>{/if}
  </div>
  <div class="actions">
    <button class="ic" onclick={onSettings} aria-label={t("settings.title")}>
      <Icon name="settings" size={20} />
    </button>
    <button class="ic" onclick={doLogout} aria-label={t("logout")}>
      <Icon name="logout" size={20} />
    </button>
  </div>
</header>

<div class="body">
  {#if loading}
    <p class="dim">…</p>
  {:else if error}
    <p class="err selectable">{error}</p>
    <button class="ghost" onclick={load}><Icon name="refresh" size={16} /> {t("servers.refresh")}</button>
  {:else if servers.length === 0}
    <p class="dim">{t("servers.empty")}</p>
  {:else}
    <ul>
      {#each servers as s (s.id)}
        <li>
          <button class="row" onclick={() => onOpen(s)}>
            <span class="dot" style="background:{statusColor(s.status)}"></span>
            <span class="name">
              <strong>{s.name}</strong>
              <small class="dim selectable">{s.egg_name} · {s.address}</small>
            </span>
            <Icon name="chevronRight" size={18} />
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .top {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    padding: calc(var(--safe-top) + 16px) 16px 12px;
    gap: 12px;
  }
  .titles {
    display: flex;
    flex-direction: column;
  }
  h1 {
    margin: 0;
    font-size: 22px;
    font-weight: 700;
  }
  .who {
    color: var(--text-dim);
    font-size: 13px;
  }
  .actions {
    display: flex;
    gap: 8px;
  }
  .ic {
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--text-muted);
  }
  .body {
    padding: 4px 12px;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .row {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 12px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 14px;
    text-align: left;
    color: var(--text);
  }
  .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    flex: none;
  }
  .name {
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
    min-width: 0;
  }
  .name strong {
    font-size: 16px;
  }
  .name small {
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ghost {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--text-muted);
    padding: 8px 12px;
    font-size: 13px;
  }
  .dim {
    color: var(--text-dim);
  }
  .err {
    color: var(--state-danger);
  }
</style>
