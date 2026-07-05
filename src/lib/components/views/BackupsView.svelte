<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { api, humanizeError } from '$lib/ipc';
  import { t } from '$lib/i18n';
  import type { Backup, Snapshot } from '$lib/types';
  import type { ServerTab } from '$lib/tabs/tabs.svelte';
  import Icon from '../Icon.svelte';

  let { tab }: { tab: ServerTab } = $props();
  const serverId = $derived(tab.serverId);

  let snapshots = $state<Snapshot[]>([]);
  let backups = $state<Backup[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let newName = $state('');
  let creating = $state(false);
  // Clé de l'action destructive en attente de confirmation (2 temps) et de celle en cours.
  let pending = $state<string | null>(null);
  let busy = $state<string | null>(null);
  let flash = $state<string | null>(null);
  let flashErr = $state(false);
  let flashTimer: ReturnType<typeof setTimeout> | undefined;
  let poll: ReturnType<typeof setInterval> | undefined;
  let pollTicks = 0;

  // Une création en cours expose un `progress` (les snapshots prêts n'en ont pas).
  const someInProgress = $derived(snapshots.some((s) => s.progress != null));

  onMount(load);
  onDestroy(() => {
    clearTimeout(flashTimer);
    clearInterval(poll);
  });

  async function load() {
    loading = true;
    error = null;
    try {
      const [s, b] = await Promise.all([api.listSnapshots(), api.listBackups(serverId)]);
      snapshots = s;
      backups = b;
      syncPolling();
    } catch (e) {
      error = humanizeError(e);
    } finally {
      loading = false;
    }
  }

  // Poll la liste tant qu'une création est en cours (pas d'endpoint de job dédié côté API), avec
  // un plafond de sécurité pour ne pas boucler indéfiniment si un job reste bloqué.
  function syncPolling() {
    if (someInProgress && !poll) {
      pollTicks = 0;
      poll = setInterval(tick, 4000);
    } else if (!someInProgress && poll) {
      clearInterval(poll);
      poll = undefined;
    }
  }
  // Force le démarrage du polling (création OPTIMISTE : le job vient d'être lancé et peut ne pas
  // encore figurer dans la liste → on poll quand même, avec une grâce de quelques ticks).
  function startPolling() {
    if (!poll) {
      pollTicks = 0;
      poll = setInterval(tick, 4000);
    }
  }
  async function tick() {
    if (++pollTicks > 90) {
      clearInterval(poll);
      poll = undefined;
      return;
    }
    try {
      snapshots = await api.listSnapshots();
    } catch {
      /* transitoire : on retentera au prochain tick */
    }
    // Arrête quand plus aucune création en cours — mais garde la main quelques ticks pour laisser
    // un job fraîchement créé apparaître dans la liste.
    if (!someInProgress && pollTicks > 2 && poll) {
      clearInterval(poll);
      poll = undefined;
    }
  }

  async function create() {
    const name = newName.trim();
    if (!name || creating) return;
    creating = true;
    try {
      await api.createSnapshot(serverId, name);
      newName = '';
      showFlash(t('backups.created'), false);
      snapshots = await api.listSnapshots();
      startPolling(); // optimiste : le job vient d'être lancé, il peut ne pas encore être visible
    } catch (e) {
      showFlash(humanizeError(e), true);
    } finally {
      creating = false;
    }
  }

  // Action destructive en 2 temps : 1er clic = arme la confirmation, 2e = exécute.
  async function run(key: string, fn: () => Promise<unknown>, okMsg: string) {
    if (pending !== key) {
      pending = key;
      return;
    }
    pending = null;
    busy = key;
    try {
      await fn();
      showFlash(okMsg, false);
      await load();
    } catch (e) {
      showFlash(humanizeError(e), true);
    } finally {
      busy = null;
    }
  }

  function showFlash(message: string, isErr: boolean) {
    flash = message;
    flashErr = isErr;
    clearTimeout(flashTimer);
    flashTimer = setTimeout(() => (flash = null), 2600);
  }

  function humanSize(bytes: number): string {
    if (!bytes || bytes < 0) return '—';
    const units = ['o', 'Ko', 'Mo', 'Go', 'To'];
    let n = bytes;
    let i = 0;
    while (n >= 1024 && i < units.length - 1) {
      n /= 1024;
      i++;
    }
    return `${n.toFixed(n < 10 && i > 0 ? 1 : 0)} ${units[i]}`;
  }
</script>

<div class="backups">
  <header class="head">
    <div>
      <h1>{t('backups.title')}</h1>
      <p class="dim">{t('backups.subtitle')}</p>
    </div>
    <button
      class="btn btn--ghost"
      onclick={load}
      disabled={loading}
      aria-label={t('common.refresh')}
      title={t('common.refresh')}
    >
      {#if loading}<span class="spinner"></span>{/if}
      <Icon name="refresh-cw" size={15} />
    </button>
  </header>

  {#if error}
    <div class="state">
      <p class="alert">{error}</p>
      <button class="btn" onclick={load}>{t('common.retry')}</button>
    </div>
  {:else}
    <!-- ===== Snapshots (à la demande) ===== -->
    <section>
      <h2>{t('backups.snapshots')}</h2>
      <p class="dim hint">{t('backups.snapshotsHint')}</p>

      <div class="card create">
        <input
          class="input"
          bind:value={newName}
          placeholder={t('backups.namePlaceholder')}
          spellcheck="false"
          maxlength="60"
          onkeydown={(e) => e.key === 'Enter' && create()}
        />
        <button class="btn" onclick={create} disabled={creating || newName.trim().length === 0}>
          {#if creating}<span class="spinner"></span>{:else}<Icon name="plus" size={15} />{/if}
          {creating ? t('backups.creating') : t('backups.create')}
        </button>
      </div>

      {#if loading && snapshots.length === 0}
        <div class="state"><span class="spinner"></span></div>
      {:else if snapshots.length === 0}
        <p class="dim empty">{t('backups.none')}</p>
      {:else}
        <div class="list">
          {#each snapshots as s (s.id)}
            <div class="row">
              <span class="ic" class:pulse={s.progress != null}><Icon name="hard-drive" size={16} /></span>
              <div class="meta">
                <span class="name">
                  {s.name || `#${s.id}`}
                  {#if s.is_legacy}<span class="tag">{t('backups.legacy')}</span>{/if}
                </span>
                <span class="sub dim">
                  {#if s.progress != null}
                    {t('backups.inProgress')} {s.progress}%
                  {:else}
                    {s.date} · {humanSize(s.size)}
                  {/if}
                </span>
              </div>
              {#if s.progress == null}
                <div class="acts">
                  {@render danger(
                    `rs-${s.id}`,
                    t('backups.restore'),
                    t('backups.restoreConfirm'),
                    () => api.restoreSnapshot(s.id, serverId),
                    t('backups.restoreStarted')
                  )}
                  {@render danger(
                    `ds-${s.id}`,
                    t('backups.delete'),
                    t('backups.deleteConfirm'),
                    () => api.deleteSnapshot(s.id),
                    t('backups.deleted'),
                    'trash'
                  )}
                </div>
              {/if}
            </div>
          {/each}
        </div>
      {/if}
    </section>

    <!-- ===== Backups quotidiens (automatiques) ===== -->
    <section>
      <h2>{t('backups.daily')}</h2>
      <p class="dim hint">{t('backups.dailyHint')}</p>
      {#if loading && backups.length === 0}
        <div class="state"><span class="spinner"></span></div>
      {:else if backups.length === 0}
        <p class="dim empty">{t('backups.none')}</p>
      {:else}
        <div class="list">
          {#each backups as b (b.id)}
            <div class="row">
              <span class="ic"><Icon name="hard-drive" size={16} /></span>
              <div class="meta">
                <span class="name">{b.date}</span>
                <span class="sub dim">#{b.id} · {humanSize(b.size)}</span>
              </div>
              <div class="acts">
                {@render danger(
                  `rb-${b.id}`,
                  t('backups.restore'),
                  t('backups.restoreConfirm'),
                  () => api.restoreBackup(serverId, b.id),
                  t('backups.restoreStarted')
                )}
              </div>
            </div>
          {/each}
        </div>
      {/if}
    </section>
  {/if}
</div>

{#if flash}
  <div class="toast" class:err={flashErr}>{flash}</div>
{/if}

<!-- Bouton d'action destructive à confirmation en 2 temps (rouge « ⚠ … ? » au 1er clic). -->
{#snippet danger(
  key: string,
  label: string,
  confirmLabel: string,
  fn: () => Promise<unknown>,
  okMsg: string,
  icon?: string
)}
  {#if pending === key}
    <button class="mini cancel" onclick={() => (pending = null)} aria-label={t('backups.cancel')}>
      <Icon name="x" size={14} />
    </button>
  {/if}
  <button
    class="mini danger"
    class:armed={pending === key}
    disabled={busy === key}
    aria-label={label}
    onclick={() => run(key, fn, okMsg)}
  >
    {#if busy === key}<span class="spinner"></span>{/if}
    {#if pending === key}
      {confirmLabel}
    {:else if icon}
      <Icon name={icon} size={14} />
    {:else}
      {label}
    {/if}
  </button>
{/snippet}

<style>
  .backups {
    max-width: 780px;
    margin: 0 auto;
    padding: 26px 24px 48px;
    display: flex;
    flex-direction: column;
    gap: 22px;
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
    margin: 5px 0 0;
    font-size: 13px;
    max-width: 60ch;
  }
  section {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  h2 {
    margin: 0;
    font-size: 13px;
    text-transform: uppercase;
    letter-spacing: 0.09em;
    color: var(--text-muted);
    font-family: var(--font-mono);
  }
  .hint {
    margin: -4px 0 4px;
    font-size: 12.5px;
  }
  .create {
    display: flex;
    gap: 10px;
    padding: 12px;
    align-items: center;
  }
  .create .input {
    flex: 1;
  }
  .state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 14px;
    padding: 28px 0;
    color: var(--text-muted);
  }
  .empty {
    font-size: 13px;
    font-style: italic;
    margin: 2px 0;
  }
  .list {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
  }
  .row:last-child {
    border-bottom: none;
  }
  .ic {
    flex: none;
    display: inline-flex;
    color: var(--text-dim);
  }
  .ic.pulse {
    color: var(--brand-primary);
    animation: pulse 1.2s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
  .meta {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .name {
    font-weight: 600;
    font-size: 13.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sub {
    font-size: 12px;
    font-family: var(--font-mono);
  }
  .tag {
    font-size: 10px;
    font-weight: 600;
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 1px 5px;
    margin-left: 6px;
  }
  .acts {
    flex: none;
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .mini {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    border: 1px solid var(--border);
    border-radius: 7px;
    background: none;
    color: var(--text-muted);
    cursor: pointer;
    font: inherit;
    font-size: 12px;
    padding: 5px 10px;
  }
  .mini:hover:not(:disabled) {
    color: var(--text);
    border-color: var(--text-dim);
  }
  .mini:disabled {
    opacity: 0.55;
    cursor: default;
  }
  /* En attente de confirmation : rouge plein pour signaler le caractère destructif. */
  .mini.armed {
    background: var(--state-danger);
    border-color: var(--state-danger);
    color: #fff;
  }
  .mini.cancel {
    padding: 5px 8px;
  }
  .toast {
    position: absolute;
    bottom: 20px;
    left: 50%;
    transform: translateX(-50%);
    background: var(--elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 10px 16px;
    font-size: 13px;
    box-shadow: var(--shadow);
    color: var(--brand-primary);
    z-index: 5;
    max-width: 90%;
    text-align: center;
  }
  .toast.err {
    color: var(--state-danger);
  }
</style>
