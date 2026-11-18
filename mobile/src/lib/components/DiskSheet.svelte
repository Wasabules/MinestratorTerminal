<script lang="ts">
  /**
   * « Espace disque », version simple : ce qui occupe le plus de place (tailles calculées par le
   * serveur, dossiers compris), occupation face à la limite, navigation dans les dossiers et
   * ouverture d'un dossier dans le gestionnaire de fichiers.
   */
  import { onMount } from "svelte";
  import { api, humanizeError } from "../ipc";
  import { t } from "../i18n";
  import { portal } from "../portal";
  import Icon from "./Icon.svelte";
  import type { DiskSpace, DiskUsage } from "../types";

  let {
    serverId,
    startPath = "/",
    onOpen,
    onClose,
  }: {
    serverId: number;
    startPath?: string;
    /** Ouvre un dossier dans le gestionnaire de fichiers (et ferme la feuille). */
    onOpen: (path: string) => void;
    onClose: () => void;
  } = $props();

  let usage = $state<DiskUsage | null>(null);
  let space = $state<DiskSpace | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let seq = 0;

  const path = $derived(usage?.path ?? startPath);

  onMount(() => {
    void load(startPath || "/");
    api
      .diskSpace(serverId)
      .then((s) => (space = s))
      .catch(() => (space = null));
  });

  async function load(target: string) {
    const mine = ++seq;
    loading = true;
    error = null;
    try {
      const u = await api.diskUsage(serverId, target);
      if (mine === seq) usage = u;
    } catch (err) {
      if (mine === seq) error = humanizeError(err);
    } finally {
      if (mine === seq) loading = false;
    }
  }

  function parent(p: string): string {
    const up = p.replace(/\/[^/]+\/?$/, "");
    return up === "" ? "/" : up;
  }

  function fmtSize(n: number): string {
    if (n <= 0) return "0 Ko";
    if (n >= 1024 ** 3) return `${(n / 1024 ** 3).toFixed(1)} Go`;
    if (n >= 1024 ** 2) return `${(n / 1024 ** 2).toFixed(1)} Mo`;
    return `${Math.max(1, Math.round(n / 1024))} Ko`;
  }

  function pct(part: number, whole: number): number {
    return whole > 0 ? Math.min(100, (part / whole) * 100) : 0;
  }

  function usedLine(used: number, limit: number): string {
    return limit > 0
      ? t("disk.usedOf").replace("{used}", fmtSize(used)).replace("{limit}", fmtSize(limit))
      : t("disk.used").replace("{used}", fmtSize(used));
  }
</script>

<div class="scrim" use:portal role="button" tabindex="-1" onclick={onClose} onkeydown={() => {}}>
  <div class="sheet" role="dialog" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
    <header class="obar">
      <button class="ic" disabled={path === "/"} onclick={() => load(parent(path))} aria-label={t("disk.up")}>
        <Icon name="back" size={18} />
      </button>
      <span class="title">
        <span class="name">{t("disk.title")}</span>
        <small class="dim selectable">{path}</small>
      </span>
      <button class="ic" onclick={onClose} aria-label={t("files.close")}>
        <Icon name="close" size={18} />
      </button>
    </header>

    {#if space}
      <div class="summary">
        <div class="line">
          <span>{t("disk.server")}</span>
          <span class="dim">{usedLine(space.used_bytes, space.limit_bytes)}</span>
        </div>
        {#if space.limit_bytes > 0}
          <span class="track"><span class="fill" class:warn={pct(space.used_bytes, space.limit_bytes) > 85} style="width:{pct(space.used_bytes, space.limit_bytes)}%"></span></span>
        {/if}
        {#if space.mybox}
          <div class="line">
            <span>{t("disk.mybox")}</span>
            <span class="dim">{usedLine(space.mybox.used_bytes, space.mybox.limit_bytes)}</span>
          </div>
          {#if space.mybox.limit_bytes > 0}
            <span class="track"><span class="fill" class:warn={pct(space.mybox.used_bytes, space.mybox.limit_bytes) > 85} style="width:{pct(space.mybox.used_bytes, space.mybox.limit_bytes)}%"></span></span>
          {/if}
        {/if}
      </div>
    {/if}

    <div class="folderbar">
      <span class="dim">{usage ? t("disk.total").replace("{size}", fmtSize(usage.total_size)) : ""}</span>
      <button class="open" onclick={() => onOpen(path)}>
        <Icon name="folder" size={16} /> {t("disk.open")}
      </button>
    </div>

    {#if error}
      <p class="err selectable">{error}</p>
    {:else if loading && !usage}
      <p class="dim pad">…</p>
    {:else if usage}
      <ul class:stale={loading}>
        {#if usage.items.length === 0}
          <li class="empty dim">{t("files.empty")}</li>
        {/if}
        {#each usage.items as it (it.path)}
          <li>
            <button class="row" onclick={() => (it.is_dir ? load(it.path) : onOpen(parent(it.path)))}>
              <Icon name={it.is_dir ? "folder" : "file"} size={18} />
              <span class="meta">
                <span class="top">
                  <span class="nm">{it.name}</span>
                  <span class="sz">{fmtSize(it.size)}</span>
                </span>
                <span class="track"><span class="fill" style="width:{pct(it.size, usage.total_size)}%"></span></span>
              </span>
              {#if it.is_dir}<Icon name="chevronRight" size={16} />{/if}
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    z-index: 40;
  }
  .sheet {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 41;
    max-height: 85vh;
    overflow-y: auto;
    background: var(--elevated);
    border-top-left-radius: 16px;
    border-top-right-radius: 16px;
    padding: 4px 8px calc(var(--safe-bottom) + 14px);
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .obar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 4px;
    border-bottom: 1px solid var(--border);
  }
  .title {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .title .name {
    font-weight: 600;
  }
  .title small {
    font-family: var(--font-mono);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ic {
    display: grid;
    place-items: center;
    width: 38px;
    height: 38px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--text-muted);
    flex: none;
  }
  .ic:disabled {
    opacity: 0.4;
  }
  .summary {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 10px 12px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    font-size: 14px;
  }
  .line {
    display: flex;
    justify-content: space-between;
    gap: 10px;
  }
  .track {
    display: block;
    height: 6px;
    border-radius: 3px;
    background: color-mix(in srgb, var(--text) 10%, transparent);
    overflow: hidden;
  }
  .fill {
    display: block;
    height: 100%;
    background: var(--brand-primary);
  }
  .fill.warn {
    background: var(--state-danger);
  }
  .folderbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    padding: 0 4px;
    font-size: 13px;
  }
  .open {
    display: flex;
    align-items: center;
    gap: 8px;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--text);
    padding: 8px 12px;
    font-size: 14px;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
    transition: opacity 0.15s ease;
  }
  ul.stale {
    opacity: 0.55;
  }
  li {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  li.empty {
    padding: 14px;
    text-align: center;
  }
  .row {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 12px;
    background: transparent;
    border: none;
    color: var(--text);
    padding: 10px 12px;
    text-align: left;
  }
  .meta {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .top {
    display: flex;
    justify-content: space-between;
    gap: 10px;
  }
  .nm {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sz {
    flex: none;
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text-muted);
  }
  .dim {
    color: var(--text-dim);
  }
  .pad {
    padding: 16px;
  }
  .err {
    margin: 0;
    padding: 8px 12px;
    color: var(--state-danger);
  }
</style>
