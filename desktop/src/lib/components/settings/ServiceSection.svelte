<script lang="ts">
  /**
   * Réglages « Service d'arrière-plan » : état du service système, qui supervise, et actions
   * (activer / désactiver / redémarrer / désinstaller) dans la limite des droits. Les actions
   * d'administration déclenchent l'élévation du système (UAC, pkexec, osascript) côté Rust.
   * Conception : docs/SERVICE.md.
   */
  import { onMount, onDestroy } from 'svelte';
  import { confirm } from '@tauri-apps/plugin-dialog';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';
  import type { ServiceOverview } from '#lib/types.ts';

  type Busy = '' | 'enable' | 'disable' | 'restart' | 'update' | 'uninstall';

  let overview = $state<ServiceOverview | null>(null);
  let loading = $state(false);
  let busy = $state<Busy>('');
  let message = $state<{ ok: boolean; text: string } | null>(null);
  let purge = $state(false);
  let copied = $state(false);
  let now = $state(Math.floor(Date.now() / 1000));
  let timer: ReturnType<typeof setInterval> | undefined;
  let copyTimer: ReturnType<typeof setTimeout> | undefined;

  const status = $derived(overview?.status ?? null);
  /** Le service est utilisable depuis cette installation. */
  const available = $derived(!!overview && overview.supported && overview.binary_available);
  const byService = $derived(overview?.supervision === 'service');
  const linked = $derived(overview?.link === 'ok');
  const canEnable = $derived(available && !byService && busy === '');
  const canDisable = $derived(linked && !!status?.has_key && busy === '');
  const canRestart = $derived(linked && !!status?.managed && busy === '');
  const canUninstall = $derived(
    available && overview?.install_state !== 'not_installed' && busy === '',
  );
  /** Indication contextuelle sous l'état (pourquoi le service ne supervise pas). */
  const hint = $derived.by(() => {
    if (!overview || !available || byService) return '';
    if (overview.install_state === 'not_installed') return t('service.notInstalledHint');
    if (overview.link === 'forbidden') return t('service.forbidden');
    if (linked && status && !status.has_key) return t('service.waitingKey');
    if (overview.same_account === false) return t('service.otherAccount');
    return '';
  });

  async function refresh() {
    loading = true;
    try {
      overview = await api.serviceOverview();
      now = Math.floor(Date.now() / 1000);
    } catch (e) {
      message = { ok: false, text: humanizeError(e) };
    } finally {
      loading = false;
    }
  }

  async function run(kind: Busy, action: () => Promise<ServiceOverview | void>, okText: string) {
    busy = kind;
    message = null;
    try {
      const next = await action();
      if (next) overview = next;
      else await refresh();
      message = { ok: true, text: okText };
    } catch (e) {
      message = { ok: false, text: humanizeError(e) };
      await refresh();
    } finally {
      busy = '';
    }
  }

  const enable = () => run('enable', api.serviceEnable, t('service.enabled'));
  const disable = () => run('disable', api.serviceDisable, t('service.disabled'));
  const update = () => run('update', api.serviceUpdate, t('service.updated'));
  const restart = () =>
    run('restart', async () => {
      await api.serviceRestart();
    }, t('service.restarted'));

  async function uninstall() {
    const ok = await confirm(t('service.confirmUninstall'), { kind: 'warning' });
    if (!ok) return;
    await run('uninstall', () => api.serviceUninstall(purge), t('service.uninstalled'));
  }

  async function copyManual() {
    if (!overview?.manual_enable) return;
    try {
      await navigator.clipboard.writeText(overview.manual_enable);
      copied = true;
      clearTimeout(copyTimer);
      copyTimer = setTimeout(() => (copied = false), 1500);
    } catch {
      /* presse-papiers indisponible */
    }
  }

  onMount(() => {
    void refresh();
    timer = setInterval(() => void refresh(), 15000);
  });
  onDestroy(() => {
    clearInterval(timer);
    clearTimeout(copyTimer);
  });
</script>

<h1>{t('service.title')}</h1>
<p class="desc dim">{t('service.intro')}</p>

{#if !overview}
  <p class="desc dim">{t('service.loading')}</p>
{:else if !overview.supported}
  <p class="note">{t('service.unsupported')}</p>
{:else if !overview.binary_available}
  <p class="note">{t('service.noBinary')}</p>
{/if}

{#if overview && available}
  <div class="row">
    <span class="label">{t('service.stateLabel')}</span>
    <span class="badge" data-state={overview.install_state}>
      <span class="led"></span>{t(`service.state.${overview.install_state}`)}
    </span>
  </div>
  <div class="row">
    <span class="label">{t('service.supervisionLabel')}</span>
    <span class="val">{byService ? t('service.byService') : t('service.byApp')}</span>
  </div>
  {#if status}
    <div class="row">
      <span class="label">{t('service.accountLabel')}</span>
      <span class="val">
        {#if status.has_key && status.account_name}
          {status.account_name}{#if status.account_id != null}&nbsp;<span class="dim">#{status.account_id}</span>{/if}
          <!-- Le service garde le compte avec lequel il a été activé, quel que soit le compte actif. -->
          {#if overview.same_account === true}
            <span class="dim">· {t('service.accountActive')}</span>
          {:else if overview.same_account === false}
            <span class="other">· {t('service.accountOther')}</span>
          {/if}
        {:else}
          {t('service.noKey')}
        {/if}
      </span>
    </div>
    {#if status.has_key}
      <div class="row">
        <span class="label">{t('service.activityLabel')}</span>
        <span class="val">
          {t('service.monitored', { n: status.monitored })} ·
          {status.last_poll != null
            ? t('service.lastPoll', { s: Math.max(0, now - status.last_poll) })
            : t('service.noPoll')}
        </span>
      </div>
    {/if}
    <div class="row">
      <span class="label">{t('service.versionLabel')}</span>
      <span class="val mono">{t('service.version', { v: status.version, pid: status.pid })}</span>
    </div>
    <div class="row">
      <span class="label">{t('service.dataLabel')}</span>
      <span class="val mono">{status.data_dir}</span>
    </div>
    <div class="row">
      <span class="label">{t('service.pushLabel')}</span>
      <span class="val">{status.fcm ? t('service.pushOn') : t('service.pushOff')}</span>
    </div>
  {/if}

  {#if hint}
    <p class="note">{hint}</p>
  {/if}

  {#if overview.outdated && status}
    <div class="note">
      <div class="note-title">{t('service.outdatedTitle')}</div>
      <p>{t('service.outdatedBody', { v: status.version })}</p>
      <button class="btn" onclick={update} disabled={busy !== ''}>
        {busy === 'update' ? t('service.updating') : t('service.update')}
      </button>
    </div>
  {/if}

  <div class="actions">
    {#if !byService}
      <button class="btn" onclick={enable} disabled={!canEnable}>
        {busy === 'enable'
          ? t('service.enabling')
          : overview.same_account === false
            ? t('service.enableActive')
            : t('service.enable')}
      </button>
    {/if}
    {#if status?.has_key}
      <button class="btn btn--ghost" onclick={disable} disabled={!canDisable}>
        {busy === 'disable' ? t('service.disabling') : t('service.disable')}
      </button>
    {/if}
    {#if linked && status?.managed}
      <button class="btn btn--ghost" onclick={restart} disabled={!canRestart}>
        {t('service.restart')}
      </button>
    {/if}
    <button class="btn btn--ghost" onclick={refresh} disabled={loading || busy !== ''}>
      {t('service.refresh')}
    </button>
  </div>
  {#if !byService}
    <p class="cmd dim">{t(`service.elevation.${overview.elevation}`)}</p>
  {/if}
  {#if message}
    <p class="msg" class:ok={message.ok}>{message.text}</p>
  {/if}

  {#if overview.manual_enable && !byService}
    <h2>{t('service.manualTitle')}</h2>
    <p class="desc dim">{t('service.manualHint')}</p>
    <div class="snippet">
      <pre>{overview.manual_enable}</pre>
      <button class="btn btn--ghost copy" onclick={copyManual}>
        {copied ? t('service.copied') : t('service.copy')}
      </button>
    </div>
  {/if}

  <h2>{t('service.notificationsTitle')}</h2>
  <p class="desc dim">{t('service.notificationsBody')}</p>

  {#if overview.install_state !== 'not_installed'}
    <h2>{t('service.uninstall')}</h2>
    <label class="check">
      <input type="checkbox" bind:checked={purge} />
      <span>{t('service.purge')}</span>
    </label>
    <div class="actions">
      <button class="btn btn--ghost danger" onclick={uninstall} disabled={!canUninstall}>
        {busy === 'uninstall' ? t('service.uninstalling') : t('service.uninstall')}
      </button>
    </div>
  {/if}
{/if}

<style>
  h1 {
    margin: 0 0 22px;
    font-size: 22px;
    letter-spacing: -0.02em;
  }
  h2 {
    margin: 26px 0 12px;
    font-size: 16px;
    color: var(--text);
    font-weight: 900;
  }
  .desc {
    font-size: 13px;
    margin: 0 0 10px;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 12px 0;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
  }
  .label {
    font-size: 14px;
    font-weight: 700;
    flex-shrink: 0;
  }
  .val {
    font-size: 13px;
    color: var(--text-muted);
    text-align: right;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .mono {
    font-family: var(--font-mono);
    font-size: 12px;
  }
  .other {
    color: var(--state-pending);
  }
  .badge[data-state='running'] {
    color: var(--state-running);
  }
  .badge[data-state='stopped'] {
    color: var(--state-pending);
  }
  .badge[data-state='not_installed'],
  .badge[data-state='unknown'] {
    color: var(--state-offline);
  }
  .note {
    font-size: 13px;
    line-height: 1.55;
    margin: 14px 0;
    padding: 11px 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    color: var(--text-muted);
  }
  .note p {
    margin: 4px 0 0;
  }
  .note-title {
    font-weight: 800;
    color: var(--text);
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 16px;
  }
  .danger {
    color: var(--state-danger);
  }
  .cmd {
    font-size: 12.5px;
    margin: 10px 0 0;
  }
  .msg {
    font-size: 13px;
    margin: 10px 0 0;
    color: var(--state-danger);
  }
  .msg.ok {
    color: var(--state-running);
  }
  .check {
    display: flex;
    align-items: center;
    gap: 9px;
    font-size: 13px;
    cursor: pointer;
  }
  .snippet {
    position: relative;
  }
  .snippet pre {
    margin: 0;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 14px 16px;
    padding-right: 84px;
    overflow-x: auto;
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.6;
    color: var(--text-muted);
    white-space: pre-wrap;
    word-break: break-all;
  }
  .snippet .copy {
    position: absolute;
    top: 8px;
    right: 8px;
    padding: 5px 12px;
    font-size: 12px;
  }
</style>
