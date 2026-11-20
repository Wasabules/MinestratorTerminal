<script lang="ts">
  /**
   * Assistant « Préréglages » d'un réseau : LuckPerms (même base MySQL/MariaDB pour le proxy et
   * chaque serveur de jeu) ou Geyser (Geyser et Floodgate, clé de Floodgate copiée du proxy sans
   * être lue, port Bedrock). Même déroulé que la liaison : plan simulé, confirmation (et accord
   * explicite pour les effets qui ne se défont pas), application. Règles : `network/preset.rs`.
   */
  import { untrack } from 'svelte';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';
  import type {
    NetworkDatabase,
    NetworkPresetKind,
    NetworkPresetPlan,
    NetworkPresetRequest,
    NetworkTopology,
  } from '#lib/types.ts';
  import Modal from '#lib/mybox/Modal.svelte';
  import Icon from '#lib/components/Icon.svelte';
  import Masked from '#lib/components/Masked.svelte';
  import PlanSteps from './PlanSteps.svelte';
  import PlanOutcome from './PlanOutcome.svelte';
  import { backendCandidates, changedServers, linkedBackends, myboxOf, platformLabel } from './util.ts';

  let {
    topology,
    proxyId,
    onclose,
  }: {
    topology: NetworkTopology;
    proxyId: number | null;
    onclose: (changed: boolean) => void;
  } = $props();

  type Stage = 'form' | 'plan' | 'confirm' | 'result';
  const KINDS: NetworkPresetKind[] = ['luckperms', 'geyser'];
  let stage = $state<Stage>('form');
  let preset = $state<NetworkPresetKind>('luckperms');
  let proxy = $state<number | null>(
    untrack(() => proxyId ?? topology.proxies[0]?.server_id ?? null),
  );
  let chosen = $state<number[]>([]);
  let databases = $state<NetworkDatabase[] | null>(null);
  let dbError = $state<string | null>(null);
  let database = $state<string>('');
  let bedrockPort = $state<string>('');
  let snapshot = $state(true);
  let acknowledged = $state(false);
  let showOthers = $state(false);
  let plan = $state<NetworkPresetPlan | null>(null);
  let result = $state<NetworkPresetPlan | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let wrote = false;

  const candidates = $derived(proxy == null ? [] : backendCandidates(topology, proxy));
  const proxyBox = $derived(proxy == null ? null : myboxOf(topology, proxy));
  const visible = $derived(
    showOthers ? candidates : candidates.filter((s) => s.mybox_id === proxyBox || chosen.includes(s.id)),
  );
  const hiddenCount = $derived(candidates.length - visible.length);
  const portValid = $derived(
    bedrockPort.trim() === '' || (/^\d{1,5}$/.test(bedrockPort.trim()) && Number(bedrockPort) >= 1 && Number(bedrockPort) <= 65535),
  );
  const canPlan = $derived(proxy != null && chosen.length > 0 && portValid && !busy);

  function resetFor(id: number | null) {
    const p = topology.proxies.find((x) => x.server_id === id);
    chosen = p ? linkedBackends(topology, p) : [];
    database = '';
  }
  untrack(() => resetFor(proxy));

  /** MyBox dont les bases sont chargées (une lecture par MyBox, LuckPerms seulement). */
  let dbFor = $state<number | null>(null);
  $effect(() => {
    const box = proxy == null ? null : myboxOf(topology, proxy);
    if (preset !== 'luckperms' || box == null || box === dbFor) return;
    dbFor = box;
    untrack(() => void loadDatabases(box));
  });

  /** Bases de la MyBox du proxy (LuckPerms) : liste sans mot de passe. */
  async function loadDatabases(box: number) {
    databases = null;
    dbError = null;
    try {
      const list = await api.networkDatabases(box);
      if (dbFor === box) databases = list;
    } catch (e) {
      if (dbFor === box) dbError = humanizeError(e);
    }
  }

  function chooseProxy(v: string) {
    proxy = v ? Number(v) : null;
    resetFor(proxy);
  }

  function toggle(id: number, on: boolean) {
    chosen = on ? (chosen.includes(id) ? chosen : [...chosen, id]) : chosen.filter((x) => x !== id);
  }

  function request(): NetworkPresetRequest {
    const lp = preset === 'luckperms';
    return {
      preset,
      proxy_server_id: proxy!,
      backend_server_ids: chosen,
      mybox_id: lp && database ? myboxOf(topology, proxy!) : null,
      database_id: lp && database ? Number(database) : null,
      bedrock_port: !lp && bedrockPort.trim() ? Number(bedrockPort) : null,
      snapshot,
    };
  }

  async function simulate() {
    if (!canPlan) return;
    busy = true;
    error = null;
    try {
      plan = await api.networkPreset(request(), null);
      acknowledged = false;
      stage = 'plan';
    } catch (e) {
      error = humanizeError(e);
    } finally {
      busy = false;
    }
  }

  async function apply() {
    if (!plan) return;
    busy = true;
    error = null;
    try {
      const r = await api.networkPreset(request(), plan.fingerprint, acknowledged);
      if (r.status === 'changed' || r.status === 'needs_confirmation') {
        plan = r;
        result = r;
        acknowledged = false;
        stage = 'plan';
      } else {
        wrote = wrote || r.applied > 0;
        plan = r;
        result = r;
        stage = 'result';
      }
    } catch (e) {
      error = humanizeError(e);
      stage = 'plan';
    } finally {
      busy = false;
    }
  }

  function close() {
    if (busy) return;
    onclose(wrote);
  }
</script>

<Modal title={t('network.preset.title')} onclose={close} width={820}>
  <ol class="stages" aria-label={t('network.wizard.stages')}>
    {#each ['form', 'plan', 'confirm', 'result'] as s, i (s)}
      <li class:on={stage === s} aria-current={stage === s ? 'step' : undefined}>
        {i + 1}. {t(`network.wizard.stage.${s}`)}
      </li>
    {/each}
  </ol>

  {#if error}<p class="alert" role="alert">{error}</p>{/if}

  {#if stage === 'form'}
    {#if topology.proxies.length === 0}
      <p class="dim">{t('network.wizard.noProxy')}</p>
    {:else}
      <fieldset class="field">
        <legend class="label">{t('network.preset.kind')}</legend>
        <div class="kinds">
          {#each KINDS as k (k)}
            <label class="kind" class:on={preset === k}>
              <input type="radio" name="preset-kind" value={k} bind:group={preset} />
              <span class="kname">{t(`network.preset.${k}.name`)}</span>
              <span class="dim small">{t(`network.preset.${k}.desc`)}</span>
            </label>
          {/each}
        </div>
      </fieldset>

      <label class="field">
        <span class="label">{t('network.wizard.proxy')}</span>
        <select class="input sans" value={proxy == null ? '' : String(proxy)} onchange={(e) => chooseProxy(e.currentTarget.value)}>
          {#each topology.proxies as p (p.server_id)}
            <option value={String(p.server_id)}>{p.name} · {t(`network.kind.${p.kind}`)}</option>
          {/each}
        </select>
      </label>

      <fieldset class="field">
        <legend class="label">{t('network.wizard.backends')}</legend>
        <p class="hint dim">{t('network.preset.backendsHint')}</p>
        {#if candidates.length === 0}
          <p class="dim">{t('network.wizard.noBackend')}</p>
        {:else}
          <ul class="pick">
            {#each visible as s (s.id)}
              <li>
                <label class="check">
                  <input type="checkbox" checked={chosen.includes(s.id)} onchange={(e) => toggle(s.id, e.currentTarget.checked)} />
                  <span class="sname">{s.name}</span>
                  <span class="dim small">{platformLabel(s.platform)}{s.version ? ` ${s.version}` : ''}</span>
                  {#if !s.readable}<span class="badge warn">{t(`status.${s.status}`)}</span>{/if}
                </label>
              </li>
            {/each}
          </ul>
          {#if hiddenCount > 0 || showOthers}
            <button class="linkish" onclick={() => (showOthers = !showOthers)}>
              {showOthers ? t('network.wizard.hideOthers') : t('network.wizard.showOthers', { n: hiddenCount })}
            </button>
          {/if}
        {/if}
      </fieldset>

      {#if preset === 'luckperms'}
        <label class="field">
          <span class="label">{t('network.preset.database')}</span>
          <select class="input sans" bind:value={database}>
            <option value="">{t('network.preset.databaseAuto')}</option>
            {#each databases ?? [] as d (d.id)}
              <option value={String(d.id)} disabled={!d.supported}>
                {d.label ? `${d.label} · ` : ''}{d.database} ({d.username})
              </option>
            {/each}
          </select>
          {#if dbError}<span class="err small">{dbError}</span>
          {:else if databases && databases.length === 0}<span class="dim small">{t('network.preset.noDatabase')}</span>{/if}
          <span class="hint dim">{t('network.preset.databaseHint')}</span>
        </label>
      {:else}
        <label class="field">
          <span class="label">{t('network.preset.bedrockPort')}</span>
          <input class="input small-input" inputmode="numeric" placeholder={t('network.preset.bedrockAuto')} bind:value={bedrockPort} aria-invalid={!portValid} class:invalid={!portValid} />
          <span class="hint dim">{t('network.preset.bedrockHint')}</span>
        </label>
      {/if}

      <label class="check">
        <input type="checkbox" bind:checked={snapshot} />
        <span>{t('network.wizard.snapshot')}</span>
      </label>
    {/if}

    <div class="actions">
      <span class="spacer"></span>
      <button class="btn btn--ghost" onclick={close}>{t('common.cancel')}</button>
      <button class="btn" disabled={!canPlan} onclick={simulate}>
        {#if busy}<span class="spinner" aria-hidden="true"></span>{/if}
        {busy ? t('network.wizard.simulating') : t('network.wizard.simulate')}
      </button>
    </div>
  {:else if stage === 'plan' && plan}
    {#if result?.status === 'changed'}
      <p class="alert warn-box" role="alert">{t('network.result.changed')}</p>
    {:else if result?.status === 'needs_confirmation'}
      <p class="alert warn-box" role="alert">{t('network.result.needs_confirmation')}</p>
    {/if}
    <p class="dim">{t('network.plan.intro')}</p>
    <div class="counts">
      <span class="badge">{t(`network.preset.${plan.preset}.name`)} · {plan.proxy_name}</span>
      <span class="badge">{t('network.plan.counts.change', { n: plan.counts.change })}</span>
      <span class="badge">{t('network.plan.counts.ok', { n: plan.counts.ok })}</span>
      {#if plan.counts.manual}<span class="badge warn">{t('network.plan.counts.manual', { n: plan.counts.manual })}</span>{/if}
      {#if plan.counts.refused}<span class="badge danger">{t('network.plan.counts.refused', { n: plan.counts.refused })}</span>{/if}
    </div>

    <h3>{t('network.preset.servers')}</h3>
    <ul class="plain">
      {#each plan.servers as s (s.server_id)}
        <li>
          <strong>{s.name}</strong>
          <span class="dim">· {s.proxy ? t('network.preset.roleProxy') : t('network.preset.roleBackend')} · {platformLabel(s.platform)}</span>
          {#if s.entry && !s.proxy}<span class="dim mono">· {s.entry}</span>{/if}
        </li>
      {/each}
    </ul>

    {#if plan.database}
      {@const d = plan.database}
      <h3>{t('network.preset.database')}</h3>
      <p class="line">
        <span class="mono"><Masked value={`${d.host}:${d.port}`} kind="address" /></span>
        <span class="mono">{d.database}</span>
        <span class="dim">({d.username})</span>
        {#if d.password_readable}
          <span class="badge ok"><Icon name="key" size={11} /> {t('network.preset.passwordRead')}</span>
        {:else}
          <span class="badge danger">{t('network.preset.passwordUnreadable')}</span>
        {/if}
      </p>
    {/if}
    {#if plan.bedrock}
      {@const b = plan.bedrock}
      <h3>{t('network.preset.bedrockPort')}</h3>
      {#if b.port != null}
        <p class="line"><span class="mono">UDP {b.port}</span> <span class="dim"><Masked value={b.source ?? ''} /></span></p>
      {:else}
        <p class="line warn-text">{t('network.preset.noPort')}</p>
        {#if b.manage_ports}<pre class="call"><Masked value={b.manage_ports} /></pre>{/if}
      {/if}
      {#if b.myshield}<p class="dim small">{t('network.preset.myshield')}</p>{/if}
    {/if}

    {#if plan.warnings.length > 0}
      <h3>{t('network.plan.warnings')}</h3>
      <ul class="plain warn-list">
        {#each plan.warnings as w, i (i)}<li><Masked value={w} /></li>{/each}
      </ul>
    {/if}
    {#if plan.to_confirm.length > 0}
      <div class="to-confirm">
        <h3>{t('network.preset.toConfirm')}</h3>
        <ul class="plain">
          {#each plan.to_confirm as w, i (i)}<li><Masked value={w} /></li>{/each}
        </ul>
        <label class="check">
          <input type="checkbox" bind:checked={acknowledged} />
          <span>{t('network.preset.acknowledge')}</span>
        </label>
      </div>
    {/if}
    <h3>{t('network.plan.steps')}</h3>
    <PlanSteps steps={plan.steps} />

    <div class="actions">
      <button class="btn btn--ghost" onclick={() => ((stage = 'form'), (result = null))}>{t('network.wizard.back')}</button>
      <span class="spacer"></span>
      {#if plan.counts.refused > 0}
        <span class="dim small">{t('network.plan.refusedHint')}</span>
      {:else if plan.counts.change === 0}
        <span class="dim small">{t('network.preset.nothingHint')}</span>
      {/if}
      <button
        class="btn"
        disabled={plan.counts.change === 0 || plan.counts.refused > 0 || (plan.to_confirm.length > 0 && !acknowledged)}
        onclick={() => (stage = 'confirm')}
      >
        {t('network.wizard.applyEllipsis')}
      </button>
    </div>
  {:else if stage === 'confirm' && plan}
    <div class="confirm">
      <p>{t('network.confirm.body', { n: plan.counts.change, servers: changedServers(plan.steps) })}</p>
      <ul class="plain">
        <li>{snapshot ? t('network.confirm.snapshotOn') : t('network.confirm.snapshotOff')}</li>
        <li>{t('network.confirm.undo')}</li>
        <li>{t('network.preset.installNote')}</li>
        <li>{t('network.confirm.restart')}</li>
      </ul>
    </div>
    <div class="actions">
      <button class="btn btn--ghost" disabled={busy} onclick={() => (stage = 'plan')}>{t('network.wizard.back')}</button>
      <span class="spacer"></span>
      <button class="btn" disabled={busy} onclick={apply}>
        {#if busy}<span class="spinner" aria-hidden="true"></span>{/if}
        {busy ? t('network.confirm.applying') : t('network.confirm.apply')}
      </button>
    </div>
    {#if busy}<p class="dim small">{t('network.confirm.wait')}</p>{/if}
  {:else if stage === 'result' && result}
    <PlanOutcome
      status={result.status}
      applied={result.applied}
      snapshots={result.snapshots}
      restart={result.counts.manual > 0
        ? t('network.result.restartManual')
        : result.preset === 'luckperms'
          ? t('network.result.restartLuckperms')
          : t('network.result.restartGeyser')}
    />
    <PlanSteps steps={result.steps} />
    <div class="actions">
      <span class="spacer"></span>
      <button class="btn" onclick={close}>{t('common.close')}</button>
    </div>
  {/if}
</Modal>

<style>
  .stages {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    list-style: none;
    margin: 0;
    padding: 0;
    font-size: 12px;
    font-weight: 800;
    color: var(--text-dim);
  }
  .stages li {
    padding: 3px 10px;
    border-radius: 999px;
    border: 1px solid var(--border);
  }
  .stages li.on {
    color: var(--on-accent);
    background: var(--accent);
    border-color: transparent;
  }
  fieldset {
    border: none;
    margin: 0;
    padding: 0;
  }
  legend {
    padding: 0;
    margin-bottom: 6px;
  }
  .sans {
    font-family: var(--font-sans);
  }
  .hint {
    margin: 0;
    font-size: 12.5px;
  }
  .kinds {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
    gap: 8px;
  }
  .kind {
    display: grid;
    grid-template-columns: auto 1fr;
    column-gap: 8px;
    row-gap: 2px;
    padding: 10px 12px;
    border-radius: var(--radius);
    border: 1px solid var(--border);
    background: var(--inset);
    cursor: pointer;
  }
  .kind input {
    grid-row: span 2;
    align-self: start;
    margin-top: 3px;
  }
  .kind.on {
    border-color: var(--accent);
  }
  .kname {
    font-weight: 900;
  }
  .pick {
    list-style: none;
    margin: 0;
    padding: 6px 8px;
    max-height: 220px;
    overflow-y: auto;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--inset);
  }
  .check {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 0;
    font-size: 13.5px;
    cursor: pointer;
  }
  .sname {
    font-weight: 800;
  }
  .small {
    font-size: 12px;
  }
  .small-input {
    padding: 6px 9px;
    font-size: 13px;
    max-width: 220px;
  }
  .invalid {
    border-color: var(--state-danger);
  }
  .err {
    color: var(--state-danger);
  }
  .linkish {
    border: none;
    background: none;
    padding: 4px 0 0;
    font: inherit;
    font-size: 12.5px;
    font-weight: 800;
    color: var(--brand-primary);
    cursor: pointer;
    align-self: flex-start;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .spacer {
    flex: 1;
  }
  h3 {
    margin: 4px 0 0;
    font-size: 12.5px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-muted);
  }
  .counts {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .badge.ok {
    color: var(--state-running-text);
  }
  .badge.warn {
    color: var(--state-pending);
  }
  .badge.danger {
    color: var(--state-danger);
  }
  .line {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
    margin: 0;
    font-size: 13px;
  }
  .warn-text {
    color: var(--state-pending);
  }
  .call {
    margin: 0;
    padding: 8px 10px;
    background: var(--chrome);
    border-radius: var(--radius-sm);
    font-family: var(--font-mono);
    font-size: 11.5px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .plain {
    margin: 0;
    padding-left: 18px;
    font-size: 13px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .warn-list li {
    color: var(--state-pending);
  }
  .to-confirm {
    border: 1px solid color-mix(in srgb, var(--state-pending) 45%, var(--border));
    border-radius: var(--radius);
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .warn-box {
    background: color-mix(in srgb, var(--state-pending) 14%, var(--inset));
    color: var(--text);
  }
  .confirm p {
    margin: 0 0 8px;
    font-size: 14px;
    font-weight: 800;
  }
</style>
