<script lang="ts">
  import { onMount, onDestroy, untrack } from 'svelte';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { api } from '#lib/ipc.ts';
  import { consoleEvents } from '#lib/events.ts';
  import { t } from '#lib/i18n/index.ts';
  import { runtimeMeta, isRunning } from '#lib/status.ts';
  import { fmtBytes } from '#lib/copilot/format.ts';
  import { fmtDuration } from '#lib/automations/format.ts';
  import { tabs, type ServerTab, type ServerView } from '#lib/tabs/tabs.svelte.ts';
  import { serverCaps, availableViews } from '#lib/games/capabilities.svelte.ts';
  import { loadUpdates, updateCount } from '#lib/mods/updates.svelte.ts';
  import { requestDiskView } from '#lib/sftp/diskRequest.svelte.ts';
  import { avatarStyle, BIOMES, biomeVars, serverBiome } from '#lib/servers/colors.svelte.ts';
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
  import BiomeLandscape from '../BiomeLandscape.svelte';
  import Icon from '../Icon.svelte';

  let { tab }: { tab: ServerTab } = $props();
  const serverId = $derived(tab.serverId);
  const connId = $derived(tab.id);
  /** Biome du serveur : colore la bannière, les sélections et les tuiles de la vue. */
  const biome = $derived(serverBiome(serverId));

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

  /** Temps en service : WebSocket d'abord (à la seconde), sinon `/live` (relu toutes les 15 s). */
  const uptimeMs = $derived(
    stats && stats.uptime > 0
      ? stats.uptime
      : liveNow?.stats.uptime
        ? liveNow.stats.uptime.total_seconds * 1000
        : 0
  );
  /** Joueurs connectés : `/live` (relu) d'abord, sinon `live/light` (lu à l'ouverture). */
  const players = $derived(liveNow?.stats.players ?? live?.players ?? null);
  const flexcore = $derived(liveNow?.stats.cpu && liveNow.stats.cpu.flexcore > 0 ? liveNow.stats.cpu : null);
  const hasInfos = $derived(
    !!item?.last_backup_url || !!item?.support_ref || uptimeMs > 0 || !!flexcore || !!liveNow?.stats.network
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

  /** Pastille d'une entrée du journal : échec, staff, puis famille d'action. */
  function activityDot(h: ActivityEntry): string {
    if (h.result && h.result !== 'success') return 'var(--state-danger)';
    const accent = (key: string) => BIOMES.find((b) => b.key === key)?.accent ?? 'var(--text-dim)';
    if (h.staff) return accent('end');
    switch (h.category) {
      case 'power':
        return accent('ocean');
      case 'sftp':
        return accent('desert');
      case 'console':
        return accent('forest');
      default:
        return 'var(--text-dim)';
    }
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
    { secs: 3600, label: '1 h' },
    { secs: 6 * 3600, label: '6 h' },
    { secs: 86400, label: '24 h' },
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

  /** Ouvre l'onglet Fichiers sur l'analyse de l'espace disque. */
  function analyzeDisk() {
    requestDiskView(serverId);
    open('sftp');
  }

  function fmtUptime(ms: number): string {
    const s = Math.floor(ms / 1000);
    if (s >= 86400) return fmtDuration(s); // « 41 j 22 h »
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    if (h > 0) return `${h} h ${m} min`;
    return `${m} min`;
  }
</script>

<div class="ov biome" style={biomeVars(biome)}>
  <!-- Bannière : paysage du biome, nom, puces d'état, alimentation. -->
  <section class="banner" style="background: {biome.sky}">
    <div class="land"><BiomeLandscape {biome} seed={serverId} height={190} pixel={6} /></div>
    <div class="banner-body">
      <div class="who">
        <h1>{tab.serverName}</h1>
        <div class="chips">
          <span class="chip state" style="color: {rt.ink}">
            <StatusDot color={rt.color} pulse={running} size={8} />
            {t(`status.${rt.key}`)}
          </span>
          {#if live?.version}<span class="chip">{live.version}</span>{/if}
          {#if address}
            <button class="chip addr" onclick={copyAddress} title={t('overview.copyAddress')}>
              <span class="mono">{address}</span>
              {#if copied}
                <Icon name="check" size={13} stroke={2.4} /><span class="copied">{t('overview.copied')}</span>
              {:else}
                <Icon name="copy" size={13} stroke={2.2} />
              {/if}
            </button>
          {/if}
        </div>
      </div>
      <div class="bpower">
        <PowerControl
          {serverId}
          {running}
          tone="banner"
          disabled={hibernated}
          restartWarning={item?.restart_warning}
          stopWarning={item?.stop_warning}
        />
      </div>
    </div>
  </section>

  <!-- Bandeau d'infos : seulement ce que l'hébergeur fournit pour ce serveur. -->
  {#if hasInfos}
    <div class="infobar">
      {#if item?.last_backup_url}
        <!-- URL signée : on l'ouvre sans jamais l'afficher (jeton porteur). -->
        <button class="linklike" onclick={() => void openUrl(item!.last_backup_url!)}>
          <Icon name="download" size={14} stroke={2.2} />
          {t('overview.downloadLastBackup')}
        </button>
      {/if}
      {#if item?.support_ref}
        <span class="selectable">{t('overview.supportRef', { ref: item.support_ref })}</span>
      {/if}
      {#if uptimeMs > 0}
        <span>{t('overview.uptime')} <strong>{fmtUptime(uptimeMs)}</strong></span>
      {/if}
      <!-- FlexCore : seulement pour une offre qui en comporte. -->
      {#if flexcore}
        <span title={t('overview.flexcoreHint')}>
          {t('overview.flexcore')}
          <strong class:burst={flexcore.is_bursting}>
            {flexcore.is_bursting ? t('overview.flexcoreOn') : t('overview.flexcoreOff')}
          </strong>
        </span>
      {/if}
      {#if liveNow?.stats.network}
        <span title={t('overview.networkHint')}>
          {t('overview.network')}
          <strong class="mono">
            ↓ {fmtBytes(liveNow.stats.network.rx_bytes)} · ↑ {fmtBytes(liveNow.stats.network.tx_bytes)}
          </strong>
        </span>
      {/if}
    </div>
  {/if}

  <div class="body">
    {#if hibernated}
      <div class="note sleep"><span class="ndot"></span><span>{t('overview.hibernated')}</span></div>
    {/if}

    {#if updates > 0}
      <button class="note upd" onclick={() => open('mods')}>
        <span class="ndot"></span>
        <span class="ntext">{t('overview.updatesAvailable', { count: updates })}</span>
        <span class="ngo">{t('overview.updatesOpen')} →</span>
      </button>
    {/if}

    <!-- Ouvrir une vue : « + » ouvre un onglet de plus (doublons permis). -->
    <section class="views" aria-label={t('overview.openView')}>
      {#each openable as v (v.id)}
        <div class="vtile">
          <button class="vmain" onclick={() => open(v.id)}>
            <span class="vg"><Icon name={v.icon} size={19} stroke={2} /></span>
            <span class="vl">
              {t(`view.${v.id}`)}
              {#if v.id === 'mods' && updates > 0}
                <span class="ubadge" title={t('overview.updatesAvailable', { count: updates })}>{updates}</span>
              {/if}
              {#if !v.ready}<span class="soon">{t('common.comingSoon')}</span>{/if}
            </span>
          </button>
          <button
            class="vnew"
            title={t('overview.newTabHint')}
            aria-label={t('overview.newTab', { view: t(`view.${v.id}`) })}
            onclick={() => open(v.id, true)}>+</button
          >
        </div>
      {/each}
    </section>

    <div class="cols">
      <div class="col main">
        <section class="card hist">
          <div class="hist-head">
            <h2>{t('overview.history')}</h2>
            <button
              class="perfbtn"
              onclick={runPerf}
              disabled={perfBusy || !running}
              title={running ? t('overview.perfHint') : t('overview.perfOffline')}
            >
              <Icon name="zap" size={14} stroke={2.2} />
              {perfBusy ? t('overview.perfRunning') : t('overview.perf')}
            </button>
            <div class="ranges" role="group" aria-label={t('overview.range')}>
              {#each RANGES as r (r.secs)}
                <button class="rg" class:on={range === r.secs} aria-pressed={range === r.secs} onclick={() => setRange(r.secs)}>
                  {r.key ? t(r.key) : r.label}
                </button>
              {/each}
            </div>
          </div>
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
        </section>

        {#if history.length > 0}
          <section class="card acts">
            <h2>{t('overview.activity')}</h2>
            {#each history as h (h.id)}
              <div class="act">
                <span class="adot" style="background: {activityDot(h)}"></span>
                <span class="a-what">
                  <strong class:staff={h.staff}>{actorLabel(h)}</strong>
                  <span>{actionLabel(h.action)}</span>
                  {#if h.context}<code class="a-ctx selectable">{h.context}</code>{/if}
                  {#if h.result && h.result !== 'success'}<span class="a-ko">{h.result}</span>{/if}
                </span>
                {#if h.ip}<span class="a-ip selectable">{h.ip}</span>{/if}
                <span class="a-date">{h.date.slice(0, 16)}</span>
              </div>
            {/each}
          </section>
        {/if}
      </div>

      <div class="col side">
        <section class="card res">
          <h2>{t('overview.resources')}</h2>
          <Gauge
            label={t('overview.cpu')}
            percent={cpuPct}
            detail={stats ? `${stats.cpu_absolute.toFixed(1)} % / ${live?.cpu.limit ?? 0} %` : t('overview.noData')}
          />
          <Gauge
            label={t('overview.ram')}
            percent={ramPct}
            detail={stats ? `${fmtBytes(stats.memory_bytes)} / ${fmtBytes(stats.memory_limit_bytes)}` : '—'}
          />
          <Gauge
            label={t('overview.disk')}
            percent={diskPct}
            detail={stats ? `${fmtBytes(stats.disk_bytes)} / ${fmtBytes(diskLimitBytes)}` : '—'}
          />
          {#if openable.some((v) => v.id === 'sftp')}
            <button class="linklike" onclick={analyzeDisk}>
              <Icon name="hard-drive" size={14} stroke={2.2} />
              {t('overview.analyzeDisk')}
            </button>
          {/if}
        </section>

        <!-- Joueurs : seulement quand l'hébergeur en remonte pour ce jeu. -->
        {#if players && (!caps || caps.players)}
          <section class="card pl">
            <div class="pl-head">
              <h2>{t('overview.players')}</h2>
              <span class="mono pl-count">{players.current} / {players.limit}</span>
            </div>
            {#if players.list.length > 0}
              <div class="pl-list">
                {#each players.list.slice(0, 8) as name (name)}
                  <span class="phead" style={avatarStyle(name)} title={name}>{name.charAt(0).toUpperCase()}</span>
                {/each}
                {#if players.list.length > 8}<span class="pl-more">+{players.list.length - 8}</span>{/if}
              </div>
              <p class="pl-names selectable">{players.list.join(', ')}</p>
            {:else if players.current === 0}
              <p class="pl-names">{t('players.none')}</p>
            {/if}
            {#if openable.some((v) => v.id === 'players')}
              <button class="linklike" onclick={() => open('players')}>
                <Icon name="users" size={14} stroke={2.2} />
                {t('overview.openPlayers')}
              </button>
            {/if}
          </section>
        {/if}

        <!-- Disponibilité 24 h / 7 j / 30 j et coupures : surveillance locale, complétée par les
             mesures de l'hébergeur là où rien ne surveillait. -->
        <UptimeCard {serverId} />
      </div>
    </div>
  </div>
</div>

<style>
  .ov {
    min-height: 100%;
    display: flex;
    flex-direction: column;
  }

  /* --- Bannière --------------------------------------------------------------- */
  /* Pas d'overflow caché : le menu d'alimentation déborde sous la bannière (le paysage, lui,
     est rogné par son propre conteneur). */
  .banner {
    position: relative;
    min-height: 190px;
    display: flex;
    align-items: flex-end;
    flex: none;
  }
  .land {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
  }
  /* Sur le paysage, tout garde les couleurs du thème sombre (les paysages le sont toujours). */
  .banner-body {
    position: relative;
    flex: 1;
    min-width: 0;
    display: flex;
    flex-wrap: wrap;
    align-items: flex-end;
    gap: 16px;
    padding: 28px 40px 22px;
  }
  .who {
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
  }
  h1 {
    margin: 0;
    font-size: 48px;
    line-height: 0.95;
    color: #ffffff;
    overflow-wrap: anywhere;
  }
  .chips {
    --state-running: #5fd68c;
    --state-running-text: #8fe8b0;
    --state-pending: #f2b84b;
    --state-offline: #6b7a7f;
    --state-hibernate: #c79bff;
    --text-dim: #8c9a94;
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    padding: 5px 11px;
    border-radius: 999px;
    border: none;
    background: #0d1113;
    color: #e4ebe8;
    font: inherit;
    font-weight: 800;
    font-size: 13px;
    line-height: 1.35;
  }
  .chip.addr {
    cursor: pointer;
    color: #e4ebe8;
  }
  .chip.addr .mono {
    font-weight: 500;
    font-size: 12.5px;
  }
  .chip.addr :global(.icon) {
    color: #b3bfba;
  }
  .chip.addr:hover {
    background: #1b2126;
  }
  .copied {
    color: #8fe8b0;
  }
  .bpower {
    margin-left: auto;
  }

  /* --- Bandeau d'infos --------------------------------------------------------- */
  .infobar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 22px;
    padding: 10px 40px;
    background: var(--inset);
    border-bottom: 1px solid var(--border);
    font-size: 14px;
    color: var(--text-muted);
  }
  .infobar strong {
    color: var(--text);
  }
  .infobar strong.mono {
    font-weight: 500;
    font-size: 13px;
  }
  .infobar strong.burst {
    color: var(--state-pending);
  }
  .linklike {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    padding: 0;
    border: 0;
    background: none;
    color: var(--brand-primary);
    font: inherit;
    font-weight: 800;
    font-size: 14px;
    cursor: pointer;
    align-self: flex-start;
  }
  .linklike:hover {
    text-decoration: underline;
  }

  /* --- Corps --------------------------------------------------------------------- */
  .body {
    flex: 1;
    padding: 22px 40px 40px;
    display: flex;
    flex-direction: column;
    gap: 18px;
  }
  h2 {
    margin: 0;
    font-size: 18px;
  }

  .note {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 16px;
    border-radius: 12px;
    border: none;
    font: inherit;
    font-size: 14.5px;
    text-align: left;
    color: var(--text);
  }
  .ndot {
    width: 10px;
    height: 10px;
    border-radius: 3px;
    flex: none;
  }
  .note.sleep {
    background: color-mix(in srgb, var(--state-hibernate) 14%, var(--surface));
  }
  .note.sleep .ndot {
    background: var(--state-hibernate);
  }
  .note.upd {
    width: 100%;
    cursor: pointer;
    background: color-mix(in srgb, var(--state-pending) 14%, var(--surface));
  }
  .note.upd:hover {
    background: color-mix(in srgb, var(--state-pending) 20%, var(--surface));
  }
  .note.upd .ndot {
    background: var(--state-pending);
  }
  .ntext {
    flex: 1;
  }
  .ngo {
    font-weight: 900;
    color: var(--state-pending);
    white-space: nowrap;
  }

  /* Tuiles « Ouvrir une vue » */
  .views {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(min(150px, 100%), 1fr));
    gap: 10px;
  }
  .vtile {
    position: relative;
    display: flex;
  }
  .vmain {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 10px;
    padding: 14px 14px 13px;
    border-radius: 14px;
    border: 1px solid var(--border);
    background: var(--surface);
    color: var(--text);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .vmain:hover {
    border-color: color-mix(in srgb, var(--accent) 55%, var(--border));
  }
  .vg {
    width: 36px;
    height: 36px;
    border-radius: 10px;
    display: grid;
    place-items: center;
    background: var(--accent-soft);
    color: var(--accent-text);
  }
  .vl {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
    font-weight: 800;
    font-size: 15px;
  }
  .ubadge {
    padding: 0 7px;
    border-radius: 9px;
    background: var(--state-pending);
    color: var(--bg);
    font-size: 12px;
    font-weight: 900;
    line-height: 18px;
  }
  .soon {
    font-size: 10.5px;
    font-weight: 800;
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 0 7px;
  }
  .vnew {
    position: absolute;
    top: 8px;
    right: 8px;
    width: 26px;
    height: 26px;
    border-radius: 7px;
    border: none;
    background: var(--border);
    color: var(--text-muted);
    font: inherit;
    font-weight: 900;
    cursor: pointer;
  }
  .vnew:hover {
    background: var(--accent);
    color: var(--on-accent);
  }

  /* Deux colonnes : historique et activité, puis ressources, joueurs, disponibilité. */
  .cols {
    display: flex;
    flex-wrap: wrap;
    gap: 18px;
    align-items: flex-start;
  }
  .col {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 18px;
  }
  .col.main {
    flex: 2 1 560px;
  }
  .col.side {
    flex: 1 1 300px;
  }
  .card {
    padding: 20px 22px;
  }

  .hist {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .hist-head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 10px;
  }
  .hist-head h2 {
    margin-right: auto;
  }
  .perfbtn {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 32px;
    padding: 0 12px;
    border-radius: 8px;
    border: none;
    background: var(--accent-soft);
    color: var(--accent-text);
    font: inherit;
    font-weight: 800;
    font-size: 13.5px;
    cursor: pointer;
    white-space: nowrap;
  }
  .perfbtn:hover:not(:disabled) {
    background: color-mix(in srgb, var(--accent) 18%, var(--accent-soft));
  }
  .perfbtn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  /* Période : la sélection prend l'accent du biome. */
  .ranges {
    display: flex;
    gap: 2px;
    padding: 3px;
    border-radius: 9px;
    background: var(--bg);
  }
  .rg {
    height: 28px;
    padding: 0 10px;
    border-radius: 7px;
    border: none;
    background: transparent;
    color: var(--text-muted);
    font: inherit;
    font-weight: 800;
    font-size: 13px;
    cursor: pointer;
    white-space: nowrap;
  }
  .rg:hover:not(.on) {
    color: var(--text);
  }
  .rg.on {
    background: var(--accent);
    color: var(--on-accent);
  }
  .players-chart {
    padding-top: 12px;
    border-top: 1px solid var(--border-strong);
  }

  /* Activité récente */
  .acts {
    display: flex;
    flex-direction: column;
  }
  .acts h2 {
    margin-bottom: 10px;
  }
  .act {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 9px 0;
    border-top: 1px solid var(--border);
    font-size: 14px;
  }
  .adot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex: none;
  }
  .a-what {
    display: flex;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 2px 7px;
    flex: 1;
    min-width: 0;
  }
  .a-what strong.staff {
    color: var(--state-hibernate);
  }
  .a-ctx {
    max-width: 100%;
    padding: 1px 6px;
    border-radius: 5px;
    background: var(--inset);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .a-ko {
    padding: 0 7px;
    border-radius: 999px;
    font-size: 11.5px;
    font-weight: 800;
    background: var(--danger-bg);
    color: var(--danger-ink);
  }
  .a-ip,
  .a-date {
    flex: none;
    font-size: 12.5px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }
  .a-ip {
    font-family: var(--font-mono);
    font-size: 12px;
  }

  /* Ressources */
  .res {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  /* Joueurs */
  .pl {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .pl-head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
  }
  .pl-count {
    font-size: 13px;
    color: var(--text-muted);
  }
  .pl-list {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
  }
  .phead {
    width: 34px;
    height: 34px;
    border-radius: 8px;
    display: grid;
    place-items: center;
    font-weight: 900;
    font-size: 15px;
  }
  .pl-more {
    font-size: 13px;
    color: var(--text-muted);
  }
  .pl-names {
    margin: 0;
    font-size: 14px;
    color: var(--text-muted);
  }

  @media (max-width: 760px) {
    .banner-body,
    .infobar,
    .body {
      padding-left: 16px;
      padding-right: 16px;
    }
    h1 {
      font-size: 36px;
    }
  }
</style>
