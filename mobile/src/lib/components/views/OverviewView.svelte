<script lang="ts">
  import { api, humanizeError, openExternal } from "../../ipc";
  import { t } from "../../i18n";
  import Icon from "../Icon.svelte";
  import type {
    ConsoleStats,
    LiveLight,
    MyBoxSummary,
    PowerAction,
    ServerAction,
    ServerListItem,
  } from "../../types";

  let {
    server,
    mybox,
    active,
  }: { server: ServerListItem; mybox: MyBoxSummary | null; active: boolean } = $props();

  const serverId = $derived(server.id);

  let live = $state<LiveLight | null>(null);
  let stats = $state<ConsoleStats | null>(null);
  let error = $state<string | null>(null);
  let busy = $state<PowerAction | null>(null);
  let copied = $state(false);
  /** Journal d'actions de l'hébergeur (couvre le panel web et le staff), via le détail serveur. */
  let history = $state<ServerAction[]>([]);
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

  async function loadHistory() {
    try {
      history = (await api.serverDetails(serverId)).history ?? [];
    } catch {
      history = [];
    }
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

  function statusColor(s: string | null): string {
    switch (s) {
      case "running":
        return "var(--state-running)";
      case "starting":
      case "stopping":
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

  const powerButtons: { action: PowerAction; key: string; icon: string; kind: string }[] = [
    { action: "start", key: "power.start", icon: "play", kind: "ok" },
    { action: "restart", key: "power.restart", icon: "restart", kind: "warn" },
    { action: "stop", key: "power.stop", icon: "stop", kind: "warn" },
    { action: "kill", key: "power.kill", icon: "kill", kind: "danger" },
  ];

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

  <!-- Statut + MyBox -->
  <div class="status">
    <span class="dot" style="background:{statusColor(live?.status ?? null)}"></span>
    <span class="slabel">{statusLabel(live?.status ?? null)}</span>
    {#if mybox}
      <span class="pill selectable">{mybox.offer} · {mybox.days_left} j</span>
    {/if}
  </div>

  <!-- Jauges live -->
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

  <!-- Infos serveur -->
  <div class="tiles">
    <div class="tile">
      <small>{t("overview.version")}</small>
      <strong class="selectable">{live?.version ?? "—"}</strong>
    </div>
    <div class="tile">
      <small>{t("overview.players")}</small>
      <strong class="selectable">{live?.players ? `${live.players.current}/${live.players.limit}` : "—"}</strong>
    </div>
    <div class="tile">
      <small>{t("overview.uptime")}</small>
      <strong class="selectable">{fmtUptime(stats?.uptime)}</strong>
    </div>
    <div class="tile">
      <small>Jeu</small>
      <strong class="selectable">{server.egg_name}</strong>
    </div>
  </div>

  <!-- Connexion -->
  <div class="conn">
    <div class="conncol">
      <small>Connexion</small>
      <span class="addr selectable">{server.address}</span>
    </div>
    <button class="copybtn" onclick={copyAddr} aria-label="Copier">
      <Icon name={copied ? "check" : "copy"} size={18} />
    </button>
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

  {#if live?.motd}
    <p class="motd selectable">{live.motd}</p>
  {/if}

  {#if server.last_backup_url || server.support_ref}
    <div class="meta">
      {#if server.last_backup_url}
        <!-- URL signée (jeton porteur) : ouverte sans jamais être affichée. -->
        <button class="linklike" onclick={() => openExternal(server.last_backup_url!)}>
          <Icon name="save" size={15} />
          {t("overview.downloadLastBackup")}
        </button>
      {/if}
      {#if server.support_ref}
        <span class="dim selectable">{t("overview.supportRef")} : {server.support_ref}</span>
      {/if}
    </div>
  {/if}

  <!-- Alimentation -->
  <div class="power">
    {#each powerButtons as b (b.action)}
      <button class={b.kind} disabled={busy !== null} onclick={() => request(b.action)}>
        <Icon name={b.icon} size={18} />
        <span>{busy === b.action ? "…" : t(b.key)}</span>
      </button>
    {/each}
  </div>

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
    <div class="acts">
      <small>{t("overview.activity")}</small>
      {#each history.slice(0, 6) as h (h.date + h.action)}
        <div class="act">
          <span class="a-date dim">{h.date.slice(0, 16)}</span>
          <span class="a-what">{h.action}</span>
          {#if h.staff}<span class="a-staff">{t("overview.byStaff")}</span>{/if}
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
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

  .acts {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .acts small {
    color: var(--text-dim);
    margin-bottom: 2px;
  }
  .act {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
  }
  .a-date {
    font-variant-numeric: tabular-nums;
    flex: none;
  }
  .a-what {
    font-family: var(--font-mono, monospace);
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .a-staff {
    flex: none;
    padding: 1px 6px;
    border-radius: 999px;
    font-size: 11px;
    background: color-mix(in srgb, var(--brand-tertiary) 22%, transparent);
    color: var(--brand-tertiary);
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
    grid-template-columns: 1fr 1fr;
    gap: 10px;
  }
  .tile {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 14px;
    display: flex;
    flex-direction: column;
    gap: 4px;
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
  }
  .power {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
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
