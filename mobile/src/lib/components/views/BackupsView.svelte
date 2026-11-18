<script lang="ts">
  import { api, humanizeError, openExternal } from "../../ipc";
  import { t } from "../../i18n";
  import { portal } from "../../portal";
  import Icon from "../Icon.svelte";
  import type { Backup, Snapshot } from "../../types";

  let {
    serverId,
    active,
    lastBackupUrl = null,
  }: {
    serverId: number;
    active: boolean;
    /** URL signée du dernier backup (jeton porteur : ouverte, jamais affichée). */
    lastBackupUrl?: string | null;
  } = $props();
  let loaded = $state(false);

  let backups = $state<Backup[]>([]);
  let snapshots = $state<Snapshot[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);

  type Modal =
    | { kind: "create"; value: string }
    | { kind: "restore-backup"; id: number; label: string }
    | { kind: "restore-snapshot"; id: number; label: string }
    | { kind: "delete-snapshot"; id: number; label: string };
  let modal = $state<Modal | null>(null);

  function fmtSize(n: number): string {
    if (n >= 1024 ** 3) return `${(n / 1024 ** 3).toFixed(1)} Go`;
    if (n >= 1024 ** 2) return `${Math.round(n / 1024 ** 2)} Mo`;
    return `${Math.round(n / 1024)} Ko`;
  }

  async function load() {
    loading = true;
    error = null;
    try {
      const [b, s] = await Promise.all([api.listBackups(serverId), api.listSnapshots()]);
      backups = b;
      snapshots = s;
    } catch (err) {
      error = humanizeError(err);
    } finally {
      loading = false;
    }
  }

  function flash(msg: string) {
    notice = msg;
    setTimeout(() => (notice = null), 1800);
  }

  async function confirmModal() {
    if (!modal) return;
    const m = modal;
    modal = null;
    try {
      if (m.kind === "create") {
        await api.createSnapshot(serverId, m.value.trim() || "snapshot");
        flash(t("backups.done"));
        setTimeout(load, 2000);
      } else if (m.kind === "restore-backup") {
        await api.restoreBackup(serverId, m.id);
        flash(t("backups.done"));
      } else if (m.kind === "restore-snapshot") {
        await api.restoreSnapshot(m.id, serverId);
        flash(t("backups.done"));
      } else if (m.kind === "delete-snapshot") {
        await api.deleteSnapshot(m.id);
        flash(t("backups.done"));
        setTimeout(load, 1500);
      }
    } catch (err) {
      error = humanizeError(err);
    }
  }

  $effect(() => {
    if (active && !loaded) {
      loaded = true;
      load();
    }
  });
</script>

<div class="view">
  {#if notice}<p class="ok">{notice}</p>{/if}
  {#if error}<p class="err selectable">{error}</p>{/if}

  {#if loading}
    <p class="dim">…</p>
  {:else}
    <!-- Snapshots -->
    <section>
      <div class="shead">
        <h3>{t("backups.snapshots")}</h3>
        <button class="create" onclick={() => (modal = { kind: "create", value: "" })}>
          <Icon name="archive" size={16} /> {t("backups.create")}
        </button>
      </div>
      {#if snapshots.length === 0}
        <p class="dim">{t("backups.none")}</p>
      {:else}
        <ul>
          {#each snapshots as s (s.id ?? `job-${s.job_id}`)}
            <li>
              <span class="meta">
                <strong class="selectable">{s.name || (s.id != null ? `#${s.id}` : "")}</strong>
                {#if s.id == null || s.progress != null}
                  <!-- Création en cours : ni restaurable ni supprimable tant qu'il n'a pas d'id. -->
                  <small class="dim">
                    {t("backups.inProgress")}{#if s.progress != null}&nbsp;{s.progress}%{/if}
                  </small>
                {:else}
                  <small class="dim selectable">{s.date} · {fmtSize(s.size)}</small>
                {/if}
              </span>
              {#if s.id != null && s.progress == null}
                {@const id = s.id}
                <button
                  class="act"
                  onclick={() => (modal = { kind: "restore-snapshot", id, label: s.name })}
                  aria-label={t("backups.restore")}><Icon name="restart" size={18} /></button>
                <button
                  class="act danger"
                  onclick={() => (modal = { kind: "delete-snapshot", id, label: s.name })}
                  aria-label={t("backups.delete")}><Icon name="trash" size={18} /></button>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
    </section>

    <!-- Backups quotidiens -->
    <section>
      <h3>{t("backups.daily")}</h3>
      {#if lastBackupUrl}
        <button class="dl" onclick={() => openExternal(lastBackupUrl)}>
          <Icon name="save" size={16} />
          {t("overview.downloadLastBackup")}
        </button>
      {/if}
      {#if backups.length === 0}
        <p class="dim">{t("backups.none")}</p>
      {:else}
        <ul>
          {#each backups as b (b.id)}
            <li>
              <span class="meta">
                <strong class="selectable">{b.date}</strong>
                <small class="dim selectable">{fmtSize(b.size)}</small>
              </span>
              <button
                class="act"
                onclick={() => (modal = { kind: "restore-backup", id: b.id, label: b.date })}
                aria-label={t("backups.restore")}><Icon name="restart" size={18} /></button>
            </li>
          {/each}
        </ul>
      {/if}
    </section>
  {/if}
</div>

{#if modal}
  <div class="scrim" use:portal role="button" tabindex="-1" onclick={() => (modal = null)} onkeydown={() => {}}></div>
  <div class="dialog" use:portal>
    {#if modal.kind === "create"}
      <p class="dtitle">{t("backups.create")}</p>
      <input class="selectable" autocapitalize="off" placeholder={t("backups.name")} bind:value={modal.value} />
      <div class="dacts">
        <button onclick={() => (modal = null)}>{t("backups.cancel")}</button>
        <button class="primary" onclick={confirmModal}>{t("backups.create")}</button>
      </div>
    {:else}
      <p class="dtitle">
        {modal.kind === "delete-snapshot" ? t("backups.confirmDelete") : t("backups.confirmRestore")}
        {#if modal.label}<br /><span class="dim selectable">« {modal.label} »</span>{/if}
      </p>
      <div class="dacts">
        <button onclick={() => (modal = null)}>{t("backups.cancel")}</button>
        <button class="primary" class:danger={modal.kind === "delete-snapshot"} onclick={confirmModal}>
          {modal.kind === "delete-snapshot" ? t("backups.delete") : t("backups.restore")}
        </button>
      </div>
    {/if}
  </div>
{/if}

<style>
  /* Action du biome : fond teinté de l'accent du serveur. */
  .dl {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    width: 100%;
    margin-bottom: 10px;
    padding: 0 14px;
    border: none;
    border-radius: var(--radius);
    background: var(--accent-soft);
    color: var(--accent-text);
    font: inherit;
    font-size: 14px;
    font-weight: 800;
  }

  .view {
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 24px;
  }
  section {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .shead {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }
  h3 {
    margin: 0;
    font-size: 13px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
  }
  .create {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    background: var(--btn-primary-bg);
    color: var(--btn-primary-ink);
    border: none;
    border-radius: var(--radius);
    padding: 0 14px;
    font-size: 13.5px;
    font-weight: 900;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  li {
    display: flex;
    align-items: center;
    gap: 8px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    padding: 10px 10px 10px 14px;
  }
  .meta {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .meta strong {
    font-size: 15px;
    font-weight: 800;
  }
  .meta small {
    font-family: var(--font-mono);
    font-size: 11.5px;
  }
  .act {
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    flex: none;
    background: var(--btn-secondary-bg);
    border: none;
    border-radius: var(--radius);
    color: var(--btn-secondary-ink);
  }
  .act.danger {
    background: var(--danger-bg);
    color: var(--danger-ink);
  }
  .dim {
    color: var(--text-dim);
  }
  .ok {
    margin: 0;
    color: var(--state-running-text);
  }
  .err {
    margin: 0;
    color: var(--state-danger);
  }

  .scrim {
    position: fixed;
    inset: 0;
    background: var(--scrim);
    z-index: 40;
  }
  .dialog {
    position: fixed;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    z-index: 41;
    width: min(90vw, 380px);
    background: var(--elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow);
    padding: 18px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .dtitle {
    margin: 0;
    font-size: 15px;
    font-weight: 800;
    line-height: 1.4;
  }
  .dialog input {
    background: var(--inset);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    color: var(--text);
    padding: 12px;
    font-size: 16px;
    outline: none;
  }
  .dialog input:focus {
    border-color: var(--brand-primary);
    box-shadow: var(--ring);
  }
  .dacts {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .dacts button {
    background: var(--btn-secondary-bg);
    border: none;
    border-radius: var(--radius);
    color: var(--btn-secondary-ink);
    padding: 0 16px;
    font-weight: 800;
  }
  .dacts button.primary {
    background: var(--btn-primary-bg);
    color: var(--btn-primary-ink);
    font-weight: 900;
  }
  .dacts button.primary.danger {
    background: var(--danger-bg);
    color: var(--danger-ink);
    font-weight: 800;
  }
</style>
