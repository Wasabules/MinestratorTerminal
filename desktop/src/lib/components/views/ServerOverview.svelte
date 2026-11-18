<script lang="ts">
  import { onMount, onDestroy, untrack } from 'svelte';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { api } from '#lib/ipc.ts';
  import { consoleEvents } from '#lib/events.ts';
  import { t } from '#lib/i18n/index.ts';
  import { runtimeMeta, isRunning } from '#lib/status.ts';
  import { fmtBytes } from '#lib/copilot/format.ts';
  import { tabs, type ServerTab, type ServerView } from '#lib/tabs/tabs.svelte.ts';
  import { serverCaps, availableViews } from '#lib/games/capabilities.svelte.ts';
  import { loadUpdates, updateCount } from '#lib/mods/updates.svelte.ts';
  import type {
    ConsoleStats,
    LiveLight,
    MetricsHistory,
    ActivityEntry,
    ServerListItem,
    ServerLive,
  } from '#lib/types.ts';
  import Gauge from '../Gauge.svelte';
  import PowerControl from '../PowerControl.svelte';
  import StatusDot from '../StatusDot.svelte';
  import MetricsChart from '../MetricsChart.svelte';
  import PlayersChart from '../PlayersChart.svelte';
  import UptimeCard from '../UptimeCard.svelte';
  import Icon from '../Icon.svelte';

  let { tab }: { tab: ServerTab } = $props();
  const serverId = $derived(tab.serverId);
  const connId = $derived(tab.id);

  let live = $state<LiveLight | null>(null);
  /** `/live` : FlexCore, réseau, uptime — ce que `live/light` et le WebSocket ne donnent pas. */
  let liveNow = $state<ServerLive | null>(null);
  // Entrée de la liste des serveurs : porte l'adresse, mais aussi les métadonnées que seule la
  // liste fournit (avertissements de power action, dernier backup, référence support).
  let item = $state<ServerListItem | null>(null);
  /** Journal d'audit de l'hébergeur (panel web et staff compris), du plus récent au plus ancien. */
  let history = $state<ActivityEntry[]>([]);
  const address = $derived(item?.address ?? '');
  let copied = $state(false);
  let copiedTimer: ReturnType<typeof setTimeout> | undefined;
  let stats = $state<ConsoleStats | null>(null);
  let runState = $state('offline');
  let hibernated = $state(false);
  let metrics = $state<MetricsHistory | null>(null);
  let range = $state(3600);
  /** Dernier chargement (ms) : les longues fenêtres ne sont pas relues toutes les 15 s. */
  let metricsAt = 0;
  let metricsTimer: ReturnType<typeof setInterval> | undefined;
  let perfTimer: ReturnType<typeof setTimeout> | undefined;
  let perfBusy = $state(false);
  const unlisteners: UnlistenFn[] = [];

  const caps = $derived(serverCaps(serverId));
  const openable = $derived(availableViews(caps).filter((v) => v.id !== 'overview'));
  /** Mises à jour de mods/plugins/datapacks (vérification partagée avec la Marketplace). */
  const updates = $derived(updateCount(serverId));
  $effect(() => {
    const id = serverId;
    // `untrack` : l'effet dépend des capacités, pas du store qu'il alimente.
    if (caps?.mods === 'minecraft') untrack(() => void loadUpdates(id));
  });
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
    void loadHistory();
    void connect();
    void loadMetrics();
    void loadNow();
    metricsTimer = setInterval(() => {
      void loadMetrics();
      void loadNow();
    }, 15000);
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

  async function loadNow() {
    try {
      liveNow = await api.serverLive(serverId);
    } catch {
      /* endpoint indisponible : les faits concernés restent masqués */
    }
  }

  async function loadAddress() {
    try {
      const list = await api.listServers();
      item = list.servers.find((s) => s.id === serverId) ?? null;
    } catch {
      /* liste indisponible */
    }
  }

  /**
   * Journal d'audit via son endpoint dédié : il nomme l'auteur de chaque action et porte le
   * contexte (la commande tapée, par exemple), là où le `history[]` du détail serveur se
   * limitait à date/ip/staff.
   */
  async function loadHistory() {
    try {
      history = (await api.serverActivity(serverId, 8, 0)).items;
    } catch {
      history = [];
    }
  }

  /** Libellé lisible d'une action ; les identifiants inconnus sont affichés tels quels. */
  function actionLabel(a: string): string {
    const k = `action.${a}`;
    return t(k) === k ? a : t(k);
  }

  /** Auteur nommé quand l'API le connaît, sinon repli sur staff/compte. */
  function actorLabel(h: ActivityEntry): string {
    if (h.actor) return h.staff ? `${h.actor} (${t('overview.byStaff')})` : h.actor;
    return h.staff ? t('overview.byStaff') : t('overview.actorAccount');
  }

  function copyAddress() {
    if (!address) return;
    void navigator.clipboard.writeText(address).catch(() => {});
    copied = true;
    clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => (copied = false), 1500);
  }

  /**
   * Fenêtres de l'historique (libellé ou clé i18n, traduite au rendu) ; au-delà de 24 h,
   * l'hébergeur fournit l'essentiel des points.
   */
  const RANGES: { secs: number; label?: string; key?: string }[] = [
    { secs: 3600, label: '1h' },
    { secs: 6 * 3600, label: '6h' },
    { secs: 86400, label: '24h' },
    { secs: 7 * 86400, key: 'overview.range7d' },
    { secs: 30 * 86400, key: 'overview.range30d' },
  ];

  async function loadMetrics(force = false) {
    // 7 j / 30 j : une minute de plus ou de moins ne se voit pas, inutile de relire toutes les 15 s.
    if (!force && range > 86400 && Date.now() - metricsAt < 60_000) return;
    const asked = range;
    try {
      const h = await api.metricsHistory(serverId, asked, 600);
      if (asked !== range) return; // réponse périmée (fenêtre changée entre-temps)
      metrics = h;
      metricsAt = Date.now();
    } catch {
      /* historique indisponible */
    }
  }

  function setRange(secs: number) {
    range = secs;
    void loadMetrics(true);
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
    <PowerControl
      {serverId}
      {running}
      disabled={hibernated}
      restartWarning={item?.restart_warning}
      stopWarning={item?.stop_warning}
    />
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

  {#if updates > 0}
    <button class="note upd" onclick={() => open('mods')}>
      <Icon name="package" size={15} />
      <span>{t('overview.updatesAvailable', { count: updates })}</span>
      <span class="upd-go">{t('overview.updatesOpen')} →</span>
    </button>
  {/if}

  {#if item?.last_backup_url || item?.support_ref}
    <div class="srvmeta dim">
      {#if item?.last_backup_url}
        <!-- URL signée : on l'ouvre sans jamais l'afficher (jeton porteur). -->
        <button class="linklike" onclick={() => void openUrl(item!.last_backup_url!)}>
          <Icon name="download" size={13} />
          {t('overview.downloadLastBackup')}
        </button>
      {/if}
      {#if item?.support_ref}
        <span class="selectable">{t('overview.supportRef', { ref: item.support_ref })}</span>
      {/if}
    </div>
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
    {#if !caps || caps.players}
      <div class="fact">
        <span class="k">{t('overview.players')}</span>
        <span class="v">{live?.players ? `${live.players.current} / ${live.players.limit}` : '—'}</span>
      </div>
    {/if}
    <div class="fact">
      <span class="k">{t('overview.version')}</span>
      <span class="v">{live?.version ?? '—'}</span>
    </div>
    <div class="fact">
      <span class="k">{t('overview.uptime')}</span>
      <span class="v">
        {#if stats && stats.uptime > 0}
          {fmtUptime(stats.uptime)}
        {:else if liveNow?.stats.uptime}
          {fmtUptime(liveNow.stats.uptime.total_seconds * 1000)}
        {:else}
          —
        {/if}
      </span>
    </div>
    <!-- FlexCore : seulement pour une offre qui en comporte. -->
    {#if liveNow?.stats.cpu && liveNow.stats.cpu.flexcore > 0}
      <div class="fact" title={t('overview.flexcoreHint')}>
        <span class="k">{t('overview.flexcore')}</span>
        <span class="v" class:burst={liveNow.stats.cpu.is_bursting}>
          {liveNow.stats.cpu.is_bursting ? t('overview.flexcoreOn') : t('overview.flexcoreOff')}
        </span>
      </div>
    {/if}
    {#if liveNow?.stats.network}
      <div class="fact" title={t('overview.networkHint')}>
        <span class="k">{t('overview.network')}</span>
        <span class="v small">
          ↓ {fmtBytes(liveNow.stats.network.rx_bytes)} · ↑ {fmtBytes(liveNow.stats.network.tx_bytes)}
        </span>
      </div>
    {/if}
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
          {#each RANGES as r (r.secs)}
            <button class="rg" class:on={range === r.secs} onclick={() => setRange(r.secs)}>
              {r.key ? t(r.key) : r.label}
            </button>
          {/each}
        </div>
      </div>
    </div>
    <div class="card chart-card">
      <MetricsChart history={metrics} cpuLimit={live?.cpu.limit ?? 0} />
      {#if metrics && metrics.players.length > 0 && (!caps || caps.players)}
        <div class="players-chart">
          <PlayersChart
            players={metrics.players}
            from={metrics.from}
            to={metrics.to}
            stepS={metrics.step_s}
          />
        </div>
      {/if}
    </div>
  </section>

  <!-- Disponibilité 24 h / 7 j / 30 j et coupures : surveillance locale, complétée par les
       mesures de l'hébergeur là où rien ne surveillait. -->
  <UptimeCard {serverId} />

  {#if history.length > 0}
    <section class="block">
      <h2>{t('overview.activity')}</h2>
      <div class="acts">
        {#each history as h (h.id)}
          <div class="act">
            <span class="a-date dim">{h.date.slice(0, 16)}</span>
            <span class="a-what">
              {actionLabel(h.action)}
              {#if h.context}<code class="a-ctx selectable">{h.context}</code>{/if}
              {#if h.result && h.result !== 'success'}<span class="a-ko">{h.result}</span>{/if}
            </span>
            <span class="a-who dim" class:staff={h.staff}>{actorLabel(h)}</span>
            {#if h.ip}<span class="a-ip dim selectable">{h.ip}</span>{/if}
          </div>
        {/each}
      </div>
    </section>
  {/if}

  <section class="block">
    <h2>{t('overview.openView')}</h2>
    <div class="views">
      {#each openable as v (v.id)}
        <div class="vcard">
          <button class="vmain" onclick={() => open(v.id)}>
            <span class="vg"><Icon name={v.icon} size={17} /></span>
            <span class="vl">{t(`view.${v.id}`)}</span>
            {#if v.id === 'mods' && updates > 0}
              <span class="ubadge" title={t('overview.updatesAvailable', { count: updates })}>{updates}</span>
            {/if}
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
  .note.upd {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    font: inherit;
    font-size: 13.5px;
    text-align: left;
    cursor: pointer;
    color: var(--text);
    border-color: color-mix(in srgb, var(--brand-primary) 45%, var(--border));
    background: color-mix(in srgb, var(--brand-primary) 10%, transparent);
  }
  .note.upd span:first-of-type {
    flex: 1;
  }
  .upd-go {
    font-weight: 600;
    color: var(--brand-primary);
  }
  .ubadge {
    margin-left: auto;
    min-width: 18px;
    padding: 0 6px;
    border-radius: 999px;
    background: var(--brand-primary);
    color: #fff;
    font-size: 11px;
    font-weight: 700;
    line-height: 18px;
    text-align: center;
  }
  .acts {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .act {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 8px;
    border-radius: 6px;
    font-size: 13px;
  }
  .act:nth-child(odd) {
    background: color-mix(in srgb, var(--text) 3%, transparent);
  }
  .a-date {
    font-variant-numeric: tabular-nums;
    flex: none;
  }
  .a-what {
    display: flex;
    align-items: baseline;
    gap: 8px;
    flex: 1;
    min-width: 0;
  }
  .a-ctx {
    padding: 1px 6px;
    border-radius: 5px;
    background: color-mix(in srgb, var(--text) 7%, transparent);
    font-family: var(--font-mono);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .a-ko {
    padding: 1px 6px;
    border-radius: 999px;
    font-size: 11px;
    background: color-mix(in srgb, var(--state-danger) 18%, transparent);
    color: var(--state-danger);
  }
  .a-who {
    flex: none;
    max-width: 18ch;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .a-who.staff {
    color: var(--brand-tertiary);
  }
  .a-ip {
    font-variant-numeric: tabular-nums;
    flex: none;
  }

  .linklike {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 0;
    border: 0;
    background: none;
    color: var(--brand-primary);
    font: inherit;
    cursor: pointer;
  }
  .linklike:hover {
    text-decoration: underline;
  }

  .srvmeta {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 18px;
    font-size: 12px;
  }

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
  .fact .v.small {
    font-size: 14px;
  }
  .fact .v.burst {
    color: var(--state-pending);
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
  .players-chart {
    margin-top: 12px;
    padding-top: 10px;
    border-top: 1px solid var(--border);
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
