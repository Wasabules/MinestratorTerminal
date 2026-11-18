<script lang="ts">
  /**
   * Redimensionnement d'un serveur : valeurs FINALES de CPU (cœurs) et de RAM (Go), bornées à ce
   * que la MyBox peut donner (le serveur rend d'abord ce qu'il occupe s'il est actif). Si l'API
   * exige le serveur arrêté, on propose de l'arrêter puis de réessayer.
   */
  import { api, humanizeError } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';
  import type { MyBoxCapacity, MyBoxServerSlot } from '#lib/types.ts';
  import Modal from './Modal.svelte';
  import { apiCode } from './util.ts';

  let {
    cap,
    server,
    onclose,
    ondone,
  }: { cap: MyBoxCapacity; server: MyBoxServerSlot; onclose: () => void; ondone: () => void } = $props();

  // svelte-ignore state_referenced_locally
  let cpu = $state(server.cpu);
  // svelte-ignore state_referenced_locally
  let ram = $state(server.ram);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let needsStop = $state(false);
  let stopping = $state(false);

  /** Ce qui reste, plus ce que le serveur occupe déjà s'il est actif. Pro : pas de borne de MyBox. */
  const maxCpu = $derived(cap.pro ? Math.max(server.cpu * 2, 2) : cap.cpu.remaining + (server.paused ? 0 : server.cpu));
  const maxRam = $derived(cap.pro ? Math.max(server.ram * 2, 8) : cap.ram.remaining + (server.paused ? 0 : server.ram));
  const changed = $derived(cpu !== server.cpu || ram !== server.ram);

  async function submit() {
    busy = true;
    error = null;
    needsStop = false;
    try {
      await api.myboxResizeServer(cap.id, server.id, cpu, ram);
      ondone();
    } catch (e) {
      needsStop = apiCode(e) === 'API_MYBOX_NEED_OFFLINE_SERVER';
      error = humanizeError(e);
    } finally {
      busy = false;
    }
  }

  async function stop() {
    stopping = true;
    try {
      await api.powerAction(server.id, 'stop');
      error = t('mybox.resize.stopSent');
    } catch (e) {
      error = humanizeError(e);
    } finally {
      stopping = false;
    }
  }
</script>

<Modal title={t('mybox.resize.title', { name: server.name })} {onclose}>
  {#if server.paused}<p class="dim">{t('mybox.resize.pausedHint')}</p>{/if}
  <label class="field">
    <span class="label">{t('mybox.cpuLabel', { n: cpu })}</span>
    <input type="range" min="1" max={Math.max(1, maxCpu)} step="1" bind:value={cpu} />
  </label>
  <label class="field">
    <span class="label">{t('mybox.ramLabel', { n: ram })}</span>
    <input type="range" min="1" max={Math.max(1, maxRam)} step="1" bind:value={ram} />
  </label>
  {#if !cap.pro}
    <p class="dim">
      {t('mybox.resize.after', {
        cpu: maxCpu - cpu,
        ram: maxRam - ram,
      })}
    </p>
  {/if}
  {#if error}<p class="alert">{error}</p>{/if}
  <div class="actions">
    {#if needsStop}
      <button class="btn btn--ghost" onclick={stop} disabled={stopping}>
        {#if stopping}<span class="spinner"></span>{/if}
        {t('mybox.resize.stop')}
      </button>
    {/if}
    <span class="spacer"></span>
    <button class="btn btn--ghost" onclick={onclose}>{t('common.cancel')}</button>
    <button class="btn" onclick={submit} disabled={busy || !changed}>
      {#if busy}<span class="spinner"></span>{/if}
      {t('mybox.resize.submit')}
    </button>
  </div>
</Modal>

<style>
  input[type='range'] {
    width: 100%;
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
