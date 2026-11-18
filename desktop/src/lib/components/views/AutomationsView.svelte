<script lang="ts">
  /**
   * Vue serveur « Automatisations » : redémarrage automatique après crash, tâches planifiées et
   * historique d'exécution. Tout est exécuté par le superviseur actif (l'app ou son service
   * d'arrière-plan, jamais les deux) — cf. crates/minestrator-core/src/automation.rs. Cette vue
   * ne fait que configurer et consulter ; elle le dit clairement, pour ne pas confondre avec les
   * tâches planifiées du panel web MineStrator.
   */
  import { onMount, onDestroy } from 'svelte';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';
  import type { ServerTab } from '#lib/tabs/tabs.svelte.ts';
  import type { AutomationRun, ServerAutomations, TaskInfo } from '#lib/types.ts';
  import {
    actionIcon,
    actionSummary,
    draftFrom,
    draftToTask,
    fmtDateTime,
    fmtDuration,
    newDraft,
    scheduleSummary,
    type TaskDraft,
  } from '#lib/automations/format.ts';
  import Icon from '../Icon.svelte';
  import PanelSchedulesSection from './PanelSchedulesSection.svelte';

  let { tab }: { tab: ServerTab } = $props();
  const serverId = $derived(tab.serverId);

  let data = $state<ServerAutomations | null>(null);
  let runs = $state<AutomationRun[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let draft = $state<TaskDraft | null>(null);
  let draftError = $state<string | null>(null);
  let saving = $state(false);
  let ruleBusy = $state(false);
  /** Tâche dont la suppression attend confirmation (2 temps), et action en cours. */
  let pendingDelete = $state<string | null>(null);
  let busy = $state<string | null>(null);
  let openRun = $state<number | null>(null);
  let flash = $state<string | null>(null);
  let flashErr = $state(false);
  let flashTimer: ReturnType<typeof setTimeout> | undefined;
  let poll: ReturnType<typeof setInterval> | undefined;

  const delays = $derived((data?.backoff_s ?? []).map(fmtDuration).join(', '));

  onMount(() => {
    void load();
    // Le journal bouge pendant que la vue reste ouverte (onglets keep-alive).
    poll = setInterval(() => void loadRuns(), 30000);
  });
  onDestroy(() => {
    clearInterval(poll);
    clearTimeout(flashTimer);
  });

  async function load() {
    loading = true;
    error = null;
    try {
      data = await api.serverAutomations(serverId);
      await loadRuns();
    } catch (e) {
      error = humanizeError(e);
    } finally {
      loading = false;
    }
  }

  async function loadRuns() {
    try {
      runs = await api.automationRuns(serverId, 50);
    } catch {
      /* journal indisponible : la configuration reste utilisable */
    }
  }

  function showFlash(message: string, isErr: boolean) {
    flash = message;
    flashErr = isErr;
    clearTimeout(flashTimer);
    flashTimer = setTimeout(() => (flash = null), 2600);
  }

  async function saveRule(enabled: boolean, maxPerHour: number) {
    if (!data) return;
    ruleBusy = true;
    try {
      data.restart = await api.setAutoRestart(serverId, enabled, maxPerHour);
      showFlash(t('automations.saved'), false);
    } catch (e) {
      showFlash(humanizeError(e), true);
      await load();
    } finally {
      ruleBusy = false;
    }
  }

  function startNew() {
    draft = newDraft();
    draftError = null;
  }

  function startEdit(info: TaskInfo) {
    draft = draftFrom(info.task);
    draftError = null;
  }

  async function saveDraft() {
    if (!draft || saving) return;
    const task = draftToTask(draft, serverId, !!data?.broadcast);
    if (typeof task === 'string') {
      draftError = t(task);
      return;
    }
    saving = true;
    draftError = null;
    try {
      await api.saveScheduledTask(task);
      draft = null;
      showFlash(t('automations.saved'), false);
      data = await api.serverAutomations(serverId);
    } catch (e) {
      draftError = humanizeError(e);
    } finally {
      saving = false;
    }
  }

  async function toggleTask(info: TaskInfo) {
    busy = info.task.id;
    try {
      await api.saveScheduledTask({ ...info.task, enabled: !info.task.enabled });
      data = await api.serverAutomations(serverId);
    } catch (e) {
      showFlash(humanizeError(e), true);
    } finally {
      busy = null;
    }
  }

  async function removeTask(info: TaskInfo) {
    if (pendingDelete !== info.task.id) {
      pendingDelete = info.task.id;
      return;
    }
    pendingDelete = null;
    busy = info.task.id;
    try {
      await api.deleteScheduledTask(info.task.id);
      data = await api.serverAutomations(serverId);
    } catch (e) {
      showFlash(humanizeError(e), true);
    } finally {
      busy = null;
    }
  }

  function toggleDay(d: number) {
    if (!draft) return;
    draft.days = draft.days.includes(d) ? draft.days.filter((x) => x !== d) : [...draft.days, d];
  }

  function taskLabel(info: TaskInfo): string {
    return info.task.name || actionSummary(info.task.action);
  }

  function runLabel(r: AutomationRun): string {
    return r.kind === 'auto_restart' ? t('automations.autoRestartRun') : (r.task_name ?? r.kind);
  }

  function statusColor(status: AutomationRun['status']): string {
    switch (status) {
      case 'ok':
        return 'var(--state-running)';
      case 'failed':
      case 'gave_up':
        return 'var(--state-danger)';
      case 'missed':
        return 'var(--state-pending)';
      default:
        return 'var(--state-offline)';
    }
  }
</script>

<div class="auto">
  <header class="head">
    <div>
      <h1>{t('automations.title')}</h1>
    </div>
    <button
      class="btn btn--ghost"
      onclick={load}
      disabled={loading}
      aria-label={t('automations.refresh')}
      title={t('automations.refresh')}
    >
      {#if loading}<span class="spinner"></span>{/if}
      <Icon name="refresh-cw" size={15} />
    </button>
  </header>

  <!-- Qui exécute, et ce que ce n'est PAS : les tâches du panel web MineStrator. -->
  <div class="note">
    <span class="note-ic"><Icon name="info" size={16} /></span>
    <div>
      <p>{t('automations.intro')}</p>
      <p class="dim">{t('automations.notPanel')}</p>
      {#if data}
        <p class="executor">
          {data.executor === 'service' ? t('automations.executorService') : t('automations.executorApp')}
        </p>
      {/if}
    </div>
  </div>

  {#if error}
    <div class="state">
      <p class="alert">{t('automations.loadFailed')} — {error}</p>
      <button class="btn" onclick={load}>{t('common.retry')}</button>
    </div>
  {:else if !data}
    <div class="state"><span class="spinner"></span></div>
  {:else}
    <!-- ===== Redémarrage automatique après crash ===== -->
    <section>
      <h2>{t('automations.autoRestart')}</h2>
      <div class="card block">
        <label class="toggle-row">
          <div>
            <div class="tl-title">{t('automations.autoRestart')}</div>
            <div class="tl-desc dim">{t('automations.autoRestartDesc', { delays })}</div>
          </div>
          <input
            type="checkbox"
            checked={data.restart.enabled}
            disabled={ruleBusy}
            onchange={(e) => saveRule(e.currentTarget.checked, data!.restart.max_per_hour)}
          />
        </label>
        <div class="sub" class:off={!data.restart.enabled}>
          <label class="inline">
            <span>{t('automations.maxPerHour')}</span>
            <input
              class="input num"
              type="number"
              min="1"
              max="10"
              value={data.restart.max_per_hour}
              disabled={ruleBusy || !data.restart.enabled}
              onchange={(e) =>
                saveRule(true, Math.min(10, Math.max(1, Number(e.currentTarget.value) || 3)))}
            />
          </label>
          <p class="dim small">{t('automations.crashLogsHint')}</p>
        </div>
      </div>
    </section>

    <!-- ===== Tâches planifiées ===== -->
    <section>
      <div class="sec-head">
        <h2>{t('automations.tasks')}</h2>
        {#if !draft}
          <button class="btn btn--ghost small-btn" onclick={startNew}>
            <Icon name="plus" size={14} />
            {t('automations.add')}
          </button>
        {/if}
      </div>
      <p class="dim hint">
        {t('automations.tasksDesc', { min: Math.round(data.tolerance_s / 60) })}
      </p>

      {#if draft}
        {@render editor(draft)}
      {/if}

      {#if data.tasks.length === 0}
        {#if !draft}<p class="dim empty">{t('automations.noTasks')}</p>{/if}
      {:else}
        <div class="list">
          {#each data.tasks as info (info.task.id)}
            <div class="row" class:disabled={!info.task.enabled}>
              <span class="ic"><Icon name={actionIcon(info.task.action.type)} size={16} /></span>
              <div class="meta">
                <span class="name">{taskLabel(info)}</span>
                <span class="sub-line dim">
                  {scheduleSummary(info.task.schedule)}
                  {#if info.task.name}· {actionSummary(info.task.action)}{/if}
                </span>
                <span class="sub-line dim">
                  {#if info.task.enabled && info.next_run != null}
                    <Icon name="clock" size={12} />
                    {t('automations.nextRun', { date: fmtDateTime(info.next_run) })}
                  {:else}
                    {t('automations.disabled')}
                  {/if}
                </span>
              </div>
              <div class="acts">
                <label class="switch" title={t('automations.enabled')}>
                  <input
                    type="checkbox"
                    checked={info.task.enabled}
                    disabled={busy === info.task.id}
                    onchange={() => toggleTask(info)}
                  />
                </label>
                <button
                  class="mini"
                  aria-label={t('automations.edit')}
                  title={t('automations.edit')}
                  onclick={() => startEdit(info)}
                >
                  <Icon name="pencil" size={14} />
                </button>
                {#if pendingDelete === info.task.id}
                  <button class="mini" onclick={() => (pendingDelete = null)} aria-label={t('automations.cancel')}>
                    <Icon name="x" size={14} />
                  </button>
                {/if}
                <button
                  class="mini danger"
                  class:armed={pendingDelete === info.task.id}
                  aria-label={t('automations.delete')}
                  disabled={busy === info.task.id}
                  onclick={() => removeTask(info)}
                >
                  {#if pendingDelete === info.task.id}
                    {t('automations.confirmDelete', { name: taskLabel(info) })}
                  {:else}
                    <Icon name="trash" size={14} />
                  {/if}
                </button>
              </div>
            </div>
          {/each}
        </div>
      {/if}
    </section>

    <!-- ===== Tâches planifiées du panel (exécutées par MineStrator) ===== -->
    <PanelSchedulesSection serverId={tab.serverId} />

    <!-- ===== Historique d'exécution ===== -->
    <section>
      <h2>{t('automations.history')}</h2>
      {#if runs.length === 0}
        <p class="dim empty">{t('automations.noHistory')}</p>
      {:else}
        <div class="list">
          <!-- Pas de clé : les identifiants de l'app et du service peuvent se recouper. -->
          {#each runs as r, i}
            <div class="run">
              <div class="run-top">
                <span class="dot" style="background: {statusColor(r.status)}"></span>
                <span class="run-date dim">{fmtDateTime(r.started_ts)}</span>
                <span class="run-name">{runLabel(r)}</span>
                <span class="status" style="color: {statusColor(r.status)}">
                  {t(`automations.status.${r.status}`)}
                </span>
              </div>
              <div class="run-msg">
                {r.message}
                {#if r.details}
                  <button class="linkbtn" onclick={() => (openRun = openRun === i ? null : i)}>
                    {openRun === i ? t('alerts.hideDetails') : t('alerts.showDetails')}
                  </button>
                {/if}
              </div>
              {#if r.details && openRun === i}
                <pre class="details">{r.details}</pre>
              {/if}
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

{#snippet editor(d: TaskDraft)}
  <div class="card block editor">
    <div class="ed-title">{d.id ? t('automations.editTask') : t('automations.newTask')}</div>

    <label class="field">
      <span class="label">{t('automations.name')}</span>
      <input class="input" bind:value={d.name} maxlength="80" placeholder={t('automations.namePlaceholder')} />
    </label>

    <div class="field">
      <span class="label">{t('automations.action')}</span>
      <div class="seg">
        <button class:on={d.action === 'restart'} onclick={() => (d.action = 'restart')}>
          {t('automations.actionRestart')}
        </button>
        <button class:on={d.action === 'snapshot'} onclick={() => (d.action = 'snapshot')}>
          {t('automations.actionSnapshot')}
        </button>
        <button class:on={d.action === 'commands'} onclick={() => (d.action = 'commands')}>
          {t('automations.actionCommands')}
        </button>
      </div>
    </div>

    {#if d.action === 'restart'}
      {#if data?.broadcast}
        <label class="field">
          <span class="label">{t('automations.warnings')}</span>
          <input class="input" bind:value={d.warnings} placeholder="10, 5, 1" />
          <span class="dim small">{t('automations.warningsHint')}</span>
        </label>
        <label class="field">
          <span class="label">{t('automations.message')}</span>
          <input class="input" bind:value={d.message} maxlength="200" />
          <span class="dim small">
            {t('automations.messageHint')} · {t('automations.broadcastWith', { cmd: data.broadcast })}
          </span>
        </label>
      {:else}
        <p class="dim small">{t('automations.noBroadcast')}</p>
      {/if}
    {:else if d.action === 'snapshot'}
      <label class="field">
        <span class="label">{t('automations.snapshotName')}</span>
        <input
          class="input"
          bind:value={d.snapshotName}
          maxlength="60"
          placeholder={t('automations.snapshotNamePlaceholder')}
        />
      </label>
    {:else}
      <label class="field">
        <span class="label">{t('automations.commands')}</span>
        <textarea
          class="input"
          rows="4"
          bind:value={d.commands}
          spellcheck="false"
          placeholder={t('automations.commandsPlaceholder')}
        ></textarea>
      </label>
    {/if}

    <div class="field">
      <span class="label">{t('automations.schedule')}</span>
      <div class="seg">
        <button class:on={d.schedule === 'daily'} onclick={() => (d.schedule = 'daily')}>
          {t('automations.daily')}
        </button>
        <button class:on={d.schedule === 'weekly'} onclick={() => (d.schedule = 'weekly')}>
          {t('automations.weekly')}
        </button>
        <button class:on={d.schedule === 'interval'} onclick={() => (d.schedule = 'interval')}>
          {t('automations.interval')}
        </button>
      </div>
    </div>

    {#if d.schedule === 'weekly'}
      <div class="days">
        {#each [1, 2, 3, 4, 5, 6, 7] as day (day)}
          <button class="chip" class:on={d.days.includes(day)} onclick={() => toggleDay(day)}>
            {t(`automations.day.${day}`)}
          </button>
        {/each}
      </div>
    {/if}

    {#if d.schedule === 'interval'}
      <label class="inline">
        <span>{t('automations.everyMinutes')}</span>
        <input class="input num" type="number" min="5" step="5" bind:value={d.every} />
      </label>
    {:else}
      <label class="inline">
        <span>{t('automations.time')}</span>
        <input class="input time" type="time" bind:value={d.time} />
      </label>
    {/if}

    <label class="inline check">
      <input type="checkbox" bind:checked={d.enabled} />
      <span>{t('automations.enabled')}</span>
    </label>

    {#if draftError}<p class="alert">{draftError}</p>{/if}

    <div class="ed-actions">
      <button class="btn btn--ghost" onclick={() => (draft = null)} disabled={saving}>
        {t('automations.cancel')}
      </button>
      <button class="btn" onclick={saveDraft} disabled={saving}>
        {#if saving}<span class="spinner"></span>{/if}
        {saving ? t('automations.saving') : t('automations.save')}
      </button>
    </div>
  </div>
{/snippet}

<style>
  .auto {
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
  .note {
    display: flex;
    gap: 12px;
    padding: 12px 14px;
    border: 1px solid color-mix(in srgb, var(--brand-primary) 35%, var(--border));
    background: color-mix(in srgb, var(--brand-primary) 7%, var(--surface));
    border-radius: var(--radius);
    font-size: 13px;
    line-height: 1.55;
  }
  .note p {
    margin: 0 0 4px;
  }
  .note p:last-child {
    margin-bottom: 0;
  }
  .note-ic {
    flex: none;
    color: var(--brand-primary);
    padding-top: 2px;
  }
  .executor {
    font-weight: 600;
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
  .sec-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  .small-btn {
    padding: 6px 12px;
    font-size: 12.5px;
  }
  .hint {
    margin: -4px 0 4px;
    font-size: 12.5px;
  }
  .small {
    font-size: 12px;
    margin: 0;
  }
  .block {
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .toggle-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    cursor: pointer;
  }
  .tl-title {
    font-weight: 600;
    font-size: 14px;
  }
  .tl-desc {
    font-size: 12.5px;
    margin-top: 3px;
    line-height: 1.5;
  }
  .sub {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding-top: 10px;
    border-top: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
  }
  .sub.off {
    opacity: 0.5;
  }
  .inline {
    display: flex;
    align-items: center;
    gap: 12px;
    font-size: 13px;
  }
  .inline.check {
    cursor: pointer;
  }
  .num {
    width: 90px;
    padding: 7px 10px;
  }
  .time {
    width: 120px;
    padding: 7px 10px;
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
  .row:last-child,
  .run:last-child {
    border-bottom: none;
  }
  .row.disabled .meta {
    opacity: 0.55;
  }
  .ic {
    flex: none;
    display: inline-flex;
    color: var(--text-dim);
  }
  .meta {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .name {
    font-weight: 600;
    font-size: 13.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sub-line {
    font-size: 12px;
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
  .acts {
    flex: none;
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .switch {
    display: inline-flex;
    padding: 0 4px;
    cursor: pointer;
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
  .mini.armed {
    background: var(--state-danger);
    border-color: var(--state-danger);
    color: #fff;
  }
  .editor {
    border-color: color-mix(in srgb, var(--brand-primary) 45%, var(--border));
  }
  .ed-title {
    font-weight: 700;
    font-size: 14px;
  }
  .seg {
    display: inline-flex;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
    align-self: flex-start;
  }
  .seg button {
    background: none;
    border: none;
    border-right: 1px solid var(--border);
    color: var(--text-muted);
    font: inherit;
    font-size: 12.5px;
    padding: 7px 12px;
    cursor: pointer;
  }
  .seg button:last-child {
    border-right: none;
  }
  .seg button.on {
    background: color-mix(in srgb, var(--brand-primary) 18%, transparent);
    color: var(--text);
    font-weight: 600;
  }
  .days {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chip {
    border: 1px solid var(--border);
    border-radius: 999px;
    background: none;
    color: var(--text-muted);
    font: inherit;
    font-size: 12.5px;
    padding: 4px 11px;
    cursor: pointer;
  }
  .chip.on {
    border-color: var(--brand-primary);
    background: color-mix(in srgb, var(--brand-primary) 16%, transparent);
    color: var(--text);
  }
  textarea.input {
    resize: vertical;
    min-height: 80px;
  }
  .ed-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .run {
    padding: 9px 12px;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
    font-size: 13px;
  }
  .run-top {
    display: flex;
    align-items: center;
    gap: 9px;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex: none;
  }
  .run-date {
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    flex: none;
  }
  .run-name {
    font-weight: 600;
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .status {
    font-size: 12px;
    font-weight: 600;
    flex: none;
  }
  .run-msg {
    color: var(--text-muted);
    font-size: 12.5px;
    margin: 3px 0 0 17px;
  }
  .linkbtn {
    background: none;
    border: none;
    color: var(--brand-primary);
    cursor: pointer;
    font: inherit;
    font-size: 12px;
    padding: 0 0 0 6px;
  }
  .details {
    margin: 6px 0 0 17px;
    max-height: 260px;
    overflow: auto;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 10px 12px;
    font-family: var(--font-mono);
    font-size: 11.5px;
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-word;
    user-select: text;
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
