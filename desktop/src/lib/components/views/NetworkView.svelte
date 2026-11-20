<script lang="ts">
  /**
   * Vue Réseau : réseaux proxy (Velocity, BungeeCord) d'une MyBox. Pour chaque proxy, un schéma
   * proxy → serveurs de jeu (état, ping, réglages et problèmes en badges), puis les problèmes et
   * leur correction. Liaison et préréglages passent par des assistants : plan simulé d'abord,
   * application après confirmation. Toute la logique (lecture, rapprochement des adresses, plans,
   * secrets comparés sans jamais être lus ici) vit dans `core/src/network/`.
   */
  import { onMount } from 'svelte';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';
  import { tabs } from '#lib/tabs/tabs.svelte.ts';
  import type {
    MyBoxSummary,
    NetworkEntry,
    NetworkPing,
    NetworkProblem,
    NetworkProxy,
    NetworkServer,
    NetworkSeverity,
    NetworkTopology,
  } from '#lib/types.ts';
  import Icon from '../Icon.svelte';
  import Masked from '../Masked.svelte';
  import StatusDot from '../StatusDot.svelte';
  import BiomeBlock from '../BiomeBlock.svelte';
  import { serverBiome } from '#lib/servers/colors.svelte.ts';
  import { serverRuntime } from '#lib/servers/runtime.svelte.ts';
  import { runtimeMeta, statusMeta } from '#lib/status.ts';
  import LinkWizard from '#lib/network/LinkWizard.svelte';
  import PresetWizard from '#lib/network/PresetWizard.svelte';
  import { platformLabel, SEVERITY_ORDER } from '#lib/network/util.ts';

  /** MyBox choisie, mémorisée pour ce poste (confort, jamais indispensable). */
  const MYBOX_KEY = 'mnstr-network-mybox';
  const PING_BATCH = 40;

  let myboxes = $state<MyBoxSummary[]>([]);
  /** `null` : toutes les MyBox. `undefined` : pas encore choisie. */
  let mybox = $state<number | null | undefined>(undefined);
  let topo = $state<NetworkTopology | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let pings = $state<Record<number, NetworkPing>>({});
  let pinging = $state<Record<number, boolean>>({});
  let bedrockPings = $state<Record<number, NetworkPing>>({});
  let bedrockPinging = $state<Record<number, boolean>>({});
  let wizard = $state<{ kind: 'link' | 'preset'; proxyId: number | null } | null>(null);

  const servers = $derived(new Map<number, NetworkServer>((topo?.servers ?? []).map((s) => [s.id, s])));
  const entryCount = $derived((topo?.proxies ?? []).reduce((n, p) => n + p.entries.length, 0));
  /** Serveurs du schéma (proxys et serveurs reconnus), pour « Pinger tout ». */
  const pingTargets = $derived.by(() => {
    const ids = new Set<number>();
    for (const p of topo?.proxies ?? []) {
      ids.add(p.server_id);
      for (const e of p.entries) if (e.target && !e.target.self_proxy) ids.add(e.target.server_id);
    }
    return [...ids].filter((id) => servers.get(id)?.readable !== false);
  });
  const anyPinging = $derived(Object.values(pinging).some(Boolean));

  function savedMybox(): number | null | undefined {
    try {
      const v = localStorage.getItem(MYBOX_KEY);
      if (v === 'all') return null;
      const n = Number(v);
      return v && Number.isFinite(n) ? n : undefined;
    } catch {
      return undefined;
    }
  }

  function remember(v: number | null) {
    try {
      localStorage.setItem(MYBOX_KEY, v === null ? 'all' : String(v));
    } catch {
      /* stockage indisponible : le choix vaut pour la session */
    }
  }

  async function init() {
    loading = true;
    error = null;
    try {
      const overview = await api.listServers();
      myboxes = overview.myboxes;
      const saved = savedMybox();
      const valid = saved === null || myboxes.some((m) => m.id === saved);
      // Par défaut : la première MyBox qui a des serveurs.
      mybox = saved !== undefined && valid
        ? saved
        : (myboxes.find((m) => overview.servers.some((s) => s.mybox_id === m.id))?.id ?? null);
      await load();
    } catch (e) {
      error = humanizeError(e);
      loading = false;
    }
  }

  async function load() {
    loading = true;
    error = null;
    try {
      topo = await api.networkTopology(mybox ?? null);
    } catch (e) {
      error = humanizeError(e);
    } finally {
      loading = false;
    }
  }

  function chooseMybox(v: string) {
    mybox = v === 'all' ? null : Number(v);
    remember(mybox);
    pings = {};
    bedrockPings = {};
    void load();
  }

  onMount(init);

  async function ping(ids: number[]) {
    const todo = ids.filter((id) => !pinging[id]);
    if (todo.length === 0) return;
    for (const id of todo) pinging[id] = true;
    try {
      // Le cœur pingue 40 serveurs au plus par appel (8 à la fois).
      for (let i = 0; i < todo.length; i += PING_BATCH) {
        const res = await api.networkPing(todo.slice(i, i + PING_BATCH));
        for (const p of res) pings[p.server_id] = p;
      }
    } catch (e) {
      error = humanizeError(e);
    } finally {
      for (const id of todo) pinging[id] = false;
    }
  }

  /** Ping Bedrock (UDP) d'un proxy : vérifie Geyser après le préréglage. */
  async function pingBedrock(id: number) {
    if (bedrockPinging[id]) return;
    bedrockPinging[id] = true;
    try {
      const [p] = await api.networkPing([id], true);
      if (p) bedrockPings[id] = p;
    } catch (e) {
      error = humanizeError(e);
    } finally {
      bedrockPinging[id] = false;
    }
  }

  /** État affiché : l'état d'exécution suivi par l'app s'il est connu, sinon celui de la liste. */
  function stateOf(id: number) {
    const runtime = serverRuntime(id);
    if (runtime) return runtimeMeta(runtime);
    return statusMeta(servers.get(id)?.status ?? '');
  }

  function problemsOf(p: NetworkProxy, serverId: number | null): NetworkProblem[] {
    return p.problems.filter((x) => x.server_id === serverId);
  }

  /** Plus forte gravité parmi des problèmes (`null` : aucun). */
  function worst(list: NetworkProblem[]): NetworkSeverity | null {
    for (const s of SEVERITY_ORDER) if (list.some((x) => x.severity === s)) return s;
    return null;
  }

  function entryProblems(p: NetworkProxy, e: NetworkEntry): NetworkProblem[] {
    return e.target && !e.target.self_proxy ? problemsOf(p, e.target.server_id) : [];
  }

  function openServer(id: number) {
    const s = servers.get(id);
    tabs.focusOrOpen(id, s?.name ?? String(id), 'overview');
  }

  function countBy(list: NetworkProblem[]) {
    return SEVERITY_ORDER.map((s) => ({ s, n: list.filter((x) => x.severity === s).length })).filter((c) => c.n > 0);
  }

  function closeWizard(changed: boolean) {
    wizard = null;
    if (changed) void load();
  }
</script>

{#snippet pingLine(id: number)}
  {@const p = pings[id]}
  <span class="ping" aria-live="polite">
    {#if pinging[id]}
      <span class="spinner small" aria-hidden="true"></span><span class="dim">{t('network.ping.pending')}</span>
    {:else if !p}
      <span class="dim">{t('network.ping.none')}</span>
    {:else if p.online}
      <span class="lat">{t('network.ping.latency', { ms: p.latency_ms ?? '?' })}</span>
      {#if p.players_max != null}
        <span class="dim">· {t('network.ping.players', { online: p.players_online ?? 0, max: p.players_max })}</span>
      {/if}
      {#if p.version}<span class="dim">· {p.version}</span>{/if}
    {:else}
      <span class="off" title={p.error ?? ''}>{t('network.ping.offline')}</span>
    {/if}
  </span>
{/snippet}

{#snippet nodeActions(id: number, name: string)}
  <span class="node-acts">
    <button
      class="icon-btn"
      title={t('network.ping.action')}
      aria-label={`${t('network.ping.action')} — ${name}`}
      disabled={!!pinging[id]}
      onclick={() => ping([id])}
    >
      <Icon name="activity" size={14} />
    </button>
    <button
      class="icon-btn"
      title={t('network.openServer')}
      aria-label={`${t('network.openServer')} — ${name}`}
      onclick={() => openServer(id)}
    >
      <Icon name="external-link" size={14} />
    </button>
  </span>
{/snippet}

{#snippet severityBadges(list: NetworkProblem[])}
  {#each countBy(list) as c (c.s)}
    <span class="badge sev {c.s}" title={t(`network.severity.${c.s}`)}>
      <Icon name={c.s === 'info' ? 'info' : 'triangle-alert'} size={12} />
      {c.n}
      <span class="sr">{t(`network.severity.${c.s}`)}</span>
    </span>
  {/each}
{/snippet}

<div class="net">
  <header class="head">
    <div>
      <h1>{t('network.title')}</h1>
      <p class="dim">{t('network.subtitle')}</p>
    </div>
    <div class="head-actions">
      <label class="mb-pick">
        <span class="label">{t('network.mybox')}</span>
        <select
          class="input"
          value={mybox === null ? 'all' : String(mybox ?? '')}
          disabled={mybox === undefined || loading}
          onchange={(e) => chooseMybox(e.currentTarget.value)}
        >
          {#each myboxes as m (m.id)}
            <option value={String(m.id)}>{m.name}</option>
          {/each}
          <option value="all">{t('network.allMyboxes')}</option>
        </select>
      </label>
      <button class="btn btn--ghost" onclick={load} disabled={loading || mybox === undefined}>
        {#if loading}<span class="spinner small" aria-hidden="true"></span>{/if}
        {t('common.refresh')}
      </button>
      <button
        class="btn btn--ghost"
        onclick={() => ping(pingTargets)}
        disabled={!topo || pingTargets.length === 0 || anyPinging}
      >
        <Icon name="activity" size={14} />
        {anyPinging ? t('network.ping.pending') : t('network.pingAll')}
      </button>
      <button class="btn btn--ghost" disabled={!topo || topo.proxies.length === 0} onclick={() => (wizard = { kind: 'preset', proxyId: null })}>
        <Icon name="package" size={14} />
        {t('network.presets')}
      </button>
      <button class="btn" disabled={!topo || topo.proxies.length === 0} onclick={() => (wizard = { kind: 'link', proxyId: null })}>
        <Icon name="link" size={14} />
        {t('network.link')}
      </button>
    </div>
  </header>

  {#if loading && !topo}
    <div class="state" role="status"><span class="spinner" aria-hidden="true"></span> {t('network.loading')}</div>
  {:else if error && !topo}
    <div class="state">
      <p class="alert" role="alert">{error}</p>
      <button class="btn" onclick={init}>{t('common.retry')}</button>
    </div>
  {:else if topo}
    {#if error}<p class="alert" role="alert">{error}</p>{/if}
    <div class="summary">
      <span class="dim">{t('network.summary', { proxies: topo.proxies.length, entries: entryCount })}</span>
      {#if topo.counts.critical}<span class="badge sev critical">{t('network.counts.critical', { n: topo.counts.critical })}</span>{/if}
      {#if topo.counts.important}<span class="badge sev important">{t('network.counts.important', { n: topo.counts.important })}</span>{/if}
      {#if topo.counts.info}<span class="badge sev info">{t('network.counts.info', { n: topo.counts.info })}</span>{/if}
    </div>

    {#if topo.proxies.length === 0}
      <section class="card empty">
        <Icon name="network" size={28} />
        <h2>{t('network.noProxy')}</h2>
        <p class="dim">{t('network.noProxyHint')}</p>
        <button class="btn btn--ghost" onclick={() => tabs.openMyboxes()}>{t('mybox.manage')}</button>
      </section>
    {/if}

    {#each topo.proxies as p (p.server_id)}
      {@const own = problemsOf(p, null)}
      <section class="card proxy-card" aria-labelledby={`proxy-${p.server_id}`}>
        <div class="schema">
          <div class="hub" class:empty-hub={p.entries.length === 0}>
            <div class="node proxy-node" class:bad={worst(own) === 'critical'}>
              <div class="node-head">
                <BiomeBlock biome={serverBiome(p.server_id)} size={12} />
                <h2 id={`proxy-${p.server_id}`} class="node-name">{p.name}</h2>
                <StatusDot color={stateOf(p.server_id).color} />
                <span class="sr">{t(`status.${stateOf(p.server_id).key}`)}</span>
              </div>
              <div class="chips">
                <span class="badge">{t(`network.kind.${p.kind}`)}</span>
                <span class="badge" class:warn={p.forwarding === 'none' && p.entries.length > 0}>
                  {t(`network.forwarding.${p.forwarding}`)}
                </span>
                {#if p.online_mode === false}<span class="badge danger">online-mode=false</span>{/if}
                {#if p.secret_defined === true}
                  <span class="badge ok" title={p.secret_note}><Icon name="key" size={11} /> {t('network.secretDefined')}</span>
                {:else if p.secret_defined === false && (p.forwarding === 'modern' || p.forwarding === 'bungeeguard')}
                  <span class="badge danger" title={p.secret_note}>{t('network.secretMissing')}</span>
                {/if}
                {#if p.bungeeguard}<span class="badge" class:ok={p.bungeeguard === 'token'}>{t(`network.guard.${p.bungeeguard}`)}</span>{/if}
                {@render severityBadges(own)}
              </div>
              <div class="node-foot">
                {@render pingLine(p.server_id)}
                {@render nodeActions(p.server_id, p.name)}
              </div>
              <div class="node-foot">
                {#if bedrockPinging[p.server_id]}
                  <span class="ping"><span class="spinner small" aria-hidden="true"></span><span class="dim">{t('network.ping.pending')}</span></span>
                {:else if bedrockPings[p.server_id]}
                  {@const b = bedrockPings[p.server_id]}
                  <span class="ping" aria-live="polite">
                    {#if b.online}
                      <span class="lat">{t('network.ping.bedrockOk', { port: b.port ?? '?', ms: b.latency_ms ?? '?' })}</span>
                    {:else}
                      <span class="off" title={b.error ?? ''}>{t('network.ping.bedrockOff')}</span>
                    {/if}
                  </span>
                {:else}
                  <span></span>
                {/if}
                <button class="btn btn--ghost small-btn" disabled={!!bedrockPinging[p.server_id]} onclick={() => pingBedrock(p.server_id)}>
                  {t('network.ping.bedrock')}
                </button>
              </div>
            </div>
          </div>

          {#if p.entries.length === 0}
            <p class="dim no-entries">{t('network.noEntries')}</p>
          {:else}
            <ul class="branches" aria-label={t('network.entriesOf', { name: p.name })}>
              {#each p.entries as e, ei (ei)}
                {@const probs = entryProblems(p, e)}
                {@const w = worst(probs)}
                {@const tryIndex = p.try_order.findIndex((x) => x.toLowerCase() === e.name.toLowerCase())}
                <li class="branch" class:dashed={!e.target || e.local || e.target.self_proxy} class:bad={w === 'critical'}>
                  <div class="node entry-node" class:muted-node={!e.target || e.local}>
                    <div class="node-head">
                      <span class="entry-name mono">{e.name}</span>
                      {#if tryIndex >= 0}<span class="badge fallback">{t('network.fallback', { n: tryIndex + 1 })}</span>{/if}
                      <span class="addr mono dim"><Masked value={e.address} kind="address" /></span>
                    </div>
                    {#if e.local}
                      <p class="target dim">{t('network.local')}</p>
                    {:else if !e.target}
                      <p class="target dim">{t('network.unknownTarget')}</p>
                    {:else if e.target.self_proxy}
                      <p class="target dim">{t('network.selfProxy')}</p>
                    {:else}
                      {@const st = stateOf(e.target.server_id)}
                      <div class="target">
                        <BiomeBlock biome={serverBiome(e.target.server_id)} size={10} />
                        <span class="tname">{e.target.name ?? e.target.server_id}</span>
                        <StatusDot color={st.color} size={8} />
                        <span class="state-label" style="color: {st.ink}">{t(`status.${st.key}`)}</span>
                        {#if e.target.matched_by !== 'primary' || !e.target.port_ok}
                          <span class="badge warn small-badge">
                            {e.target.port_ok ? t(`network.matched.${e.target.matched_by}`) : t('network.portMismatch')}
                          </span>
                        {/if}
                      </div>
                      {#if e.backend}
                        {@const b = e.backend}
                        <div class="chips">
                          <span class="badge">{platformLabel(b.platform)}{b.version ? ` ${b.version}` : ''}</span>
                          {#if b.chained_proxy}
                            <span class="badge">{t('network.chained')}</span>
                          {:else}
                            {#if b.online_mode === true}<span class="badge warn">online-mode=true</span>{/if}
                            {#if b.paper_config === 'missing' && b.platform === 'paper'}
                              <span class="badge warn">{t('network.paperMissing')}</span>
                            {/if}
                            {#if b.velocity_forwarding === true}<span class="badge">{t('network.velocityOn')}</span>{/if}
                            {#if b.velocity_secret}
                              <span class="badge" class:ok={b.velocity_secret === 'same'} class:danger={b.velocity_secret === 'different' || b.velocity_secret === 'missing'}>
                                <Icon name="key" size={11} /> {t(`network.secret.${b.velocity_secret}`)}
                              </span>
                            {/if}
                            {#if b.bungeecord === true}<span class="badge">bungeecord: true</span>{/if}
                            {#if b.bungeeguard}<span class="badge ok">BungeeGuard</span>{/if}
                            {#if b.fabric_proxy}
                              <span class="badge" class:ok={b.fabric_proxy === 'configured'} class:warn={b.fabric_proxy !== 'configured'}>
                                {t(`network.fabric.${b.fabric_proxy}`)}
                              </span>
                            {/if}
                            {#if b.fabric_secret}
                              <span class="badge" class:ok={b.fabric_secret === 'same'} class:danger={b.fabric_secret !== 'same' && b.fabric_secret !== 'unknown'}>
                                <Icon name="key" size={11} /> {t(`network.secret.${b.fabric_secret}`)}
                              </span>
                            {/if}
                          {/if}
                          {@render severityBadges(probs)}
                        </div>
                      {:else if e.unread}
                        <p class="dim small"><Masked value={t('network.unread', { reason: e.unread })} /></p>
                      {/if}
                      <div class="node-foot">
                        {@render pingLine(e.target.server_id)}
                        {@render nodeActions(e.target.server_id, e.target.name ?? String(e.target.server_id))}
                      </div>
                    {/if}
                  </div>
                </li>
              {/each}
            </ul>
          {/if}
        </div>

        {#if p.forced_hosts.length > 0}
          <div class="hosts">
            <h3>{t('network.forcedHosts')}</h3>
            <ul>
              {#each p.forced_hosts as h, hi (hi)}
                <li><span class="mono"><Masked value={h.host} kind="address" /></span> → <span class="mono">{h.servers.join(', ')}</span></li>
              {/each}
            </ul>
          </div>
        {/if}

        <div class="problems">
          <div class="problems-head">
            <h3>{t('network.problems')}</h3>
            <button class="btn btn--ghost small-btn" onclick={() => (wizard = { kind: 'link', proxyId: p.server_id })}>
              <Icon name="link" size={13} />
              {t('network.linkThis')}
            </button>
          </div>
          {#if p.problems.length === 0}
            <p class="dim ok-line"><Icon name="check" size={14} /> {t('network.noProblems')}</p>
          {:else}
            <ul class="plist">
              {#each p.problems as x, i (i)}
                <li class="problem">
                  <span class="badge sev {x.severity}">{t(`network.severity.${x.severity}`)}</span>
                  <div class="ptext">
                    <p><Masked value={x.message} /></p>
                    {#if x.advice}<p class="dim advice"><Masked value={x.advice} /></p>{/if}
                  </div>
                  {#if x.link_fix}
                    <button class="btn btn--ghost small-btn" onclick={() => (wizard = { kind: 'link', proxyId: p.server_id })}>
                      {t('network.fixWithWizard')}
                    </button>
                  {/if}
                </li>
              {/each}
            </ul>
          {/if}
        </div>
      </section>
    {/each}

    {#if topo.not_read.length > 0 || topo.notes.length > 0}
      <details class="card more" open={topo.proxies.length === 0}>
        <summary>{t('network.notRead', { n: topo.not_read.length + topo.notes.length })}</summary>
        <ul>
          {#each [...topo.not_read, ...topo.notes] as line, i (i)}
            <li class="dim"><Masked value={line} /></li>
          {/each}
        </ul>
      </details>
    {/if}
    <p class="dim small foot-note">{t('network.secretsNote')}</p>
  {/if}
</div>

{#if wizard && topo}
  {#if wizard.kind === 'link'}
    <LinkWizard topology={topo} proxyId={wizard.proxyId} onclose={closeWizard} />
  {:else}
    <PresetWizard topology={topo} proxyId={wizard.proxyId} onclose={closeWizard} />
  {/if}
{/if}

<style>
  .net {
    max-width: 1180px;
    margin: 0 auto;
    padding: 28px 24px 48px;
    display: flex;
    flex-direction: column;
    gap: 18px;
    /* Trait des liaisons du schéma. */
    --line: var(--border-strong);
  }
  .head {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 16px;
  }
  .head h1 {
    margin: 0;
    font-size: 24px;
    letter-spacing: -0.02em;
  }
  .head p {
    margin: 4px 0 0;
    font-size: 13px;
  }
  .head-actions {
    display: flex;
    align-items: flex-end;
    flex-wrap: wrap;
    gap: 8px;
  }
  .mb-pick {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .mb-pick .label {
    font-size: 11.5px;
  }
  .mb-pick select {
    font-family: var(--font-sans);
    padding: 8px 10px;
    min-width: 180px;
  }
  .state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 14px;
    padding: 48px 0;
    color: var(--text-muted);
  }
  .summary {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
    font-size: 13px;
  }
  .empty {
    padding: 28px;
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: 10px;
    color: var(--text-muted);
  }
  .empty h2 {
    margin: 0;
    font-size: 17px;
    color: var(--text);
  }
  .empty p {
    margin: 0;
    max-width: 560px;
    font-size: 13.5px;
  }
  .proxy-card {
    padding: 18px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  /* --- Schéma : proxy à gauche, serveurs à droite, reliés par un tronc et des branches. --- */
  .schema {
    display: grid;
    grid-template-columns: minmax(240px, 300px) 1fr;
    align-items: center;
  }
  .hub {
    position: relative;
    padding-right: 28px;
  }
  .hub::after {
    content: '';
    position: absolute;
    right: 0;
    top: 50%;
    width: 28px;
    border-top: 2px solid var(--line);
  }
  .hub.empty-hub::after {
    display: none;
  }
  .branches {
    list-style: none;
    margin: 0;
    padding: 0 0 0 28px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .branch {
    position: relative;
  }
  /* Tronc : segment vertical de chaque branche, prolongé dans l'espace entre deux nœuds. */
  .branch::before {
    content: '';
    position: absolute;
    left: -28px;
    top: -5px;
    bottom: -5px;
    border-left: 2px solid var(--line);
  }
  .branch:first-child::before {
    top: 50%;
  }
  .branch:last-child::before {
    bottom: 50%;
  }
  .branch:only-child::before {
    display: none;
  }
  .branch::after {
    content: '';
    position: absolute;
    left: -28px;
    top: 50%;
    width: 28px;
    border-top: 2px solid var(--line);
  }
  .branch.dashed::after {
    border-top-style: dashed;
  }
  .branch.bad::after {
    border-top-color: var(--state-danger);
  }
  .no-entries {
    margin: 0;
    padding-left: 16px;
    font-size: 13px;
  }

  .node {
    background: var(--inset);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 7px;
    min-width: 0;
  }
  .proxy-node {
    background: var(--elevated);
    border-color: var(--border-strong);
  }
  .node.bad {
    border-color: color-mix(in srgb, var(--state-danger) 55%, var(--border));
  }
  .muted-node {
    opacity: 0.8;
  }
  .node-head {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 7px;
    min-width: 0;
  }
  .node-name {
    margin: 0;
    font-size: 15px;
    font-weight: 900;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .entry-name {
    font-weight: 800;
    font-size: 13px;
  }
  .addr {
    font-size: 12px;
    margin-left: auto;
  }
  .target {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
    margin: 0;
    font-size: 13px;
  }
  .tname {
    font-weight: 800;
  }
  .state-label {
    font-size: 12px;
    font-weight: 700;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
  }
  .chips .badge,
  .small-badge {
    font-size: 11.5px;
    padding: 2px 8px;
    gap: 4px;
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
  .badge.fallback {
    font-size: 11px;
    padding: 1px 7px;
    color: var(--accent-text);
    background: var(--accent-soft);
    border-color: transparent;
  }
  .badge.sev {
    font-size: 11.5px;
    padding: 2px 8px;
    gap: 4px;
  }
  .badge.sev.critical {
    color: var(--state-danger);
    border-color: color-mix(in srgb, var(--state-danger) 40%, var(--border));
  }
  .badge.sev.important {
    color: var(--state-pending);
    border-color: color-mix(in srgb, var(--state-pending) 40%, var(--border));
  }
  .badge.sev.info {
    color: var(--text-muted);
  }
  .node-foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    font-size: 12px;
  }
  .ping {
    display: inline-flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 5px;
    font-family: var(--font-mono);
    font-size: 11.5px;
  }
  .lat {
    color: var(--state-running-text);
    font-weight: 700;
  }
  .off {
    color: var(--state-danger);
    font-weight: 700;
  }
  .node-acts {
    display: inline-flex;
    gap: 2px;
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
    opacity: 0.4;
    cursor: default;
  }
  .spinner.small {
    width: 11px;
    height: 11px;
  }

  .hosts h3,
  .problems h3 {
    margin: 0;
    font-size: 13px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-muted);
  }
  .hosts ul {
    margin: 6px 0 0;
    padding-left: 18px;
    font-size: 13px;
  }
  .problems {
    display: flex;
    flex-direction: column;
    gap: 8px;
    border-top: 1px solid var(--border);
    padding-top: 12px;
  }
  .problems-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .plist {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .problem {
    display: grid;
    grid-template-columns: 92px 1fr auto;
    align-items: start;
    gap: 10px;
    font-size: 13px;
  }
  .problem .badge {
    justify-self: start;
  }
  .ptext p {
    margin: 0;
  }
  .advice {
    margin-top: 3px !important;
    font-size: 12.5px;
  }
  .small-btn {
    padding: 5px 10px;
    font-size: 12.5px;
  }
  .ok-line {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0;
    font-size: 13px;
    color: var(--state-running-text);
  }
  .more {
    padding: 12px 16px;
    font-size: 13px;
  }
  .more summary {
    cursor: pointer;
    font-weight: 800;
  }
  .more ul {
    margin: 8px 0 0;
    padding-left: 18px;
  }
  .small {
    font-size: 12px;
    margin: 0;
  }
  .foot-note {
    margin: 0;
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: -1px;
    padding: 0;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }

  @media (max-width: 760px) {
    .schema {
      grid-template-columns: 1fr;
      gap: 12px;
    }
    .hub {
      padding-right: 0;
    }
    .hub::after,
    .branch::before,
    .branch::after {
      display: none;
    }
    .branches {
      padding-left: 0;
    }
    .problem {
      grid-template-columns: 1fr;
    }
  }
</style>
