<script lang="ts">
  /**
   * Assistant « Relier des serveurs » : proxy, serveurs de jeu (ordre = repli par défaut), noms
   * d'entrée, mode de transfert, routage par domaine, nettoyage des entrées d'exemple. Le plan est
   * d'abord simulé (diffs, étapes manuelles, avertissements), puis appliqué après confirmation —
   * le cœur refuse d'écrire si le plan a changé depuis (empreinte). Règles : `network/link.rs`.
   */
  import { untrack } from 'svelte';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';
  import type { LinkRequest, NetworkLinkPlan, NetworkTopology } from '#lib/types.ts';
  import Modal from '#lib/mybox/Modal.svelte';
  import Icon from '#lib/components/Icon.svelte';
  import Masked from '#lib/components/Masked.svelte';
  import PlanSteps from './PlanSteps.svelte';
  import PlanOutcome from './PlanOutcome.svelte';
  import {
    backendCandidates,
    changedServers,
    fallbackBackends,
    linkedBackends,
    myboxOf,
    platformLabel,
    validEntryName,
  } from './util.ts';

  let {
    topology,
    proxyId,
    onclose,
  }: {
    topology: NetworkTopology;
    proxyId: number | null;
    /** `changed` : quelque chose a été écrit (la vue relit la topologie). */
    onclose: (changed: boolean) => void;
  } = $props();

  type Stage = 'form' | 'plan' | 'confirm' | 'result';
  let stage = $state<Stage>('form');
  let proxy = $state<number | null>(
    untrack(() => proxyId ?? topology.proxies[0]?.server_id ?? null),
  );
  /** Serveurs de jeu choisis, dans l'ordre (repli par défaut). */
  let order = $state<number[]>([]);
  let names = $state<Record<number, string>>({});
  let fallback = $state<Record<number, boolean>>({});
  let mode = $state<'auto' | 'modern' | 'legacy'>('auto');
  let hosts = $state<{ host: string; target: number | null }[]>([]);
  let prune = $state(false);
  let snapshot = $state(true);
  let showOthers = $state(false);
  let plan = $state<NetworkLinkPlan | null>(null);
  /** Plan rendu après une application (résultat, ou nouveau plan si le réseau a changé). */
  let result = $state<NetworkLinkPlan | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let wrote = false;

  const proxyInfo = $derived(topology.proxies.find((p) => p.server_id === proxy) ?? null);
  const candidates = $derived(proxy == null ? [] : backendCandidates(topology, proxy));
  const proxyBox = $derived(proxy == null ? null : myboxOf(topology, proxy));
  const visible = $derived(
    showOthers ? candidates : candidates.filter((s) => s.mybox_id === proxyBox || order.includes(s.id)),
  );
  const hiddenCount = $derived(candidates.length - visible.length);
  const byId = $derived(new Map(topology.servers.map((s) => [s.id, s])));
  const badNames = $derived(order.filter((id) => !validEntryName(names[id] ?? '')));
  const canPlan = $derived(proxy != null && order.length > 0 && badNames.length === 0 && !busy);

  /** Valeurs par défaut pour un proxy : les serveurs qu'il relie déjà, sa liste de repli. */
  function resetFor(id: number | null) {
    const p = topology.proxies.find((x) => x.server_id === id);
    order = p ? linkedBackends(topology, p) : [];
    const fb = p ? fallbackBackends(p) : [];
    fallback = Object.fromEntries(order.filter((x) => fb.includes(x)).map((x) => [x, true]));
    names = {};
    hosts = [];
    mode = 'auto';
    prune = false;
  }
  untrack(() => resetFor(proxy));

  function chooseProxy(v: string) {
    proxy = v ? Number(v) : null;
    resetFor(proxy);
  }

  function toggle(id: number, on: boolean) {
    if (on && !order.includes(id)) order = [...order, id];
    if (!on) {
      order = order.filter((x) => x !== id);
      hosts = hosts.map((h) => (h.target === id ? { ...h, target: null } : h));
    }
  }

  function move(id: number, delta: number) {
    const i = order.indexOf(id);
    const j = i + delta;
    if (i < 0 || j < 0 || j >= order.length) return;
    const next = [...order];
    [next[i], next[j]] = [next[j], next[i]];
    order = next;
  }

  function request(): LinkRequest {
    return {
      proxy_server_id: proxy!,
      backend_server_ids: order,
      names: order
        .filter((id) => (names[id] ?? '').trim() !== '')
        .map((id) => ({ server_id: id, name: names[id].trim() })),
      try_order: order.filter((id) => fallback[id]),
      forced_hosts: hosts
        .filter((h) => h.host.trim() !== '' && h.target != null)
        .map((h) => ({ host: h.host.trim(), targets: [h.target!] })),
      prune,
      mode: mode === 'auto' ? null : mode,
      snapshot,
    };
  }

  async function simulate() {
    if (!canPlan) return;
    busy = true;
    error = null;
    try {
      plan = await api.networkLink(request(), null);
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
      const r = await api.networkLink(request(), plan.fingerprint);
      if (r.status === 'changed') {
        // Le réseau a bougé depuis la simulation : rien n'a été écrit, nouveau plan à revoir.
        plan = r;
        result = r;
        stage = 'plan';
      } else {
        wrote = wrote || r.applied > 0;
        result = r;
        plan = r;
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

  const ENTRY_CHANGE: Record<string, string> = { added: 'ok', updated: 'warn', unchanged: '' };
</script>

<Modal title={t('network.wizard.linkTitle')} onclose={close} width={820}>
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
        <p class="hint dim">{t('network.wizard.backendsHint')}</p>
        {#if candidates.length === 0}
          <p class="dim">{t('network.wizard.noBackend')}</p>
        {:else}
          <ul class="pick">
            {#each visible as s (s.id)}
              <li>
                <label class="check">
                  <input type="checkbox" checked={order.includes(s.id)} onchange={(e) => toggle(s.id, e.currentTarget.checked)} />
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

      {#if order.length > 0}
        <fieldset class="field">
          <legend class="label">{t('network.wizard.entries')}</legend>
          <p class="hint dim">{t('network.wizard.entriesHint')}</p>
          <table class="entries">
            <thead>
              <tr>
                <th scope="col">{t('network.wizard.col.order')}</th>
                <th scope="col">{t('network.wizard.col.server')}</th>
                <th scope="col">{t('network.wizard.col.name')}</th>
                <th scope="col">{t('network.wizard.col.fallback')}</th>
              </tr>
            </thead>
            <tbody>
              {#each order as id, i (id)}
                {@const s = byId.get(id)}
                <tr>
                  <td class="ord">
                    <button class="icon-btn" disabled={i === 0} aria-label={t('network.wizard.moveUp', { name: s?.name ?? id })} onclick={() => move(id, -1)}>
                      <Icon name="arrow-up" size={12} />
                    </button>
                    <button class="icon-btn" disabled={i === order.length - 1} aria-label={t('network.wizard.moveDown', { name: s?.name ?? id })} onclick={() => move(id, 1)}>
                      <Icon name="arrow-down" size={12} />
                    </button>
                  </td>
                  <td>{s?.name ?? id}</td>
                  <td>
                    <input
                      class="input small-input"
                      class:invalid={!validEntryName(names[id] ?? '')}
                      maxlength="32"
                      placeholder={t('network.wizard.autoName')}
                      aria-label={t('network.wizard.nameFor', { name: s?.name ?? id })}
                      aria-invalid={!validEntryName(names[id] ?? '')}
                      bind:value={names[id]}
                    />
                  </td>
                  <td class="center">
                    <input
                      type="checkbox"
                      aria-label={t('network.wizard.fallbackFor', { name: s?.name ?? id })}
                      checked={!!fallback[id]}
                      onchange={(e) => (fallback[id] = e.currentTarget.checked)}
                    />
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
          {#if badNames.length > 0}<p class="err small">{t('network.wizard.badName')}</p>{/if}
          <p class="hint dim">{t('network.wizard.fallbackHint')}</p>
        </fieldset>

        <fieldset class="field">
          <legend class="label">{t('network.wizard.mode')}</legend>
          <div class="radios">
            <label class="check"><input type="radio" name="link-mode" value="auto" bind:group={mode} /> {t('network.wizard.modeAuto')}</label>
            <label class="check">
              <input type="radio" name="link-mode" value="modern" bind:group={mode} disabled={proxyInfo?.kind === 'bungeecord'} />
              {t('network.wizard.modeModern')}
            </label>
            <label class="check"><input type="radio" name="link-mode" value="legacy" bind:group={mode} /> {t('network.wizard.modeLegacy')}</label>
          </div>
          <p class="hint dim">{proxyInfo?.kind === 'bungeecord' ? t('network.wizard.modeHintBungee') : t('network.wizard.modeHint')}</p>
        </fieldset>

        <fieldset class="field">
          <legend class="label">{t('network.wizard.hosts')}</legend>
          <p class="hint dim">{t('network.wizard.hostsHint')}</p>
          {#each hosts as h, i (i)}
            <div class="host-row">
              <input class="input small-input" placeholder="play.exemple.fr" aria-label={t('network.wizard.hostDomain')} bind:value={h.host} />
              <span aria-hidden="true">→</span>
              <select class="input sans small-input" aria-label={t('network.wizard.hostTarget')} value={h.target == null ? '' : String(h.target)} onchange={(e) => (h.target = e.currentTarget.value ? Number(e.currentTarget.value) : null)}>
                <option value="">{t('network.wizard.hostPick')}</option>
                {#each order as id (id)}
                  <option value={String(id)}>{byId.get(id)?.name ?? id}</option>
                {/each}
              </select>
              <button class="icon-btn" aria-label={t('network.wizard.hostRemove')} onclick={() => (hosts = hosts.filter((_, j) => j !== i))}>
                <Icon name="x" size={14} />
              </button>
            </div>
          {/each}
          <button class="btn btn--ghost small-btn" onclick={() => (hosts = [...hosts, { host: '', target: order[0] ?? null }])}>
            <Icon name="plus" size={13} /> {t('network.wizard.hostAdd')}
          </button>
        </fieldset>

        <div class="field">
          <label class="check">
            <input type="checkbox" bind:checked={prune} />
            <span>{t('network.wizard.prune')}</span>
          </label>
          <p class="hint dim">{t('network.wizard.pruneHint')}</p>
          <label class="check">
            <input type="checkbox" bind:checked={snapshot} />
            <span>{t('network.wizard.snapshot')}</span>
          </label>
        </div>
      {/if}
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
    {/if}
    <p class="dim">{t('network.plan.intro')}</p>
    <div class="facts">
      <span class="badge">{t(`network.kind.${plan.proxy_kind}`)} · {plan.proxy_name}</span>
      <span class="badge">{t(`network.wizard.mode_${plan.mode}`)}</span>
      <span class="badge">
        <Icon name="key" size={11} />
        {t(`network.plan.secret.${plan.secret}`)}{#if plan.secret_source}&nbsp;({plan.secret_source}){/if}
      </span>
    </div>
    <div class="counts">
      <span class="badge">{t('network.plan.counts.change', { n: plan.counts.change })}</span>
      <span class="badge">{t('network.plan.counts.ok', { n: plan.counts.ok })}</span>
      {#if plan.counts.manual}<span class="badge warn">{t('network.plan.counts.manual', { n: plan.counts.manual })}</span>{/if}
      {#if plan.counts.refused}<span class="badge danger">{t('network.plan.counts.refused', { n: plan.counts.refused })}</span>{/if}
    </div>

    <h3>{t('network.plan.entries')}</h3>
    <table class="entries">
      <thead>
        <tr>
          <th scope="col">{t('network.wizard.col.name')}</th>
          <th scope="col">{t('network.plan.address')}</th>
          <th scope="col">{t('network.wizard.col.server')}</th>
          <th scope="col">{t('network.plan.change')}</th>
        </tr>
      </thead>
      <tbody>
        {#each plan.entries as e, ei (ei)}
          <tr>
            <td class="mono">{e.name}</td>
            <td class="mono"><Masked value={e.address} kind="address" /></td>
            <td>{e.server}</td>
            <td><span class="badge {ENTRY_CHANGE[e.change]}">{t(`network.plan.entry.${e.change}`)}</span></td>
          </tr>
        {/each}
      </tbody>
    </table>
    <p class="line">
      <strong>{t('network.plan.tryOrder')}</strong>
      <span class="mono">{plan.try_order.join(' → ') || '—'}</span>
      {#if plan.try_changed}<span class="badge warn">{t('network.plan.modified')}</span>{/if}
    </p>
    {#if plan.forced_hosts.length > 0}
      <h3>{t('network.forcedHosts')}</h3>
      <ul class="plain">
        {#each plan.forced_hosts as h, hi (hi)}
          <li>
            <span class="mono"><Masked value={h.host} kind="address" /></span> → <span class="mono">{h.servers.join(', ')}</span>
            <span class="badge">{t(`network.plan.host.${h.change}`)}</span>
          </li>
        {/each}
      </ul>
    {/if}
    {#if plan.pruned.length > 0}
      <h3>{t('network.plan.pruned')}</h3>
      <ul class="plain">
        {#each plan.pruned as e, pi (pi)}
          <li><span class="mono">{e.name}</span> <span class="dim mono"><Masked value={e.address} kind="address" /></span></li>
        {/each}
      </ul>
    {/if}
    {#if plan.warnings.length > 0}
      <h3>{t('network.plan.warnings')}</h3>
      <ul class="plain warn-list">
        {#each plan.warnings as w, i (i)}<li><Masked value={w} /></li>{/each}
      </ul>
    {/if}
    <h3>{t('network.plan.steps')}</h3>
    <PlanSteps steps={plan.steps} />

    <div class="actions">
      <button class="btn btn--ghost" onclick={() => ((stage = 'form'), (result = null))}>{t('network.wizard.back')}</button>
      <span class="spacer"></span>
      {#if plan.counts.refused > 0}
        <span class="dim small">{t('network.plan.refusedHint')}</span>
      {:else if plan.counts.change === 0}
        <span class="dim small">{t('network.plan.nothingHint')}</span>
      {/if}
      <button class="btn" disabled={plan.counts.change === 0 || plan.counts.refused > 0} onclick={() => (stage = 'confirm')}>
        {t('network.wizard.applyEllipsis')}
      </button>
    </div>
  {:else if stage === 'confirm' && plan}
    <div class="confirm">
      <p>{t('network.confirm.body', { n: plan.counts.change, servers: changedServers(plan.steps) })}</p>
      <ul class="plain">
        <li>{snapshot ? t('network.confirm.snapshotOn') : t('network.confirm.snapshotOff')}</li>
        <li>{t('network.confirm.undo')}</li>
        <li>{t('network.confirm.restart')}</li>
        {#if plan.secret === 'generated'}<li>{t('network.confirm.secret')}</li>{/if}
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
      restart={t('network.result.restartLink')}
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
  .radios {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 18px;
  }
  table.entries {
    width: 100%;
    border-collapse: collapse;
    font-size: 13px;
  }
  .entries th {
    text-align: left;
    font-size: 11.5px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-muted);
    padding: 5px 6px;
    border-bottom: 1px solid var(--border);
  }
  .entries td {
    padding: 5px 6px;
    border-bottom: 1px solid var(--border);
    vertical-align: middle;
  }
  .ord {
    width: 60px;
    white-space: nowrap;
  }
  .center {
    text-align: center;
  }
  .small-input {
    padding: 6px 9px;
    font-size: 13px;
  }
  .invalid {
    border-color: var(--state-danger);
  }
  .err {
    color: var(--state-danger);
    margin: 4px 0 0;
  }
  .host-row {
    display: grid;
    grid-template-columns: 1fr auto 1fr auto;
    align-items: center;
    gap: 8px;
    margin-bottom: 6px;
  }
  .icon-btn {
    display: inline-grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border: none;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
  }
  .icon-btn:hover:not(:disabled) {
    background: var(--hover);
    color: var(--text);
  }
  .icon-btn:disabled {
    opacity: 0.35;
    cursor: default;
  }
  .small-btn {
    padding: 5px 10px;
    font-size: 12.5px;
    align-self: flex-start;
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
  .facts,
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
