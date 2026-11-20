<script lang="ts">
  /**
   * Garde des agents MCP (Réglages → MCP) : portée des écritures (mode constructeur), journal des
   * appels reçus par le serveur MCP et modifications des agents, annulables par session. Toute la
   * logique vit dans le cœur (crates/minestrator-core/src/agent_guard.rs) ; l'annulation est
   * confirmée ici, avant l'appel.
   */
  import { onMount } from 'svelte';
  import { confirm } from '@tauri-apps/plugin-dialog';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';
  import { fmtDateTime } from '#lib/automations/format.ts';
  import type {
    ActionOutcome,
    AgentAction,
    Changeset,
    McpConfig,
    RollbackReport,
    RollbackStatus,
    ServerListItem,
    StepState,
    WriteScope,
  } from '#lib/types.ts';
  import Icon from '../Icon.svelte';

  let {
    config = $bindable(),
    servers,
    onsave,
  }: { config: McpConfig; servers: ServerListItem[]; onsave: () => void } = $props();

  let actions = $state<AgentAction[]>([]);
  let changesets = $state<Changeset[]>([]);
  let serverFilter = $state<number | null>(null);
  let onlyWrites = $state(false);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let busy = $state<string | null>(null);
  let open = $state<Record<string, boolean>>({});
  let reports = $state<Record<string, RollbackReport>>({});

  onMount(refresh);

  async function refresh() {
    loading = true;
    error = null;
    try {
      [actions, changesets] = await Promise.all([
        api.agentActions({
          server_id: serverFilter,
          session: null,
          since: null,
          only_writes: onlyWrites,
          limit: 100,
        }),
        api.agentChangesets(20),
      ]);
    } catch (e) {
      error = humanizeError(e);
    } finally {
      loading = false;
    }
  }

  function serverName(id: number): string {
    return servers.find((s) => s.id === id)?.name ?? t('agentGuard.serverN', { id });
  }

  function setScope(scope: WriteScope) {
    if (config.write_scope === scope) return;
    config.write_scope = scope;
    onsave();
  }

  function toggleAllowed(id: number) {
    config.allowed_servers = config.allowed_servers.includes(id)
      ? config.allowed_servers.filter((x) => x !== id)
      : [...config.allowed_servers, id];
    onsave();
  }

  const OUTCOME: Record<ActionOutcome, string> = {
    ok: 'agentGuard.outcomeOk',
    error: 'agentGuard.outcomeError',
    denied: 'agentGuard.outcomeDenied',
  };
  const STATE: Record<StepState, string> = {
    active: 'agentGuard.stateActive',
    undone: 'agentGuard.stateUndone',
    skipped: 'agentGuard.stateSkipped',
  };
  const REPORT: Record<RollbackStatus, string> = {
    undone: 'agentGuard.reportUndone',
    skipped: 'agentGuard.reportSkipped',
    refused: 'agentGuard.reportRefused',
    failed: 'agentGuard.reportFailed',
  };

  function argsText(a: AgentAction): string {
    return typeof a.args === 'string' ? a.args : JSON.stringify(a.args);
  }

  /** Étapes encore en place, et parmi elles celles qu'on sait défaire. */
  function counts(cs: Changeset) {
    const active = cs.steps.filter((s) => s.state === 'active');
    return { active: active.length, undoable: active.filter((s) => s.undoable).length };
  }

  async function undo(cs: Changeset, force: boolean) {
    const question = force
      ? t('agentGuard.confirmForce')
      : t('agentGuard.confirmUndo', { n: counts(cs).active });
    if (!(await confirm(question, { kind: 'warning' }))) return;
    busy = cs.id;
    error = null;
    try {
      reports[cs.id] = await api.rollbackChangeset(cs.id, null, force);
      open[cs.id] = true;
      await refresh();
    } catch (e) {
      error = humanizeError(e);
    } finally {
      busy = null;
    }
  }

  /** L'annulation s'est arrêtée sur une cible changée depuis : on peut la forcer. */
  function refused(cs: Changeset): boolean {
    const r = reports[cs.id];
    return !!r && r.stopped && r.steps.some((s) => s.status === 'refused') && counts(cs).active > 0;
  }
</script>

<h2>{t('agentGuard.scopeTitle')}</h2>
<p class="desc dim">{t('agentGuard.scopeDesc')}</p>
<div class="seg" role="radiogroup" aria-label={t('agentGuard.scopeTitle')}>
  <button
    role="radio"
    aria-checked={config.write_scope === 'all'}
    class:on={config.write_scope === 'all'}
    onclick={() => setScope('all')}
  >
    {t('agentGuard.scopeAll')}
  </button>
  <button
    role="radio"
    aria-checked={config.write_scope === 'builder'}
    class:on={config.write_scope === 'builder'}
    onclick={() => setScope('builder')}
  >
    {t('agentGuard.scopeBuilder')}
  </button>
</div>
<p class="hint dim">
  {config.write_scope === 'builder' ? t('agentGuard.scopeBuilderDesc') : t('agentGuard.scopeAllDesc')}
</p>
{#if config.write_scope === 'builder'}
  <div class="lab">{t('agentGuard.allowedServers')}</div>
  {#if servers.length === 0}
    <p class="hint dim">{t('agentGuard.noServers')}</p>
  {:else}
    <div class="servers">
      {#each servers as s (s.id)}
        <label class="srv-row">
          <input
            type="checkbox"
            checked={config.allowed_servers.includes(s.id)}
            onchange={() => toggleAllowed(s.id)}
          />
          <span>{s.name}</span>
        </label>
      {/each}
    </div>
  {/if}
{/if}
<p class="hint dim">{t('agentGuard.envNote')}</p>

<div class="head">
  <h2>{t('agentGuard.journalTitle')}</h2>
  <button class="mini" onclick={refresh} disabled={loading} aria-label={t('agentGuard.refresh')}>
    {#if loading}<span class="spinner"></span>{:else}<Icon name="refresh-cw" size={13} />{/if}
    {t('agentGuard.refresh')}
  </button>
</div>
<p class="desc dim">{t('agentGuard.journalDesc')}</p>
<div class="filters">
  <select class="sel" bind:value={serverFilter} onchange={refresh}>
    <option value={null}>{t('agentGuard.allServers')}</option>
    {#each servers as s (s.id)}
      <option value={s.id}>{s.name}</option>
    {/each}
  </select>
  <label class="check">
    <input type="checkbox" bind:checked={onlyWrites} onchange={refresh} />
    <span>{t('agentGuard.onlyWrites')}</span>
  </label>
</div>
{#if error}
  <p class="msg">{error}</p>
{/if}
{#if actions.length === 0}
  <p class="hint dim">{t('agentGuard.emptyJournal')}</p>
{:else}
  <div class="log">
    {#each actions as a (a.id)}
      <div class="entry">
        <div class="e-top">
          <span class="e-time dim">{fmtDateTime(a.ts)}</span>
          <span class="e-tool">{a.tool}</span>
          {#if a.write}<span class="tag">{t('agentGuard.write')}</span>{/if}
          {#if a.servers.length > 0}
            <span class="e-servers">{a.servers.map(serverName).join(', ')}</span>
          {/if}
          <span class="spacer"></span>
          <span class="outcome {a.outcome}">{t(OUTCOME[a.outcome])}</span>
          <span class="e-dur dim">{a.duration_ms} ms</span>
        </div>
        <div class="e-args" title={argsText(a)}>{argsText(a)}</div>
        {#if a.detail}<div class="e-detail">{a.detail}</div>{/if}
        <div class="e-meta dim">{a.client ? `${a.client} · ` : ''}{a.session}</div>
      </div>
    {/each}
  </div>
{/if}

<h2>{t('agentGuard.changesetsTitle')}</h2>
<p class="desc dim">{t('agentGuard.changesetsDesc')}</p>
{#if changesets.length === 0}
  <p class="hint dim">{t('agentGuard.emptyChangesets')}</p>
{/if}
{#each changesets as cs (cs.id)}
  {@const c = counts(cs)}
  <div class="cs">
    <div class="cs-top">
      <div class="cs-main">
        <div class="cs-title">
          <span class="tag">
            {cs.origin === 'copilot'
              ? t('agentGuard.originCopilot')
              : cs.origin === 'app'
                ? t('agentGuard.originApp')
                : t('agentGuard.originMcp')}
          </span>
          {#if cs.client}<span class="cs-client">{cs.client}</span>{/if}
          <span class="cs-id dim" title={cs.id}>{cs.id}</span>
        </div>
        <div class="cs-sub dim">
          {fmtDateTime(cs.updated)} · {cs.servers.map(serverName).join(', ')} ·
          {t('agentGuard.stepsCount', { n: cs.steps.length })} ·
          {t('agentGuard.activeCount', { n: c.active })} ·
          {t('agentGuard.undoableCount', { n: c.undoable })}
        </div>
      </div>
      <div class="cs-acts">
        <button class="mini" onclick={() => (open[cs.id] = !open[cs.id])}>
          {open[cs.id] ? t('agentGuard.hideSteps') : t('agentGuard.showSteps')}
        </button>
        <button
          class="mini danger"
          disabled={busy !== null || c.undoable === 0}
          onclick={() => undo(cs, false)}
        >
          {#if busy === cs.id}
            <span class="spinner"></span>
            {t('agentGuard.undoing')}
          {:else}
            {t('agentGuard.undo')}
          {/if}
        </button>
      </div>
    </div>

    {#if reports[cs.id]}
      {@const r = reports[cs.id]}
      <div class="report">
        {#if r.steps.length === 0}
          <div>{t('agentGuard.nothingToUndo')}</div>
        {:else}
          {#each r.steps as s (s.seq)}
            <div class="r-row {s.status}">
              <span class="r-seq">#{s.seq}</span>
              <span class="r-label">{s.tool}{s.target ? ` ${s.target}` : ''}</span>
              <span class="r-status">{t(REPORT[s.status])}</span>
              <span class="r-msg dim">{s.message}</span>
            </div>
          {/each}
          <div class="r-end">{r.stopped ? t('agentGuard.reportStopped') : t('agentGuard.reportDone')}</div>
        {/if}
        {#if refused(cs)}
          <button class="mini danger" disabled={busy !== null} onclick={() => undo(cs, true)}>
            <Icon name="triangle-alert" size={13} />
            {t('agentGuard.forceUndo')}
          </button>
        {/if}
      </div>
    {/if}

    {#if open[cs.id]}
      <div class="steps">
        {#each cs.steps as s (s.seq)}
          <div class="step" class:done={s.state !== 'active'}>
            <span class="r-seq">#{s.seq}</span>
            <span class="e-time dim">{fmtDateTime(s.ts)}</span>
            <span class="r-label">{s.tool}{s.target ? ` ${s.target}` : ''}</span>
            {#if s.server_id !== null}<span class="e-servers">{serverName(s.server_id)}</span>{/if}
            <span class="spacer"></span>
            {#if s.state === 'active' && !s.undoable}
              <span class="outcome denied">{t('agentGuard.notUndoable')}</span>
            {:else}
              <span class="outcome {s.state === 'active' ? 'ok' : 'muted'}">{t(STATE[s.state])}</span>
            {/if}
            {#if s.failed}<span class="outcome error">{t('agentGuard.failedStep')}</span>{/if}
            {#if s.note}<div class="s-note dim">{s.note}</div>{/if}
          </div>
        {/each}
      </div>
    {/if}
  </div>
{/each}

<style>
  h2 {
    margin: 26px 0 12px;
    font-size: 16px;
    color: var(--text);
    font-weight: 900;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  .desc {
    font-size: 13px;
    margin: 0 0 10px;
    line-height: 1.55;
  }
  .hint {
    font-size: 12.5px;
    margin: 8px 0 10px;
    line-height: 1.5;
  }
  .lab {
    font-size: 12.5px;
    font-weight: 800;
    color: var(--text-muted);
    margin: 4px 0 8px;
  }
  .seg {
    display: inline-flex;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
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
    font-weight: 800;
  }
  .servers {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
    max-height: 240px;
    overflow-y: auto;
  }
  .srv-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 9px 12px;
    font-size: 13px;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 55%, transparent);
    cursor: pointer;
  }
  .srv-row:last-child {
    border-bottom: none;
  }
  .filters {
    display: flex;
    align-items: center;
    gap: 14px;
    flex-wrap: wrap;
    margin-bottom: 10px;
  }
  .sel {
    font: inherit;
    font-size: 12.5px;
    color: var(--text);
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 6px 10px;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    cursor: pointer;
  }
  .log {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    max-height: 420px;
    overflow-y: auto;
  }
  .entry {
    padding: 8px 12px;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 55%, transparent);
    min-width: 0;
  }
  .entry:last-child {
    border-bottom: none;
  }
  .e-top,
  .step {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    font-size: 12.5px;
  }
  .e-time {
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
  }
  .e-tool,
  .r-label {
    font-family: var(--font-mono);
    font-size: 12px;
    font-weight: 700;
    overflow-wrap: anywhere;
  }
  .e-servers {
    font-size: 12px;
    color: var(--text-muted);
  }
  .e-dur {
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
  }
  .e-args {
    font-family: var(--font-mono);
    font-size: 11.5px;
    color: var(--text-muted);
    margin-top: 3px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .e-detail {
    font-size: 12px;
    color: var(--state-danger);
    margin-top: 3px;
    overflow-wrap: anywhere;
  }
  .e-meta {
    font-size: 11px;
    margin-top: 2px;
    overflow-wrap: anywhere;
  }
  .tag {
    font-size: 10.5px;
    font-weight: 800;
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 1px 6px;
  }
  .outcome {
    font-size: 11px;
    font-weight: 800;
    border-radius: 5px;
    padding: 1px 6px;
  }
  .outcome.ok {
    color: var(--state-running);
    background: color-mix(in srgb, var(--state-running) 14%, transparent);
  }
  .outcome.error {
    color: var(--state-danger);
    background: color-mix(in srgb, var(--state-danger) 14%, transparent);
  }
  .outcome.denied {
    color: var(--state-pending);
    background: color-mix(in srgb, var(--state-pending) 14%, transparent);
  }
  .outcome.muted {
    color: var(--text-dim);
    border: 1px solid var(--border);
  }
  .spacer {
    flex: 1;
  }
  .cs {
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--surface);
    padding: 10px 12px;
    margin-bottom: 10px;
  }
  .cs-top {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .cs-main {
    flex: 1;
    min-width: 0;
  }
  .cs-title {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .cs-client {
    font-weight: 800;
    font-size: 13.5px;
  }
  .cs-id {
    font-family: var(--font-mono);
    font-size: 11.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
  }
  .cs-sub {
    font-size: 12px;
    margin-top: 3px;
  }
  .cs-acts {
    flex: none;
    display: flex;
    gap: 6px;
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
    opacity: 0.5;
    cursor: default;
  }
  .mini.danger:hover:not(:disabled) {
    color: var(--state-danger);
  }
  .steps,
  .report {
    margin-top: 10px;
    border-top: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
    padding-top: 8px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .step.done .r-label {
    text-decoration: line-through;
    color: var(--text-dim);
  }
  .s-note {
    flex-basis: 100%;
    font-size: 11.5px;
    padding-left: 28px;
  }
  .r-seq {
    font-size: 11.5px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
    min-width: 22px;
  }
  .r-row {
    display: flex;
    align-items: baseline;
    gap: 8px;
    flex-wrap: wrap;
    font-size: 12.5px;
  }
  .r-status {
    font-weight: 800;
    font-size: 12px;
  }
  .r-row.undone .r-status {
    color: var(--state-running);
  }
  .r-row.refused .r-status,
  .r-row.failed .r-status {
    color: var(--state-danger);
  }
  .r-row.skipped .r-status {
    color: var(--text-dim);
  }
  .r-msg {
    font-size: 12px;
    overflow-wrap: anywhere;
  }
  .r-end {
    font-size: 12.5px;
    font-weight: 700;
  }
  .report .mini {
    align-self: flex-start;
  }
  .msg {
    font-size: 13px;
    margin: 6px 0 10px;
    color: var(--state-danger);
  }
  .spinner {
    width: 11px;
    height: 11px;
    border: 2px solid currentColor;
    border-right-color: transparent;
    border-radius: 50%;
    display: inline-block;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
