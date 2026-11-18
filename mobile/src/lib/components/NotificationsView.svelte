<script lang="ts">
  /**
   * Écran Notifications — pendant mobile de l'onglet desktop.
   *
   * Les notifications voyagent avec `GET /user` : pas d'endpoint dédié accessible avec une clé
   * du panel, pas de marquage « lu ». Lecture seule, donc ; recherche et filtres se font côté
   * client sur la liste déjà chargée, sans aller-retour réseau.
   */
  import { api, humanizeError } from "../ipc";
  import { t } from "../i18n";
  import Icon from "./Icon.svelte";
  import Masked from "./Masked.svelte";
  import type { UserNotification } from "../types";

  let { onBack }: { onBack: () => void } = $props();

  let all = $state<UserNotification[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let query = $state("");
  /** `null` = toutes les catégories. */
  let kind = $state<string | null>(null);

  async function load() {
    loading = true;
    error = null;
    try {
      all = await api.userNotifications();
    } catch (err) {
      error = humanizeError(err);
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    load();
  });

  /** Catégories réellement présentes, avec leur compte : l'hébergeur en ajoute au fil du temps. */
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
        (q === "" || n.message.toLowerCase().includes(q) || n.kind.toLowerCase().includes(q)),
    );
  });

  /** Sévérité au vocabulaire du panel → token de la charte (DESIGN.md). */
  function color(c: string): string {
    switch (c) {
      case "error":
        return "var(--state-danger)";
      case "warning":
        return "var(--state-pending)";
      case "success":
        return "var(--state-running)";
      default:
        return "var(--brand-primary)";
    }
  }
</script>

<div class="screen">
  <header>
    <button class="ic" onclick={onBack} aria-label={t("common.back")}><Icon name="back" size={20} /></button>
    <span class="title">{t("notifications.title")}</span>
    <button class="ic" onclick={load} aria-label={t("servers.refresh")} disabled={loading}>
      <Icon name="refresh" size={19} />
    </button>
  </header>

  <div class="body">
    {#if loading && all.length === 0}
      <p class="dim">…</p>
    {:else if error}
      <p class="err selectable">{error}</p>
    {:else if all.length === 0}
      <p class="dim">{t("notifications.empty")}</p>
    {:else}
      <p class="count dim">{all.length} {t("notifications.count")}</p>

      <div class="search">
        <Icon name="search" size={16} />
        <input
          type="search"
          bind:value={query}
          placeholder={t("notifications.search")}
          aria-label={t("notifications.search")}
        />
      </div>

      <div class="chips">
        <button class="fchip" class:on={kind === null} onclick={() => (kind = null)}>
          {t("notifications.allKinds")} <span class="n">{all.length}</span>
        </button>
        {#each kinds as [k, n] (k)}
          <button class="fchip" class:on={kind === k} onclick={() => (kind = k)}>
            {k} <span class="n">{n}</span>
          </button>
        {/each}
      </div>

      {#if shown.length === 0}
        <p class="dim">{t("notifications.noMatch")}</p>
      {:else}
        <ul class="list">
          {#each shown as n (n.id)}
            <li class="item">
              <span class="bar" style="background: {color(n.color)}"></span>
              <div class="c">
                <div class="top">
                  <span class="kind" style="color: {color(n.color)}">{n.kind}</span>
                  <span class="date dim">{n.date.slice(0, 16)}</span>
                </div>
                <!-- Mode anonyme : IP et e-mails du message masqués à l'affichage. -->
                <p class="msg selectable"><Masked value={n.message} /></p>
              </div>
            </li>
          {/each}
        </ul>
      {/if}
    {/if}
  </div>
</div>

<style>
  .screen {
    display: flex;
    flex-direction: column;
    min-height: 100dvh;
  }
  header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: calc(var(--safe-top) + 8px) 12px 8px;
    background: var(--chrome);
    border-bottom: 1px solid var(--border);
  }
  .ic {
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    background: transparent;
    border: none;
    border-radius: var(--radius);
    color: var(--text);
  }
  .ic:disabled {
    opacity: 0.5;
  }
  .title {
    font-size: 18px;
    font-weight: 900;
    flex: 1;
  }
  .body {
    flex: 1;
    padding: 12px 14px calc(var(--safe-bottom) + 24px);
  }
  .count {
    margin: 0 0 10px;
    font-size: 12px;
  }
  .err {
    color: var(--state-danger);
    font-size: 14px;
  }

  .search {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 48px;
    padding: 0 14px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--inset);
    color: var(--text-dim);
    margin-bottom: 10px;
  }
  .search:focus-within {
    border-color: var(--brand-primary);
    box-shadow: var(--ring);
  }
  .search input {
    border: 0;
    background: none;
    color: var(--text);
    font: inherit;
    font-size: 15px; /* 16px mini évite le zoom auto d'Android sur focus */
    outline: none;
    width: 100%;
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-bottom: 14px;
  }
  .fchip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 0 12px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--inset);
    color: var(--text-muted);
    font: inherit;
    font-size: 13px;
    font-weight: 800;
  }
  .fchip.on {
    border-color: transparent;
    background: var(--accent);
    color: var(--on-accent);
  }
  .fchip .n {
    font-family: var(--font-mono);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    opacity: 0.75;
  }

  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .item {
    display: flex;
    gap: 10px;
    padding: 12px 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--surface);
  }
  .bar {
    width: 3px;
    border-radius: 999px;
    flex: none;
  }
  .c {
    min-width: 0;
    flex: 1;
  }
  .top {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 10px;
    margin-bottom: 3px;
  }
  .kind {
    font-weight: 800;
    font-size: 14px;
  }
  .date {
    font-family: var(--font-mono);
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
    flex: none;
  }
  .msg {
    margin: 0;
    font-size: 14px;
    line-height: 1.5;
    color: var(--text-muted);
    overflow-wrap: anywhere;
  }
</style>
