<script lang="ts">
  /**
   * Tâches planifiées du panel MineStrator d'un serveur : exécutées par l'hébergeur, 24 h/24,
   * même ordinateur éteint — à distinguer des automatisations de l'app (vue parente). Mêmes
   * données que l'onglet « Tâches » du panel web ; le cœur valide toute saisie avant envoi.
   */
  import { onMount } from 'svelte';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { t, getLocale } from '#lib/i18n/index.ts';
  import type { PanelCron, PanelSchedule, PanelTask } from '#lib/types.ts';
  import { describeCron, detectPreset, presetCron, type CronPreset } from '#lib/automations/cron.ts';
  import Icon from '../Icon.svelte';

  let { serverId }: { serverId: number } = $props();

  let schedules = $state<PanelSchedule[] | null>(null);
  let error = $state<string | null>(null);
  let busy = $state<string | null>(null);
  let pendingDelete = $state<string | null>(null);
  let notice = $state<string | null>(null);

  // Édition d'une tâche planifiée (`id` null : création).
  type ScheduleDraft = {
    id: number | null;
    name: string;
    preset: CronPreset;
    time: string;
    days: number[];
    minute: number;
    cron: PanelCron;
    is_active: boolean;
    only_when_online: boolean;
    error: string | null;
  };
  let draft = $state<ScheduleDraft | null>(null);

  // Édition d'une action (`taskId` null : ajout).
  type TaskDraft = {
    scheduleId: number;
    taskId: number | null;
    action: 'command' | 'power';
    command: string;
    power: string;
    time_offset: number;
    continue_on_failure: boolean;
    error: string | null;
  };
  let taskDraft = $state<TaskDraft | null>(null);

  const POWER = ['start', 'restart', 'stop', 'kill'] as const;
  const ISO_DAYS = [1, 2, 3, 4, 5, 6, 7];

  onMount(load);

  async function load() {
    error = null;
    try {
      schedules = await api.panelSchedules(serverId);
    } catch (e) {
      error = humanizeError(e);
    }
  }

  function flash(msg: string) {
    notice = msg;
    setTimeout(() => (notice = null), 2500);
  }

  async function run(key: string, fn: () => Promise<unknown>, done?: string): Promise<boolean> {
    busy = key;
    try {
      await fn();
      await load();
      if (done) flash(done);
      return true;
    } catch (e) {
      error = humanizeError(e);
      return false;
    } finally {
      busy = null;
    }
  }

  function fmtDate(s: string | null): string {
    if (!s) return '';
    const d = new Date(s.replace(' ', 'T'));
    return Number.isNaN(d.getTime())
      ? s
      : d.toLocaleString(getLocale(), { day: '2-digit', month: '2-digit', hour: '2-digit', minute: '2-digit' });
  }

  function dayLabel(iso: number): string {
    const base = new Date(Date.UTC(2024, 0, iso)); // 1er janvier 2024 = lundi
    return new Intl.DateTimeFormat(getLocale(), { weekday: 'short', timeZone: 'UTC' }).format(base);
  }

  function taskText(task: PanelTask): string {
    const what =
      task.action === 'power'
        ? t('panelSchedules.taskPower', { action: t(`panelSchedules.power.${task.payload}`) })
        : t('panelSchedules.taskCommand', { command: task.payload });
    return task.time_offset > 0 ? `${what} — ${t('panelSchedules.after', { s: task.time_offset })}` : what;
  }

  // --- Tâche planifiée -------------------------------------------------------

  function startNew() {
    taskDraft = null;
    draft = {
      id: null,
      name: '',
      preset: 'daily',
      time: '04:00',
      days: [1, 2, 3, 4, 5, 6, 7],
      minute: 0,
      cron: { minute: '0', hour: '4', day_of_month: '*', month: '*', day_of_week: '*' },
      is_active: true,
      only_when_online: false,
      error: null,
    };
  }

  function startEdit(s: PanelSchedule) {
    taskDraft = null;
    const p = detectPreset(s.cron);
    draft = {
      id: s.id,
      name: s.name,
      ...p,
      cron: { ...s.cron },
      is_active: s.is_active,
      only_when_online: s.only_when_online,
      error: null,
    };
  }

  function draftCron(d: ScheduleDraft): PanelCron | null {
    return d.preset === 'custom' ? d.cron : presetCron(d.preset, d.time, d.days, d.minute);
  }

  async function saveDraft() {
    const d = draft;
    if (!d) return;
    const cron = draftCron(d);
    if (!cron) {
      d.error = t('panelSchedules.pickDays');
      return;
    }
    const input = { name: d.name, cron, is_active: d.is_active, only_when_online: d.only_when_online };
    busy = 'draft';
    try {
      if (d.id === null) await api.panelScheduleCreate(serverId, input);
      else await api.panelScheduleUpdate(serverId, d.id, input);
      draft = null;
      await load();
      flash(t('panelSchedules.saved'));
    } catch (e) {
      d.error = humanizeError(e);
    } finally {
      busy = null;
    }
  }

  function toggleActive(s: PanelSchedule) {
    void run(
      `s${s.id}`,
      () =>
        api.panelScheduleUpdate(serverId, s.id, {
          name: s.name,
          cron: s.cron,
          is_active: !s.is_active,
          only_when_online: s.only_when_online,
        }),
    );
  }

  function removeSchedule(s: PanelSchedule) {
    const key = `s${s.id}`;
    if (pendingDelete !== key) {
      pendingDelete = key;
      return;
    }
    pendingDelete = null;
    void run(key, () => api.panelScheduleDelete(serverId, s.id), t('panelSchedules.deleted'));
  }

  // --- Actions ---------------------------------------------------------------

  function startTask(s: PanelSchedule, task?: PanelTask) {
    draft = null;
    taskDraft = {
      scheduleId: s.id,
      taskId: task?.id ?? null,
      action: task?.action === 'power' ? 'power' : 'command',
      command: task?.action === 'command' ? task.payload : '',
      power: task?.action === 'power' ? task.payload : 'restart',
      time_offset: task?.time_offset ?? 0,
      continue_on_failure: task?.continue_on_failure ?? false,
      error: null,
    };
  }

  async function saveTask() {
    const d = taskDraft;
    if (!d) return;
    const input = {
      action: d.action,
      payload: d.action === 'power' ? d.power : d.command,
      time_offset: Math.round(Number(d.time_offset) || 0),
      continue_on_failure: d.continue_on_failure,
    };
    busy = 'task';
    try {
      if (d.taskId === null) await api.panelTaskAdd(serverId, d.scheduleId, input);
      else await api.panelTaskUpdate(serverId, d.scheduleId, d.taskId, input);
      taskDraft = null;
      await load();
      flash(t('panelSchedules.saved'));
    } catch (e) {
      d.error = humanizeError(e);
    } finally {
      busy = null;
    }
  }

  function removeTask(s: PanelSchedule, task: PanelTask) {
    const key = `t${task.id}`;
    if (pendingDelete !== key) {
      pendingDelete = key;
      return;
    }
    pendingDelete = null;
    void run(key, () => api.panelTaskDelete(serverId, s.id, task.id), t('panelSchedules.deleted'));
  }

  function move(s: PanelSchedule, index: number, delta: number) {
    const ids = s.tasks.map((x) => x.id);
    const j = index + delta;
    if (j < 0 || j >= ids.length) return;
    [ids[index], ids[j]] = [ids[j], ids[index]];
    void run(`s${s.id}`, () => api.panelTasksReorder(serverId, s.id, ids));
  }
</script>

<section>
  <div class="sec-head">
    <h2>{t('panelSchedules.title')}</h2>
    {#if !draft}
      <button class="btn btn--ghost small-btn" onclick={startNew}>
        <Icon name="plus" size={14} />
        {t('panelSchedules.add')}
      </button>
    {/if}
  </div>
  <p class="dim hint">{t('panelSchedules.intro')}</p>

  {#if notice}<p class="ok">{notice}</p>{/if}
  {#if error}<p class="alert">{error}</p>{/if}

  {#if draft}
    {@const d = draft}
    <div class="card block editor">
      <div class="ed-title">{d.id === null ? t('panelSchedules.new') : t('panelSchedules.editTitle')}</div>
      <label class="field">
        <span>{t('panelSchedules.name')}</span>
        <input class="input" bind:value={d.name} maxlength="100" placeholder={t('panelSchedules.namePlaceholder')} />
      </label>
      <div class="seg" role="group" aria-label={t('panelSchedules.when')}>
        {#each ['daily', 'weekly', 'hourly', 'custom'] as const as p (p)}
          <button class:on={d.preset === p} onclick={() => (d.preset = p)}>{t(`panelSchedules.preset.${p}`)}</button>
        {/each}
      </div>
      {#if d.preset === 'daily' || d.preset === 'weekly'}
        <label class="inline">
          <span>{t('panelSchedules.at')}</span>
          <input class="input time" type="time" bind:value={d.time} />
        </label>
      {/if}
      {#if d.preset === 'weekly'}
        <div class="days">
          {#each ISO_DAYS as day (day)}
            <label class="day" class:on={d.days.includes(day)}>
              <input
                type="checkbox"
                checked={d.days.includes(day)}
                onchange={(e) =>
                  (d.days = e.currentTarget.checked ? [...d.days, day] : d.days.filter((x) => x !== day))}
              />
              {dayLabel(day)}
            </label>
          {/each}
        </div>
      {/if}
      {#if d.preset === 'hourly'}
        <label class="inline">
          <span>{t('panelSchedules.atMinute')}</span>
          <input class="input num" type="number" min="0" max="59" bind:value={d.minute} />
        </label>
      {/if}
      {#if d.preset === 'custom'}
        <div class="cron">
          {#each [['minute', 'cronMinute'], ['hour', 'cronHour'], ['day_of_month', 'cronDom'], ['month', 'cronMonth'], ['day_of_week', 'cronDow']] as const as [field, label] (field)}
            <label class="cron-f">
              <span>{t(`panelSchedules.${label}`)}</span>
              <input class="input mono" bind:value={d.cron[field]} />
            </label>
          {/each}
        </div>
        <p class="dim small">{t('panelSchedules.cronHelp')}</p>
      {/if}
      {#if draftCron(d)}
        <p class="dim small">{describeCron(draftCron(d)!, t, getLocale())}</p>
      {/if}
      <label class="inline check">
        <input type="checkbox" bind:checked={d.is_active} />
        <span>{t('panelSchedules.active')}</span>
      </label>
      <label class="inline check">
        <input type="checkbox" bind:checked={d.only_when_online} />
        <span>{t('panelSchedules.onlyWhenOnline')}</span>
      </label>
      {#if d.id === null}<p class="dim small">{t('panelSchedules.addActionsAfter')}</p>{/if}
      {#if d.error}<p class="alert">{d.error}</p>{/if}
      <div class="ed-actions">
        <button class="btn btn--ghost" onclick={() => (draft = null)} disabled={busy === 'draft'}>{t('common.cancel')}</button>
        <button class="btn" onclick={saveDraft} disabled={busy === 'draft'}>
          {#if busy === 'draft'}<span class="spinner"></span>{/if}
          {t('panelSchedules.save')}
        </button>
      </div>
    </div>
  {/if}

  {#if schedules === null && !error}
    <p class="dim empty">…</p>
  {:else if schedules && schedules.length === 0}
    {#if !draft}<p class="dim empty">{t('panelSchedules.none')}</p>{/if}
  {:else if schedules}
    <div class="list">
      {#each schedules as s (s.id)}
        <div class="sched" class:disabled={!s.is_active}>
          <div class="row">
            <span class="ic"><Icon name="clock" size={16} /></span>
            <div class="meta">
              <span class="name">
                {s.name}
                {#if s.is_processing}<span class="badge">{t('panelSchedules.running')}</span>{/if}
              </span>
              <span class="sub-line dim">
                {describeCron(s.cron, t, getLocale())}{#if s.only_when_online} · {t('panelSchedules.onlyWhenOnlineShort')}{/if}
              </span>
              <span class="sub-line dim">
                {#if s.is_active && s.next_run_at}
                  {t('panelSchedules.next', { date: fmtDate(s.next_run_at) })}
                {:else if !s.is_active}
                  {t('panelSchedules.inactive')}
                {/if}
                {#if s.last_run_at} · {t('panelSchedules.last', { date: fmtDate(s.last_run_at) })}{/if}
              </span>
            </div>
            <div class="acts">
              <label class="switch" title={t('panelSchedules.active')}>
                <input type="checkbox" checked={s.is_active} disabled={busy === `s${s.id}`} onchange={() => toggleActive(s)} />
              </label>
              <button class="mini" title={t('panelSchedules.edit')} aria-label={t('panelSchedules.edit')} onclick={() => startEdit(s)}>
                <Icon name="pencil" size={14} />
              </button>
              {#if pendingDelete === `s${s.id}`}
                <button class="mini" onclick={() => (pendingDelete = null)} aria-label={t('common.cancel')}><Icon name="x" size={14} /></button>
              {/if}
              <button
                class="mini danger"
                class:armed={pendingDelete === `s${s.id}`}
                aria-label={t('panelSchedules.delete')}
                disabled={busy === `s${s.id}`}
                onclick={() => removeSchedule(s)}
              >
                {#if pendingDelete === `s${s.id}`}{t('panelSchedules.confirmDelete', { name: s.name })}{:else}<Icon name="trash" size={14} />{/if}
              </button>
            </div>
          </div>

          <ol class="tasks">
            {#each s.tasks as task, i (task.id)}
              <li>
                <span class="ic"><Icon name={task.action === 'power' ? 'power' : 'terminal'} size={13} /></span>
                <span class="task-text">{taskText(task)}{#if task.continue_on_failure} · {t('panelSchedules.continueShort')}{/if}</span>
                <span class="acts">
                  <button class="mini" disabled={i === 0 || busy !== null} aria-label={t('panelSchedules.up')} onclick={() => move(s, i, -1)}><Icon name="arrow-up" size={12} /></button>
                  <button class="mini" disabled={i === s.tasks.length - 1 || busy !== null} aria-label={t('panelSchedules.down')} onclick={() => move(s, i, 1)}><Icon name="arrow-down" size={12} /></button>
                  <button class="mini" aria-label={t('panelSchedules.edit')} onclick={() => startTask(s, task)}><Icon name="pencil" size={12} /></button>
                  <button
                    class="mini danger"
                    class:armed={pendingDelete === `t${task.id}`}
                    aria-label={t('panelSchedules.delete')}
                    disabled={busy === `t${task.id}`}
                    onclick={() => removeTask(s, task)}
                  >
                    {#if pendingDelete === `t${task.id}`}{t('panelSchedules.confirmDeleteAction')}{:else}<Icon name="trash" size={12} />{/if}
                  </button>
                </span>
              </li>
            {:else}
              <li class="dim empty-task">{t('panelSchedules.noActions')}</li>
            {/each}
          </ol>

          {#if taskDraft && taskDraft.scheduleId === s.id}
            {@const td = taskDraft}
            <div class="card block editor task-editor">
              <div class="ed-title">{td.taskId === null ? t('panelSchedules.newAction') : t('panelSchedules.editAction')}</div>
              <div class="seg" role="group">
                <button class:on={td.action === 'command'} onclick={() => (td.action = 'command')}>{t('panelSchedules.actionCommand')}</button>
                <button class:on={td.action === 'power'} onclick={() => (td.action = 'power')}>{t('panelSchedules.actionPower')}</button>
              </div>
              {#if td.action === 'command'}
                <input class="input mono" bind:value={td.command} maxlength="500" placeholder={t('panelSchedules.commandPlaceholder')} />
              {:else}
                <select class="input" bind:value={td.power}>
                  {#each POWER as p (p)}<option value={p}>{t(`panelSchedules.power.${p}`)}</option>{/each}
                </select>
              {/if}
              <label class="inline">
                <span>{t('panelSchedules.delay')}</span>
                <input class="input num" type="number" min="0" max="900" bind:value={td.time_offset} />
              </label>
              <label class="inline check">
                <input type="checkbox" bind:checked={td.continue_on_failure} />
                <span>{t('panelSchedules.continueOnFailure')}</span>
              </label>
              {#if td.error}<p class="alert">{td.error}</p>{/if}
              <div class="ed-actions">
                <button class="btn btn--ghost" onclick={() => (taskDraft = null)} disabled={busy === 'task'}>{t('common.cancel')}</button>
                <button class="btn" onclick={saveTask} disabled={busy === 'task'}>
                  {#if busy === 'task'}<span class="spinner"></span>{/if}
                  {t('panelSchedules.save')}
                </button>
              </div>
            </div>
          {:else}
            <button class="linkbtn add-action" onclick={() => startTask(s)}>
              <Icon name="plus" size={12} /> {t('panelSchedules.addAction')}
            </button>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</section>

<style>
  section {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  h2 {
    margin: 0;
    font-size: 16px;
    color: var(--text);
    font-weight: 900;
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
    line-height: 1.5;
  }
  .small {
    font-size: 12px;
    margin: 0;
  }
  .ok {
    margin: 0;
    font-size: 13px;
    color: var(--brand-primary);
  }
  .alert {
    margin: 0;
    font-size: 13px;
    color: var(--state-danger);
  }
  .empty {
    font-size: 13px;
    font-style: italic;
    margin: 2px 0;
  }
  .block {
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .editor {
    border-color: color-mix(in srgb, var(--brand-primary) 45%, var(--border));
  }
  .task-editor {
    margin: 0 12px 12px 40px;
  }
  .ed-title {
    font-weight: 800;
    font-size: 14px;
  }
  .ed-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 5px;
    font-size: 13px;
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
  .mono {
    font-family: var(--font-mono);
  }
  .seg {
    display: inline-flex;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
    align-self: flex-start;
  }
  .seg button {
    border: none;
    background: none;
    color: var(--text-muted);
    padding: 6px 12px;
    font: inherit;
    font-size: 12.5px;
    cursor: pointer;
  }
  .seg button.on {
    background: color-mix(in srgb, var(--brand-primary) 18%, transparent);
    color: var(--text);
  }
  .days {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .day {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    border: 1px solid var(--border);
    border-radius: 7px;
    padding: 4px 9px;
    font-size: 12.5px;
    cursor: pointer;
  }
  .day.on {
    border-color: var(--brand-primary);
  }
  .day input {
    display: none;
  }
  .cron {
    display: grid;
    grid-template-columns: repeat(5, minmax(0, 1fr));
    gap: 8px;
  }
  .cron-f {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 11.5px;
    color: var(--text-muted);
  }
  .cron-f .input {
    padding: 6px 8px;
  }
  .list {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
  }
  .sched {
    border-bottom: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
  }
  .sched:last-child {
    border-bottom: none;
  }
  .sched.disabled .meta {
    opacity: 0.55;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
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
    font-weight: 800;
    font-size: 13.5px;
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
  .badge {
    font-size: 11px;
    font-weight: 800;
    color: var(--brand-primary);
  }
  .sub-line {
    font-size: 12px;
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
    padding: 4px 8px;
  }
  .mini:hover:not(:disabled) {
    color: var(--text);
    border-color: var(--text-dim);
  }
  .mini:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .mini.armed {
    background: var(--danger-bg);
    border-color: var(--danger-bg);
    color: var(--danger-ink);
  }
  .tasks {
    list-style: none;
    margin: 0;
    padding: 0 12px 6px 40px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .tasks li {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12.5px;
  }
  .task-text {
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .empty-task {
    font-style: italic;
  }
  .linkbtn {
    border: none;
    background: none;
    color: var(--brand-primary);
    cursor: pointer;
    font: inherit;
    font-size: 12.5px;
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .add-action {
    margin: 0 0 10px 40px;
  }
</style>
