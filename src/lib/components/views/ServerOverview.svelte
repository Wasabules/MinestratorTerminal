<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { api } from '$lib/ipc';
  import { consoleEvents } from '$lib/events';
  import { t } from '$lib/i18n';
  import { runtimeMeta, isRunning } from '$lib/status';
  import { fmtBytes } from '$lib/copilot/format';
  import { tabs, VIEWS, type ServerTab, type ServerView } from '$lib/tabs/tabs.svelte';
  import type { ConsoleStats, LiveLight, MetricSample } from '$lib/types';
  import Gauge from '../Gauge.svelte';
  import PowerControl from '../PowerControl.svelte';
  import StatusDot from '../StatusDot.svelte';
  import MetricsChart from '../MetricsChart.svelte';
  import Icon from '../Icon.svelte';

  let { tab }: { tab: ServerTab } = $props();
  const serverId = $derived(tab.serverId);
  const connId = $derived(tab.id);

  let live = $state<LiveLight | null>(null);
  let address = $state(''); // adresse de connexion (dns ou ip:port), depuis la liste des serveurs
  let copied = $state(false);
  let copiedTimer: ReturnType<typeof setTimeout> | undefined;
  let stats = $state<ConsoleStats | null>(null);
  let runState = $state('offline');
  let hibernated = $state(false);
  let metrics = $state<MetricSample[]>([]);
  let range = $state(3600);
  let metricsTimer: ReturnType<typeof setInterval> | undefined;
  let perfTimer: ReturnType<typeof setTimeout> | undefined;
  let perfBusy = $state(false);
  const unlisteners: UnlistenFn[] = [];

  const openable = VIEWS.filter((v) => v.id !== 'overview');
  const rt = $derived(runtimeMeta(runState));
  const running = $derived(isRunning(runState));

  const cpuPct = $derived(
    stats && live && live.cpu.limit > 0 ? (stats.cpu_absolute / live.cpu.limit) * 100 : 0
  );
  const ramPct = $derived(
    stats && stats.memory_limit_bytes > 0 ? (stats.memory_bytes / stats.memory_limit_bytes) * 100 : 0
  );
  const diskLimitBytes = $derived((live?.disk.limit ?? 0) * 1024 * 1024);
  const diskPct = $derived(
    stats && diskLimitBytes > 0 ? (stats.disk_bytes / diskLimitBytes) * 100 : 0
  );

  let destroyed = false;
  onMount(() => {
    void loadLive();
    void loadAddress();
    void connect();
    void loadMetrics();
    metricsTimer = setInterval(loadMetrics, 15000);
  });
  onDestroy(() => {
    destroyed = true;
    unlisteners.forEach((u) => u());
    clearInterval(metricsTimer);
    clearTimeout(perfTimer);
    clearTimeout(copiedTimer);
    api.consoleDisconnect(connId).catch(() => {});
  });

  async function loadLive() {
    try {
      live = await api.liveLight(serverId);
    } catch {
      /* limites indisponibles */
    }
  }

  async function loadAddress() {
    try {
      const list = await api.listServers();
      address = list.servers.find((s) => s.id === serverId)?.address ?? '';
    } catch {
      /* liste indisponible */
    }
  }

  function copyAddress() {
    if (!address) return;
    void navigator.clipboard.writeText(address).catch(() => {});
    copied = true;
    clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => (copied = false), 1500);
  }

  async function loadMetrics() {
    try {
      metrics = await api.metricsHistory(serverId, range);
    } catch {
      /* historique indisponible */
    }
  }

  function setRange(secs: number) {
    range = secs;
    void loadMetrics();
  }

  async function runPerf() {
    perfBusy = true;
    try {
      await api.copilotPerformance(serverId, tab.serverName);
    } catch {
      /* le résultat (ou l'erreur) apparaît dans le Copilote */
    }
    // L'analyse tourne en fond (~35 s + LLM) → indicateur Copilote. On rouvre le bouton vite.
    clearTimeout(perfTimer);
    perfTimer = setTimeout(() => (perfBusy = false), 3000);
  }

  async function connect() {
    unlisteners.push(
      await consoleEvents.stats((p) => {
        if (p.conn_id === connId) {
          stats = p;
          runState = p.state || runState;
        }
      })
    );
    unlisteners.push(
      await consoleEvents.status((p) => {
        if (p.conn_id === connId) runState = p.state;
      })
    );
    unlisteners.push(
      await consoleEvents.connection((p) => {
        if (p.conn_id === connId) hibernated = p.phase === 'hibernated';
      })
    );
    // Démonté pendant l'enregistrement → détacher les listeners et ne pas ouvrir la connexion.
    if (destroyed) {
      unlisteners.forEach((u) => u());
      return;
    }
    await api.consoleConnect(connId, serverId).catch(() => {});
    if (destroyed) api.consoleDisconnect(connId).catch(() => {});
  }

  function open(view: ServerView, newTab = false) {
    if (newTab) tabs.openNew(serverId, tab.serverName, view);
    else tabs.focusOrOpen(serverId, tab.serverName, view);
  }

  function fmtUptime(ms: number): string {
    const s = Math.floor(ms / 1000);
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    if (h > 0) return `${h} h ${m} min`;
    return `${m} min`;
  }
</script>

<div class="view">
  <header class="head">
    <div class="title">
      <h1>{tab.serverName}</h1>
      <span class="pill" style="color: {rt.color}">
        <StatusDot color={rt.color} pulse={running} />
        {t(`status.${rt.key}`)}
      </span>
    </div>
    <PowerControl {serverId} {running} disabled={hibernated} />
  </header>

  {#if address}
    <button class="addrbar" onclick={copyAddress} title={t('overview.copyAddress')}>
      <span class="ag"><Icon name="globe" size={15} /></span>
      <span class="av">{address}</span>
      <span class="ac">
        {#if copied}
          <Icon name="check" size={14} /> {t('overview.copied')}
        {:else}
          <Icon name="copy" size={14} /> {t('overview.copy')}
        {/if}
      </span>
    </button>
  {/if}

  {#if hibernated}
    <div class="note">{t('overview.hibernated')}</div>
  {/if}

  <section class="gauges">
    <div class="card g">
      <Gauge
        label={t('overview.cpu')}
        percent={cpuPct}
        detail={stats ? `${stats.cpu_absolute.toFixed(1)} % / ${live?.cpu.limit ?? 0} %` : t('overview.noData')}
      />
    </div>
    <div class="card g">
      <Gauge
        label={t('overview.ram')}
        percent={ramPct}
        detail={stats ? `${fmtBytes(stats.memory_bytes)} / ${fmtBytes(stats.memory_limit_bytes)}` : '—'}
      />
    </div>
    <div class="card g">
      <Gauge
        label={t('overview.disk')}
        percent={diskPct}
        detail={stats ? `${fmtBytes(stats.disk_bytes)} / ${fmtBytes(diskLimitBytes)}` : '—'}
      />
    </div>
  </section>

  <section class="facts">
    <div class="fact">
      <span class="k">{t('overview.players')}</span>
      <span class="v">{live?.players ? `${live.players.current} / ${live.players.limit}` : '—'}</span>
    </div>
    <div class="fact">
      <span class="k">{t('overview.version')}</span>
      <span class="v">{live?.version ?? '—'}</span>
    </div>
    <div class="fact">
      <span class="k">{t('overview.uptime')}</span>
      <span class="v">{stats && stats.uptime > 0 ? fmtUptime(stats.uptime) : '—'}</span>
    </div>
  </section>

  <section class="block">
    <div class="hist-head">
      <h2>{t('overview.history')}</h2>
      <div class="hh-right">
        <button
          class="perfbtn"
          onclick={runPerf}
          disabled={perfBusy || !running}
          title={running ? t('overview.perfHint') : t('overview.perfOffline')}
        >
          <Icon name="zap" size={15} /> {perfBusy ? t('overview.perfRunning') : t('overview.perf')}
        </button>
        <div class="ranges">
          <button class="rg" class:on={range === 3600} onclick={() => setRange(3600)}>1h</button>
          <button class="rg" class:on={range === 21600} onclick={() => setRange(21600)}>6h</button>
          <button class="rg" class:on={range === 86400} onclick={() => setRange(86400)}>24h</button>
        </div>
      </div>
    </div>
    <div class="card chart-card">
      <MetricsChart samples={metrics} cpuLimit={live?.cpu.limit ?? 0} />
    </div>
  </section>

  <section class="block">
    <h2>{t('overview.openView')}</h2>
    <div class="views">
      {#each openable as v (v.id)}
        <div class="vcard">
          <button class="vmain" onclick={() => open(v.id)}>
            <span class="vg"><Icon name={v.icon} size={17} /></span>
            <span class="vl">{t(`view.${v.id}`)}</span>
            {#if !v.ready}<span class="soon">{t('common.comingSoon')}</span>{/if}
          </button>
          <button class="vnew" title="+" onclick={() => open(v.id, true)}>+</button>
        </div>
      {/each}
    </div>
    <p class="dim hint">{t('overview.newTabHint')}</p>
  </section>
</div>

<style>
  .view {
    max-width: 900px;
    margin: 0 auto;
    padding: 26px 24px 48px;
    display: flex;
    flex-direction: column;
    gap: 24px;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    flex-wrap: wrap;
  }
  .title {
    display: flex;
    align-items: center;
    gap: 14px;
  }
  .title h1 {
    margin: 0;
    font-size: 24px;
    letter-spacing: -0.02em;
  }
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    font-weight: 600;
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 5px 12px;
  }
  .note {
    border: 1px solid color-mix(in srgb, var(--state-hibernate) 40%, var(--border));
    background: color-mix(in srgb, var(--state-hibernate) 10%, transparent);
    border-radius: var(--radius);
    padding: 12px 14px;
    font-size: 13.5px;
    color: var(--text-muted);
  }
  /* Barre d'adresse de connexion (clic = copie). */
  .addrbar {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    cursor: pointer;
    font: inherit;
    color: var(--text);
    padding: 10px 14px;
    text-align: left;
  }
  .addrbar:hover {
    border-color: color-mix(in srgb, var(--brand-primary) 45%, var(--border));
  }
  .ag {
    display: inline-flex;
    color: var(--brand-primary);
    flex: none;
  }
  .av {
    flex: 1;
    min-width: 0;
    font-family: var(--font-mono);
    font-size: 14px;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ac {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    flex: none;
    font-size: 12px;
    color: var(--text-dim);
  }
  .addrbar:hover .ac {
    color: var(--text-muted);
  }
  .gauges {
    display: grid;
    gap: 14px;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
  }
  .g {
    padding: 16px 18px;
  }
  .facts {
    display: grid;
    gap: 14px;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
  }
  .fact {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .fact .k {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.07em;
    color: var(--text-dim);
  }
  .fact .v {
    font-size: 18px;
    font-weight: 700;
    font-family: var(--font-mono);
  }
  .block {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .block h2 {
    margin: 0;
    font-size: 13px;
    text-transform: uppercase;
    letter-spacing: 0.09em;
    color: var(--text-muted);
    font-family: var(--font-mono);
  }
  .hist-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
  }
  .hh-right {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .perfbtn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    background: color-mix(in srgb, var(--brand-primary) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--brand-primary) 45%, var(--border));
    border-radius: 8px;
    cursor: pointer;
    font: inherit;
    font-size: 12.5px;
    font-weight: 600;
    color: var(--text);
    padding: 5px 12px;
    white-space: nowrap;
  }
  .perfbtn :global(.icon) {
    color: var(--brand-primary);
  }
  .perfbtn:hover:not(:disabled) {
    background: color-mix(in srgb, var(--brand-primary) 20%, transparent);
  }
  .perfbtn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .ranges {
    display: flex;
    gap: 4px;
  }
  .rg {
    background: none;
    border: 1px solid var(--border);
    border-radius: 7px;
    cursor: pointer;
    font: inherit;
    font-size: 12px;
    color: var(--text-muted);
    padding: 4px 10px;
  }
  .rg.on {
    color: #fff;
    background: var(--brand-primary);
    border-color: transparent;
  }
  .chart-card {
    padding: 14px 16px 12px;
  }
  .views {
    display: grid;
    gap: 10px;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
  }
  .vcard {
    display: flex;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
    background: var(--surface);
  }
  .vmain {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 10px;
    background: none;
    border: none;
    cursor: pointer;
    font: inherit;
    color: var(--text);
    padding: 12px 14px;
  }
  .vmain:hover {
    background: color-mix(in srgb, var(--text) 5%, transparent);
  }
  .vg {
    display: inline-flex;
    align-items: center;
    color: var(--brand-primary);
  }
  .vl {
    font-weight: 600;
    font-size: 14px;
  }
  .soon {
    margin-left: auto;
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 1px 7px;
  }
  .vnew {
    background: none;
    border: none;
    border-left: 1px solid var(--border);
    color: var(--text-dim);
    cursor: pointer;
    font-size: 17px;
    padding: 0 12px;
  }
  .vnew:hover {
    color: var(--brand-primary);
  }
  .hint {
    font-size: 12.5px;
    margin: 0;
  }
</style>
