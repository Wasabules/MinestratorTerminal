<script lang="ts">
  import { untrack } from "svelte";
  import { api, humanizeError } from "../../ipc";
  import { t } from "../../i18n";
  import Icon from "../Icon.svelte";
  import MetricsCard from "../MetricsCard.svelte";
  import { alerts } from "../../stores/alerts.svelte";
  import { loadUpdates, updateCount } from "../../stores/updates.svelte";
  import type {
    ConsoleStats,
    LiveLight,
    MyBoxSummary,
    PowerAction,
    ActivityEntry,
    ServerListItem,
  } from "../../types";

  let {
    server,
    mybox,
    active,
    onNavigate,
  }: {
    server: ServerListItem;
    mybox: MyBoxSummary | null;
    active: boolean;
    /** Ouvre un autre onglet du serveur (la tuile Joueurs y renvoie). */
    onNavigate?: (tab: string) => void;
  } = $props();

  /** MOTD replié par défaut : décoratif, il mangeait le haut de l'écran. */
  let motdOpen = $state(false);

  const serverId = $derived(server.id);

  let live = $state<LiveLight | null>(null);
  let stats = $state<ConsoleStats | null>(null);
  let error = $state<string | null>(null);
  let busy = $state<PowerAction | null>(null);
  let copied = $state(false);
  /** Journal d'audit de l'hébergeur (couvre le panel web et le staff), endpoint dédié. */
  let history = $state<ActivityEntry[]>([]);
  /** Action retenue tant que l'avertissement de l'hébergeur n'est pas confirmé. */
  let pending = $state<PowerAction | null>(null);

  /** Avertissement configuré par l'hébergeur pour cette action, sinon `null`. */
  function warningFor(action: PowerAction): string | null {
    if (action === "restart") return server.restart_warning || null;
    if (action === "stop" || action === "kill") return server.stop_warning || null;
    return null;
  }

  /** Demande confirmation si l'hébergeur a posé un avertissement, sinon exécute directement. */
  function request(action: PowerAction) {
    if (warningFor(action)) {
      pending = action;
      return;
    }
    void power(action);
  }

  /**
   * Libellé lisible d'une action. L'API renvoie des identifiants pointés (`power.restart`) ;
   * ceux qu'on ne connaît pas sont affichés tels quels plutôt que masqués.
   */
  function actionLabel(a: string): string {
    const k = `action.${a}`;
    return t(k) !== k ? t(k) : a;
  }

  function actionIcon(a: string): string {
    if (a.startsWith("console.")) return "console";
    if (a.startsWith("sftp.") || a.startsWith("file")) return "files";
    if (a.startsWith("power.")) {
      const verb = a.slice(6);
      if (verb === "start") return "play";
      if (verb === "stop") return "stop";
      if (verb === "kill") return "kill";
      return "restart";
    }
    return "shield";
  }

  /**
   * Qui a fait l'action. L'endpoint `/activity` nomme l'auteur (`actor_name`), contrairement au
   * `history[]` du détail serveur. Repli sur staff/compte quand le nom manque.
   */
  function actorLabel(h: ActivityEntry): string {
    if (h.actor) return h.staff ? `${h.actor} (${t("overview.actorStaff")})` : h.actor;
    return h.staff ? t("overview.actorStaff") : t("overview.actorAccount");
  }

  /** `2026-08-12 23:29:20.515` → `12/08 23:29`. */
  function shortDate(d: string): string {
    const m = d.match(/^(\d{4})-(\d{2})-(\d{2})[ T](\d{2}:\d{2})/);
    return m ? `${m[3]}/${m[2]} ${m[4]}` : d.slice(0, 16);
  }

  /** Journal d'audit : la vue n'en montre qu'une poignée, inutile d'en demander plus. */
  async function loadHistory() {
    try {
      history = (await api.serverActivity(serverId, 8, 0)).items;
    } catch {
      history = [];
    }
  }

  /**
   * Partage l'adresse via la feuille de partage Android (`navigator.share`), pour l'envoyer à
   * ses joueurs. Repli silencieux sur la copie si l'API n'est pas disponible ou si l'utilisateur
   * annule — `share()` rejette aussi dans ce cas, ce qui n'est pas une erreur.
   */
  async function shareAddr() {
    const data = { title: server.name, text: server.address };
    try {
      if (navigator.share) {
        await navigator.share(data);
        return;
      }
    } catch {
      return; // annulation utilisateur : ne pas enchaîner sur une copie surprise
    }
    void copyAddr();
  }

  async function copyAddr() {
    try {
      await navigator.clipboard.writeText(server.address);
      copied = true;
      setTimeout(() => (copied = false), 1200);
    } catch {
      /* presse-papiers indispo : le texte reste sélectionnable */
    }
  }

  async function refreshLive() {
    try {
      live = await api.liveLight(serverId);
      error = null;
    } catch (err) {
      error = humanizeError(err);
    }
  }
  async function refreshStats() {
    try {
      stats = await api.sampleStats(serverId);
    } catch {
      stats = null;
    }
  }

  async function power(action: PowerAction) {
    if (busy) return;
    pending = null;
    busy = action;
    try {
      await api.powerAction(serverId, action);
      setTimeout(() => {
        refreshLive();
        refreshStats();
      }, 1500);
    } catch (err) {
      error = humanizeError(err);
    } finally {
      busy = null;
    }
  }

  /**
   * État d'exécution. `GET /server/{id}/live/light` renvoie `status: null` (vérifié le
   * 2026-08-19) : la seule source fiable est l'échantillon WebSocket (`stats.state`), d'où
   * l'ordre. `live.status` reste en second au cas où l'API se mette à le renseigner.
   */
  const runState = $derived(stats?.state ?? live?.status ?? null);
  const isRunning = $derived(runState === "running");
  /** État connu ? Si non, on n'ose pas désactiver de bouton (permissif plutôt que bloquant). */
  const stateKnown = $derived(runState !== null);

  function statusColor(s: string | null): string {
    switch (s) {
      case "running":
        return "var(--state-running)";
      case "starting":
      case "stopping":
      case "installing":
        return "var(--state-pending)";
      default:
        return "var(--state-offline)";
    }
  }
  function statusLabel(s: string | null): string {
    const k = `status.${s ?? "unknown"}`;
    return t(k) !== k ? t(k) : t("status.unknown");
  }
  function gaugeColor(pct: number): string {
    if (pct >= 90) return "var(--state-danger)";
    if (pct >= 70) return "var(--state-pending)";
    return "var(--state-running)";
  }
  function fmtBytes(n: number): string {
    if (n <= 0) return "0";
    const gb = n / 1024 ** 3;
    return gb >= 1 ? `${gb.toFixed(1)} Go` : `${Math.round(n / 1024 ** 2)} Mo`;
  }
  function fmtUptime(ms: number | undefined): string {
    if (!ms || ms <= 0) return "—";
    const s = Math.floor(ms / 1000);
    const d = Math.floor(s / 86400);
    const h = Math.floor((s % 86400) / 3600);
    const m = Math.floor((s % 3600) / 60);
    if (d) return `${d}j ${h}h`;
    if (h) return `${h}h ${m}m`;
    return `${m}m`;
  }
  const clamp = (n: number) => Math.max(0, Math.min(100, n));

  const cpuLimit = $derived(live ? Math.max(live.cpu.dedicated + live.cpu.flexcore, 100) : 100);
  const cpuVal = $derived(stats?.cpu_absolute ?? 0);
  const memUsed = $derived(stats?.memory_bytes ?? 0);
  const memLimit = $derived(
    stats?.memory_limit_bytes && stats.memory_limit_bytes > 0
      ? stats.memory_limit_bytes
      : (live?.memory.limit ?? 0) * 1024 ** 2,
  );
  const diskUsed = $derived(stats?.disk_bytes ?? 0);
  const diskLimit = $derived((live?.disk.limit ?? 0) * 1024 ** 2);

  /** Mises à jour de mods/plugins/datapacks (vérification partagée avec l'onglet Mods). */
  const updates = $derived(updateCount(serverId));
  $effect(() => {
    const id = serverId;
    // `untrack` : l'effet dépend de l'onglet actif, pas du store qu'il alimente.
    if (active && server.capabilities?.mods === "minecraft") untrack(() => void loadUpdates(id));
  });

  /** Dernières alertes du superviseur pour ce serveur (journal local, cf. écran Alertes). */
  const serverAlerts = $derived(alerts.forServer(serverId).slice(0, 3));

  function alertColor(severity: string): string {
    if (severity === "critical") return "var(--state-danger)";
    return severity === "info" ? "var(--state-running)" : "var(--state-pending)";
  }

  /** Secondes Unix → `12/08 23:29` (même forme que le journal d'activité). */
  function alertDate(ts: number): string {
    const d = new Date(ts * 1000);
    const p = (n: number) => String(n).padStart(2, "0");
    return `${p(d.getDate())}/${p(d.getMonth() + 1)} ${p(d.getHours())}:${p(d.getMinutes())}`;
  }

  const gauges = $derived([
    { key: "overview.cpu", pct: clamp((cpuVal / cpuLimit) * 100), value: `${cpuVal.toFixed(0)}%` },
    {
      key: "overview.ram",
      pct: memLimit > 0 ? clamp((memUsed / memLimit) * 100) : 0,
      value: memLimit > 0 ? `${fmtBytes(memUsed)} / ${fmtBytes(memLimit)}` : "—",
    },
    {
      key: "overview.disk",
      pct: diskLimit > 0 ? clamp((diskUsed / diskLimit) * 100) : 0,
      value: diskLimit > 0 ? `${fmtBytes(diskUsed)} / ${fmtBytes(diskLimit)}` : "—",
    },
  ]);

  /**
   * Trois boutons sur une ligne. Démarrer et Arrêter s'excluent — les afficher ensemble
   * proposerait toujours une action sans objet — donc le premier emplacement est contextuel.
   * Redémarrer et Kill n'ont de sens que sur un serveur en marche : désactivés sinon, sauf
   * si l'état est inconnu (on ne bloque pas sur une incertitude).
   */
  const powerButtons = $derived<
    { action: PowerAction; key: string; icon: string; kind: string; off: boolean }[]
  >([
    isRunning
      ? { action: "stop", key: "power.stop", icon: "stop", kind: "warn", off: false }
      : { action: "start", key: "power.start", icon: "play", kind: "ok", off: false },
    {
      action: "restart",
      key: "power.restart",
      icon: "restart",
      kind: "warn",
      off: stateKnown && !isRunning,
    },
    {
      action: "kill",
      key: "power.kill",
      icon: "kill",
      kind: "danger",
      off: stateKnown && !isRunning,
    },
  ]);

  // Poll seulement quand l'onglet est actif (pager).
  $effect(() => {
    if (!active) return;
    refreshLive();
    refreshStats();
    loadHistory();
    const a = setInterval(refreshLive, 8000);
    const b = setInterval(refreshStats, 4000);
    return () => {
      clearInterval(a);
      clearInterval(b);
    };
  });
</script>

<div class="view">
  {#if error}<p class="err selectable">{error}</p>{/if}

  {#if updates > 0}
    <button class="upd-note" onclick={() => onNavigate?.("market")}>
      <Icon name="package" size={16} />
      <span>{t("overview.updates").replace("{count}", String(updates))}</span>
      <Icon name="chevronRight" size={16} />
    </button>
  {/if}

  <!-- Bloc de tête : état, offre et jauges d'un seul coup d'œil. -->
  <section class="head-card">
    <div class="status">
      <span class="dot" class:pulse={isRunning} style="background:{statusColor(runState)}"></span>
      <span class="slabel">{statusLabel(runState)}</span>
      {#if mybox}
        <span class="pill selectable">{mybox.offer} · {mybox.days_left} j</span>
      {/if}
    </div>

    <div class="gauges">
      {#each gauges as g (g.key)}
        <div class="gauge">
          <div class="grow">
            <span class="gname">{t(g.key)}</span>
            <span class="gval selectable">{g.value}</span>
          </div>
          <div class="track"><div class="fill" style="width:{g.pct}%; background:{gaugeColor(g.pct)}"></div></div>
        </div>
      {/each}
    </div>
  </section>

  <!-- Alimentation : à sa place sous les jauges, puis collée en HAUT dès qu'on défile plus
       bas. `sticky` dans `.page` (le conteneur scrollable) plutôt que `fixed`, sinon la barre
       s'afficherait aussi par-dessus les autres onglets du pager. -->
  <div class="power sticky">
    {#each powerButtons as b (b.action)}
      <button
        class={b.kind}
        disabled={busy !== null || b.off}
        onclick={() => request(b.action)}
      >
        <Icon name={b.icon} size={18} />
        <span>{busy === b.action ? "…" : t(b.key)}</span>
      </button>
    {/each}
  </div>

  <!-- Connexion : l'info qu'on vient chercher le plus souvent après l'état. -->
  <div class="conn">
    <div class="conncol">
      <small>Connexion</small>
      <span class="addr selectable">{server.address}</span>
    </div>
    <button class="copybtn" onclick={shareAddr} aria-label={t("overview.share")}>
      <Icon name="external" size={18} />
    </button>
    <button class="copybtn" onclick={copyAddr} aria-label="Copier">
      <Icon name={copied ? "check" : "copy"} size={18} />
    </button>
  </div>

  <!-- Trois tuiles : « Jeu » a disparu, l'en-tête porte déjà l'icône et le nom du jeu. -->
  <div class="tiles">
    <div class="tile">
      <small>{t("overview.version")}</small>
      <strong class="selectable">{live?.version ?? "—"}</strong>
    </div>
    <button class="tile as-btn" onclick={() => onNavigate?.("players")}>
      <small>{t("overview.players")}</small>
      <strong>{live?.players ? `${live.players.current}/${live.players.limit}` : "—"}</strong>
    </button>
    <div class="tile">
      <small>{t("overview.uptime")}</small>
      <strong class="selectable">{fmtUptime(stats?.uptime)}</strong>
    </div>
  </div>

  {#if live?.players && live.players.list.length > 0}
    <div class="online">
      <small>{t("players.online")}</small>
      <div class="chips">
        {#each live.players.list.slice(0, 12) as p (p)}
          <span class="chip selectable">{p}</span>
        {/each}
      </div>
    </div>
  {/if}

  <MetricsCard {serverId} {active} {cpuLimit} {diskLimit} />

  {#if serverAlerts.length > 0}
    <section class="acts">
      <h3>{t("alerts.recent")}</h3>
      <ul>
        {#each serverAlerts as a (a.id)}
          <li class="act">
            <span class="a-dot" style="background:{alertColor(a.severity)}"></span>
            <span class="a-body">
              <span class="a-top">
                <strong>{t(`alerts.kind.${a.kind}`) !== `alerts.kind.${a.kind}` ? t(`alerts.kind.${a.kind}`) : a.kind}</strong>
                <span class="a-date dim">{alertDate(a.ts)}</span>
              </span>
              <span class="a-msg selectable">{a.message}</span>
            </span>
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  {#if live?.motd}
    <div class="motdbox">
      <button class="motdtoggle" onclick={() => (motdOpen = !motdOpen)}>
        {t("overview.motd")}
        <span class="caret" class:open={motdOpen}>›</span>
      </button>
      {#if motdOpen}<p class="motd selectable">{live.motd}</p>{/if}
    </div>
  {/if}

  {#if pending}
    <div class="warnbox">
      <p class="warntext">{warningFor(pending)}</p>
      <div class="warnact">
        <button class="ghost" onclick={() => (pending = null)}>{t("backups.cancel")}</button>
        <button class="danger" onclick={() => power(pending!)}>{t("overview.confirm")}</button>
      </div>
    </div>
  {/if}

  {#if history.length > 0}
    <section class="acts">
      <h3>{t("overview.activity")}</h3>
      <ul>
        {#each history.slice(0, 6) as h (h.date + h.action)}
          <li class="act" class:staff={h.staff}>
            <span class="a-ico"><Icon name={actionIcon(h.action)} size={16} /></span>
            <span class="a-body">
              <span class="a-top">
                    <strong>{actionLabel(h.action)}</strong>
                {#if h.result && h.result !== "success"}
                  <span class="a-ko">{h.result}</span>
                {/if}
                <span class="a-date dim">{shortDate(h.date)}</span>
              </span>
              <span class="a-sub dim">
                <!-- `&nbsp;` : Svelte élague l'espace en tête de balise, d'où un « Wasab_II· » collé. -->
                {actorLabel(h)}{#if h.ip}<span class="a-ip selectable">&nbsp;· {h.ip}</span>{/if}
              </span>
              {#if h.context}
                <!-- Contexte fourni par l'API : la commande console tapée, typiquement. -->
                <code class="a-ctx selectable">{h.context}</code>
              {/if}
            </span>
          </li>
        {/each}
      </ul>
    </section>
  {/if}
</div>

<style>
  /* Mises à jour de mods/plugins/datapacks disponibles : renvoie vers l'onglet Mods. */
  .upd-note {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    min-height: 48px;
    padding: 10px 12px;
    border: 1px solid color-mix(in srgb, var(--brand-primary) 45%, var(--border));
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--brand-primary) 10%, transparent);
    color: var(--text);
    font: inherit;
    font-size: 14px;
    text-align: left;
  }
  .upd-note span {
    flex: 1;
  }
  .head-card {
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 14px;
    border: 1px solid var(--border);
    border-radius: 14px;
    background: var(--surface);
  }
  .tile.as-btn {
    text-align: left;
    border: 1px solid var(--border);
    font: inherit;
    cursor: pointer;
  }
  .tile.as-btn strong {
    color: var(--brand-primary);
  }
  .motdbox {
    border: 1px solid var(--border);
    border-radius: 12px;
    background: var(--surface);
    overflow: hidden;
  }
  .motdtoggle {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    padding: 11px 13px;
    background: none;
    border: 0;
    color: var(--text-muted);
    font: inherit;
    font-size: 13px;
  }
  .caret {
    transition: transform 0.15s ease;
  }
  .caret.open {
    transform: rotate(90deg);
  }
  .motdbox .motd {
    padding: 0 13px 12px;
  }

  .meta {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 16px;
    font-size: 13px;
  }
  .linklike {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 0;
    border: 0;
    background: none;
    color: var(--brand-primary);
    font: inherit;
    font-size: 13px;
  }

  .warnbox {
    padding: 12px;
    border: 1px solid var(--border);
    border-radius: 12px;
    background: var(--surface);
  }
  .warntext {
    margin: 0 0 10px;
    font-size: 13px;
    line-height: 1.45;
    color: var(--text-muted);
  }
  .warnact {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .warnact button {
    padding: 8px 14px;
    border-radius: 10px;
    border: 1px solid var(--border);
    font: inherit;
    font-size: 14px;
  }
  .warnact .ghost {
    background: transparent;
    color: var(--text-muted);
  }
  .warnact .danger {
    background: var(--state-danger);
    border-color: transparent;
    color: #fff;
  }

  .acts h3 {
    margin: 0 0 8px;
    font-size: 14px;
    font-weight: 600;
    color: var(--text-muted);
  }
  .acts ul {
    list-style: none;
    margin: 0;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 12px;
    background: var(--surface);
    overflow: hidden;
  }
  .act {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
  }
  .act + .act {
    border-top: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
  }
  .a-ico {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    border-radius: 50%;
    flex: none;
    background: color-mix(in srgb, var(--text) 7%, transparent);
    color: var(--text-muted);
  }
  /* Action du staff de l'hébergeur : signalée par la couleur, pas par un badge de plus. */
  .act.staff .a-ico {
    background: color-mix(in srgb, var(--brand-tertiary) 20%, transparent);
    color: var(--brand-tertiary);
  }
  .a-body {
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
    flex: 1;
  }
  .a-top {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 10px;
  }
  .a-top strong {
    font-size: 14px;
    font-weight: 600;
  }
  .a-date {
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    flex: none;
  }
  .a-sub {
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .a-ip {
    font-variant-numeric: tabular-nums;
  }
  .a-ctx {
    display: block;
    margin-top: 3px;
    padding: 3px 7px;
    border-radius: 6px;
    background: color-mix(in srgb, var(--text) 6%, transparent);
    font-family: var(--font-mono, monospace);
    font-size: 12px;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .a-dot {
    width: 9px;
    height: 9px;
    margin: 0 10px 0 11px;
    border-radius: 50%;
    flex: none;
    align-self: flex-start;
    margin-top: 6px;
  }
  .a-msg {
    font-size: 13px;
    line-height: 1.4;
    color: var(--text-muted);
    overflow-wrap: anywhere;
  }
  .a-ko {
    flex: none;
    padding: 1px 6px;
    border-radius: 999px;
    font-size: 11px;
    background: color-mix(in srgb, var(--state-danger) 20%, transparent);
    color: var(--state-danger);
  }

  .view {
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .status {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .dot {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    flex: none;
  }
  .slabel {
    font-size: 16px;
    font-weight: 600;
  }
  .pill {
    margin-left: auto;
    font-size: 12px;
    color: var(--text-dim);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 3px 10px;
  }
  .gauges {
    display: flex;
    flex-direction: column;
    gap: 14px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 16px;
  }
  .gauge {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .grow {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
  }
  .gname {
    font-size: 13px;
    font-weight: 600;
  }
  .gval {
    font-size: 12px;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }
  .track {
    height: 8px;
    border-radius: 999px;
    background: var(--elevated);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    border-radius: 999px;
    transition:
      width 0.5s ease,
      background 0.3s;
  }
  .tiles {
    display: grid;
    /* Trois colonnes : en 1fr 1fr, la troisième tuile restait seule sur sa ligne. */
    grid-template-columns: repeat(3, 1fr);
    gap: 8px;
  }
  .tile {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    /* Resserré : trois colonnes laissent ~1/3 de largeur par tuile. */
    padding: 10px;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .tile strong {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tile small {
    color: var(--text-dim);
    font-size: 12px;
  }
  .tile strong {
    font-size: 17px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .conn {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 14px;
    display: flex;
    flex-direction: row;
    align-items: center;
    gap: 12px;
  }
  .conncol {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }
  .conn small {
    color: var(--text-dim);
    font-size: 12px;
  }
  .copybtn {
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    flex: none;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--text-muted);
  }
  .addr {
    font-family: var(--font-mono);
    font-size: 14px;
    word-break: break-all;
  }
  .online {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .online small {
    color: var(--text-dim);
    font-size: 12px;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chip {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 4px 10px;
    font-size: 12px;
  }
  .motd {
    margin: 0;
    color: var(--text-muted);
    font-size: 13px;
    font-family: var(--font-mono);
    /* Le MOTD contient de vrais retours à la ligne : les respecter au lieu de tout aplatir. */
    white-space: pre-line;
    line-height: 1.4;
  }
  .power.sticky {
    position: sticky;
    top: 0;
    z-index: 5;
    /* Marges négatives : la barre occupe toute la largeur malgré le padding de `.view`. */
    margin: 0 -16px;
    padding: 10px 16px;
    background: color-mix(in srgb, var(--bg) 92%, transparent);
    backdrop-filter: blur(8px);
    border-bottom: 1px solid var(--border);
  }
  .power {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 8px;
  }
  .power button:disabled {
    opacity: 0.4;
  }
  .power button {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 13px;
    font-weight: 600;
    background: var(--surface);
    color: var(--text);
  }
  .power button.ok {
    background: var(--brand-primary);
    color: #fff;
    border-color: transparent;
  }
  .power button.warn {
    color: var(--state-pending);
  }
  .power button.danger {
    color: var(--state-danger);
  }
  .power button:disabled {
    opacity: 0.5;
  }
  .err {
    margin: 0;
    color: var(--state-danger);
  }
</style>
