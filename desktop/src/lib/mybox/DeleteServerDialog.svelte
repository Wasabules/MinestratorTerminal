<script lang="ts">
  /**
   * Suppression DÉFINITIVE d'un serveur, avec confirmation forte : saisir son nom exact (vérifié
   * aussi en Rust). Une snapshot préalable peut être lancée et suivie ici ; la suppression attend
   * qu'elle soit terminée.
   */
  import { onDestroy } from 'svelte';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';
  import type { BackupJob, MyBoxCapacity, MyBoxServerSlot, SnapshotJob } from '#lib/types.ts';
  import Modal from './Modal.svelte';
  import { snapshotPossible } from './util.ts';

  let {
    cap,
    server,
    onclose,
    ondone,
  }: { cap: MyBoxCapacity; server: MyBoxServerSlot; onclose: () => void; ondone: () => void } = $props();

  let typed = $state('');
  let busy = $state(false);
  let error = $state<string | null>(null);
  let snapshot = $state<SnapshotJob | null>(null);
  let job = $state<BackupJob | null>(null);
  let timer: ReturnType<typeof setInterval> | undefined;

  const canSnapshot = $derived(snapshotPossible(cap, server));
  const snapshotRunning = $derived(!!snapshot && !(job && ['completed', 'failed', 'cancelled'].includes(job.status)));
  const confirmed = $derived(typed.trim() === server.name.trim() && server.name.trim() !== '');

  onDestroy(() => clearInterval(timer));

  async function startSnapshot() {
    error = null;
    try {
      snapshot = await api.myboxSnapshotBeforeDelete(cap.id, server.id);
      job = null;
      timer = setInterval(poll, 3000);
    } catch (e) {
      error = humanizeError(e);
    }
  }

  async function poll() {
    if (!snapshot) return;
    try {
      job = await api.backupJob(snapshot.job_id);
      if (['completed', 'failed', 'cancelled'].includes(job.status)) clearInterval(timer);
    } catch {
      // Lecture best-effort : on retente au prochain tour.
    }
  }

  async function remove() {
    if (!confirmed || snapshotRunning) return;
    busy = true;
    error = null;
    try {
      await api.myboxDeleteServer(cap.id, server.id, typed);
      ondone();
    } catch (e) {
      error = humanizeError(e);
    } finally {
      busy = false;
    }
  }
</script>

<Modal title={t('mybox.delete.title', { name: server.name })} {onclose}>
  <p class="alert">{t('mybox.delete.warning')}</p>
  <ul class="dim">
    <li>{t('mybox.delete.consequence1')}</li>
    <li>{t('mybox.delete.consequence2')}</li>
    <li>{t('mybox.delete.consequence3')}</li>
  </ul>

  <div class="snap card">
    <strong>{t('mybox.delete.snapshotTitle')}</strong>
    {#if !canSnapshot}
      <span class="dim">{t('mybox.delete.snapshotUnavailable')}</span>
    {:else if !snapshot}
      <span class="dim">{t('mybox.delete.snapshotHint')}</span>
      <button class="btn btn--ghost" onclick={startSnapshot}>{t('mybox.delete.snapshotStart')}</button>
    {:else if job?.status === 'completed'}
      <span class="ok">{t('mybox.delete.snapshotDone', { name: snapshot.snapshot_name })}</span>
    {:else if job && (job.status === 'failed' || job.status === 'cancelled')}
      <span class="alert">{t('mybox.delete.snapshotFailed')}</span>
    {:else}
      <span class="dim"><span class="spinner"></span> {t('mybox.delete.snapshotRunning', { progress: job?.progress ?? 0 })}</span>
    {/if}
  </div>

  <label class="field">
    <span class="label">{t('mybox.delete.confirmLabel', { name: server.name })}</span>
    <input class="input" autocomplete="off" spellcheck="false" bind:value={typed} placeholder={server.name} />
  </label>

  {#if error}<p class="alert">{error}</p>{/if}
  <div class="actions">
    <span class="spacer"></span>
    <button class="btn btn--ghost" onclick={onclose}>{t('common.cancel')}</button>
    <button class="btn btn--danger" onclick={remove} disabled={!confirmed || busy || snapshotRunning}>
      {#if busy}<span class="spinner"></span>{/if}
      {t('mybox.delete.submit')}
    </button>
  </div>
</Modal>

<style>
  ul {
    margin: 0;
    padding-left: 18px;
    font-size: 13px;
  }
  .snap {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px;
    font-size: 13px;
  }
  .ok {
    color: var(--state-running);
  }
  .actions {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .spacer {
    flex: 1;
  }
</style>
