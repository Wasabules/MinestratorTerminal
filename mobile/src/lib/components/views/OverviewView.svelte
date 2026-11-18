<script lang="ts">
  /**
   * Aperçu d'un serveur : le tableau de bord desktop (`ServerOverview.svelte`) ramené à une
   * colonne. La bannière (nom, état, adresse, alimentation) est rendue au-dessus par
   * `ServerHero` ; l'état live (`live`, `stats`) est relevé par `ServerView`, qui le partage avec
   * l'en-tête.
   *
   * Ordre : bandeau d'infos, avis (hibernation, mises à jour), ressources, joueurs, historique,
   * alertes récentes, activité, message du serveur.
   */
  import { untrack } from "svelte";
  import { api } from "../../ipc";
  import { t } from "../../i18n";
  import Icon from "../Icon.svelte";
  import Gauge from "../Gauge.svelte";
  import Masked from "../Masked.svelte";
  import MetricsCard from "../MetricsCard.svelte";
  import { alerts } from "../../stores/alerts.svelte";
  import { loadUpdates, updateCount } from "../../stores/updates.svelte";
  import { avatarStyle } from "../../servers/colors.svelte";
  import type {
    ActivityEntry,
    ConsoleStats,
    LiveLight,
    MyBoxSummary,
    ServerListItem,
  } from "../../types";

  let {
    server,
    mybox,
    live,
    stats,
    error = null,
    active,
    onNavigate,
  }: {
    server: ServerListItem;
    mybox: MyBoxSummary | null;
    live: LiveLight | null;
    stats: ConsoleStats | null;
    /** Dernière erreur de relevé live (`ServerView`). */
    error?: string | null;
    active: boolean;
    /** Ouvre un autre onglet du serveur (joueurs, Marketplace). */
    onNavigate?: (tab: string) => void;
  } = $props();

  /** MOTD replié par défaut : décoratif, il mangeait le haut de l'écran. */
  let motdOpen = $state(false);

  const serverId = $derived(server.id);

  /** Journal d'audit de l'hébergeur (couvre le panel web et le staff), endpoint dédié. */
  let history = $state<ActivityEntry[]>([]);

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

  function fmtBytes(n: number): string {
    if (n <= 0) return "0";
    const gb = n / 1024 ** 3;
    return gb >= 1 ? `${gb.toFixed(1)} Go` : `${Math.round(n / 1024 ** 2)} Mo`;
  }
  function fmtUptime(ms: number | undefined): string | null {
    if (!ms || ms <= 0) return null;
    const s = Math.floor(ms / 1000);
    const d = Math.floor(s / 86400);
    const h = Math.floor((s % 86400) / 3600);
    const m = Math.floor((s % 3600) / 60);
    if (d) return `${d} j ${h} h`;
    if (h) return `${h} h ${m} min`;
    return `${m} min`;
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
  const uptime = $derived(fmtUptime(stats?.uptime));

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
  function alertKind(kind: string): string {
    const k = `alerts.kind.${kind}`;
    return t(k) !== k ? t(k) : kind;
  }

  /** Secondes Unix → `12/08 23:29` (même forme que le journal d'activité). */
  function alertDate(ts: number): string {
    const d = new Date(ts * 1000);
    const p = (n: number) => String(n).padStart(2, "0");
    return `${p(d.getDate())}/${p(d.getMonth() + 1)} ${p(d.getHours())}:${p(d.getMinutes())}`;
  }

  const showPlayers = $derived(server.capabilities?.players !== false && !!live?.players);
  const hasInfos = $derived(!!live?.version || !!uptime || !!mybox || !!server.support_ref);

  // Journal d'audit relu à chaque retour sur l'Aperçu.
  $effect(() => {
    if (!active) return;
    void loadHistory();
  });
</script>

<div class="view">
  <!-- Bandeau d'infos : ce que l'hébergeur fournit pour ce serveur. -->
  {#if hasInfos}
  <div class="infobar">
    {#if live?.version}
      <span>{t("overview.version")} <strong class="selectable">{live.version}</strong></span>
    {/if}
    {#if uptime}
      <span>{t("overview.uptime")} <strong>{uptime}</strong></span>
    {/if}
    {#if mybox}
      <span class:soon={mybox.days_left <= 7}>
        MyBox
        <strong class="selectable">
          {t("overview.myboxDays").replace("{offer}", mybox.offer).replace("{days}", String(mybox.days_left))}
        </strong>
      </span>
    {/if}
    {#if server.support_ref}
      <!-- Référence à donner au support : utile mais secondaire. -->
      <span>{t("overview.supportRef")} <strong class="mono selectable">{server.support_ref}</strong></span>
    {/if}
  </div>
  {/if}

  <div class="body">
    {#if error}<p class="alert selectable">{error}</p>{/if}

    {#if server.status === "hibernation"}
      <div class="note sleep"><span class="ndot"></span><span>{t("overview.hibernated")}</span></div>
    {/if}

    {#if updates > 0}
      <button class="note upd" onclick={() => onNavigate?.("market")}>
        <span class="ndot"></span>
        <span class="ntext">{t("overview.updates").replace("{count}", String(updates))}</span>
        <span class="ngo">{t("overview.updatesOpen")} <Icon name="chevronRight" size={16} stroke={2.6} /></span>
      </button>
    {/if}

    <section class="card res">
      <h2>{t("overview.resources")}</h2>
      <Gauge
        label={t("overview.cpu")}
        percent={clamp((cpuVal / cpuLimit) * 100)}
        detail={stats ? `${cpuVal.toFixed(0)} % / ${cpuLimit} %` : "—"}
      />
      <Gauge
        label={t("overview.ram")}
        percent={memLimit > 0 ? clamp((memUsed / memLimit) * 100) : 0}
        detail={memLimit > 0 ? `${fmtBytes(memUsed)} / ${fmtBytes(memLimit)}` : "—"}
      />
      <Gauge
        label={t("overview.disk")}
        percent={diskLimit > 0 ? clamp((diskUsed / diskLimit) * 100) : 0}
        detail={diskLimit > 0 ? `${fmtBytes(diskUsed)} / ${fmtBytes(diskLimit)}` : "—"}
      />
    </section>

    <!-- Joueurs : seulement quand l'hébergeur en remonte pour ce jeu. -->
    {#if showPlayers && live?.players}
      <section class="card pl">
        <div class="pl-head">
          <h2>{t("overview.players")}</h2>
          <span class="mono pl-count">{live.players.current} / {live.players.limit}</span>
        </div>
        {#if live.players.list.length > 0}
          <div class="pl-list">
            {#each live.players.list.slice(0, 8) as name (name)}
              <span class="phead" style={avatarStyle(name)} aria-hidden="true">{name.charAt(0).toUpperCase()}</span>
            {/each}
            {#if live.players.list.length > 8}<span class="pl-more">+{live.players.list.length - 8}</span>{/if}
          </div>
          <p class="pl-names selectable">{live.players.list.slice(0, 12).join(", ")}</p>
        {:else if live.players.current === 0}
          <p class="pl-names">{t("players.none")}</p>
        {/if}
        <button class="btn btn--accent btn--block" onclick={() => onNavigate?.("players")}>
          <Icon name="players" size={18} />
          {t("overview.openPlayers")}
        </button>
      </section>
    {/if}

    <MetricsCard {serverId} {active} {cpuLimit} {diskLimit} />

    {#if serverAlerts.length > 0}
      <section class="card list">
        <h2>{t("alerts.recent")}</h2>
        <ul>
          {#each serverAlerts as a (a.id)}
            <li class="row">
              <span class="adot" style="background:{alertColor(a.severity)}"></span>
              <span class="r-body">
                <span class="r-top">
                  <strong>{alertKind(a.kind)}</strong>
                  <span class="r-date">{alertDate(a.ts)}</span>
                </span>
                <span class="r-msg selectable"><Masked value={a.message} /></span>
              </span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    {#if history.length > 0}
      <section class="card list">
        <h2>{t("overview.activity")}</h2>
        <ul>
          {#each history.slice(0, 6) as h (h.date + h.action)}
            <li class="row" class:staff={h.staff}>
              <span class="r-ico"><Icon name={actionIcon(h.action)} size={16} /></span>
              <span class="r-body">
                <span class="r-top">
                  <strong>{actionLabel(h.action)}</strong>
                  {#if h.result && h.result !== "success"}
                    <span class="r-ko">{h.result}</span>
                  {/if}
                  <span class="r-date">{shortDate(h.date)}</span>
                </span>
                <span class="r-sub">
                  <!-- `&nbsp;` : Svelte élague l'espace en tête de balise, d'où un « Wasab_II· » collé. -->
                  {actorLabel(h)}{#if h.ip}<span class="r-ip selectable">&nbsp;· <Masked value={h.ip} kind="address" /></span>{/if}
                </span>
                {#if h.context}
                  <!-- Contexte fourni par l'API : la commande console tapée, typiquement. -->
                  <code class="r-ctx selectable"><Masked value={h.context} /></code>
                {/if}
              </span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    {#if live?.motd}
      <section class="card motdbox">
        <button class="motdtoggle" aria-expanded={motdOpen} onclick={() => (motdOpen = !motdOpen)}>
          <span>{t("overview.motd")}</span>
          <span class="caret" class:open={motdOpen}><Icon name="chevronDown" size={18} /></span>
        </button>
        {#if motdOpen}<p class="motd selectable"><Masked value={live.motd} /></p>{/if}
      </section>
    {/if}
  </div>
</div>

<style>
  .view {
    display: flex;
    flex-direction: column;
  }

  /* --- Bandeau d'infos ------------------------------------------------------------ */
  .infobar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 18px;
    padding: 10px 16px;
    background: var(--inset);
    border-bottom: 1px solid var(--border);
    font-size: 13.5px;
    color: var(--text-muted);
  }
  .infobar strong {
    color: var(--text);
  }
  .infobar strong.mono {
    font-weight: 500;
    font-size: 13px;
  }
  .infobar .soon strong {
    color: var(--state-pending);
  }

  /* --- Corps ---------------------------------------------------------------------- */
  .body {
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 16px 16px 24px;
  }
  h2 {
    margin: 0;
    font-size: 17px;
  }
  .card {
    padding: 16px;
  }
  .alert {
    margin: 0;
    padding: 10px 13px;
    border-radius: var(--radius);
    background: var(--danger-bg);
    color: var(--danger-ink);
    font-size: 14px;
  }

  /* Avis : pastille carrée de couleur, comme le desktop. */
  .note {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    min-height: 52px;
    padding: 10px 14px;
    border: none;
    border-radius: 12px;
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
    background: color-mix(in srgb, var(--state-pending) 14%, var(--surface));
  }
  .note.upd .ndot {
    background: var(--state-pending);
  }
  .ntext {
    flex: 1;
  }
  .ngo {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    font-weight: 900;
    color: var(--state-pending);
    white-space: nowrap;
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
    font-size: 13.5px;
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
    overflow-wrap: anywhere;
  }

  /* Listes (alertes, activité) */
  .list h2 {
    margin-bottom: 4px;
  }
  .list ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .row {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    padding: 11px 0;
    border-top: 1px solid var(--border);
  }
  .row:first-child {
    border-top: none;
  }
  .adot {
    width: 9px;
    height: 9px;
    margin: 6px 3px 0;
    border-radius: 50%;
    flex: none;
  }
  .r-ico {
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border-radius: 9px;
    flex: none;
    background: var(--accent-soft);
    color: var(--accent-text);
  }
  /* Action du staff de l'hébergeur : signalée par la couleur, pas par un badge de plus. */
  .row.staff .r-ico {
    background: color-mix(in srgb, var(--state-hibernate) 18%, transparent);
    color: var(--state-hibernate);
  }
  .row.staff .r-sub {
    color: var(--state-hibernate);
  }
  .r-body {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    flex: 1;
  }
  .r-top {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }
  .r-top strong {
    font-size: 14.5px;
    margin-right: auto;
    min-width: 0;
  }
  .r-date {
    font-size: 12.5px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
    flex: none;
  }
  .r-sub {
    font-size: 13px;
    color: var(--text-dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .r-ip {
    font-family: var(--font-mono);
    font-size: 12px;
  }
  .r-ctx {
    display: block;
    margin-top: 4px;
    padding: 3px 7px;
    border-radius: 6px;
    background: var(--inset);
    font-size: 12px;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .r-msg {
    font-size: 13.5px;
    line-height: 1.4;
    color: var(--text-muted);
    overflow-wrap: anywhere;
  }
  .r-ko {
    flex: none;
    padding: 0 7px;
    border-radius: 999px;
    font-size: 11.5px;
    font-weight: 800;
    background: var(--danger-bg);
    color: var(--danger-ink);
  }

  /* Message du serveur (MOTD) */
  .motdbox {
    padding: 0;
    overflow: hidden;
  }
  .motdtoggle {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    min-height: 52px;
    padding: 0 16px;
    background: none;
    border: 0;
    color: var(--text);
    font-size: 15px;
    font-weight: 800;
  }
  .caret {
    display: inline-flex;
    color: var(--text-muted);
    transition: transform 0.15s ease;
  }
  .caret.open {
    transform: rotate(180deg);
  }
  .motd {
    margin: 0;
    padding: 0 16px 14px;
    color: var(--text-muted);
    font-size: 13px;
    font-family: var(--font-mono);
    /* Le MOTD contient de vrais retours à la ligne : les respecter au lieu de tout aplatir. */
    white-space: pre-line;
    line-height: 1.45;
  }
</style>
