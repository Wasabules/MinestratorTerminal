<script lang="ts">
  import { api, humanizeError } from "../../ipc";
  import { t } from "../../i18n";
  import Icon from "../Icon.svelte";
  import type { ConsoleStats, LiveLight, PowerAction } from "../../types";

  let { serverId }: { serverId: number } = $props();

  let live = $state<LiveLight | null>(null);
  let stats = $state<ConsoleStats | null>(null);
  let error = $state<string | null>(null);
  let busy = $state<PowerAction | null>(null);

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

  // --- Helpers d'affichage ---
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
    return t(`status.${s ?? "unknown"}`) !== `status.${s ?? "unknown"}`
      ? t(`status.${s ?? "unknown"}`)
      : t("status.unknown");
  }
  function gaugeColor(pct: number): string {
    if (pct >= 90) return "var(--state-danger)";
    if (pct >= 70) return "var(--state-pending)";
    return "var(--state-running)";
  }
  function fmtBytes(n: number): string {
    if (n <= 0) return "0";
    const gb = n / 1024 ** 3;
    if (gb >= 1) return `${gb.toFixed(1)} Go`;
    return `${Math.round(n / 1024 ** 2)} Mo`;
  }
  function clampPct(n: number): number {
    return Math.max(0, Math.min(100, n));
  }

  // Valeurs dérivées (0 si hors ligne).
  const cpuLimit = $derived(live ? Math.max(live.cpu.dedicated + live.cpu.flexcore, 100) : 100);
  const cpuVal = $derived(stats?.cpu_absolute ?? 0);
  const cpuPct = $derived(clampPct((cpuVal / cpuLimit) * 100));

  const memUsed = $derived(stats?.memory_bytes ?? 0);
  const memLimit = $derived(
    stats?.memory_limit_bytes && stats.memory_limit_bytes > 0
      ? stats.memory_limit_bytes
      : (live?.memory.limit ?? 0) * 1024 ** 2,
  );
  const memPct = $derived(memLimit > 0 ? clampPct((memUsed / memLimit) * 100) : 0);

  const diskUsed = $derived(stats?.disk_bytes ?? 0);
  const diskLimit = $derived((live?.disk.limit ?? 0) * 1024 ** 2);
  const diskPct = $derived(diskLimit > 0 ? clampPct((diskUsed / diskLimit) * 100) : 0);

  $effect(() => {
    refreshLive();
    refreshStats();
    const a = setInterval(refreshLive, 8000);
    const b = setInterval(refreshStats, 4000);
    return () => {
      clearInterval(a);
      clearInterval(b);
    };
  });

  const gauges = $derived([
    {
      key: "overview.cpu",
      pct: cpuPct,
      value: `${cpuVal.toFixed(0)}%`,
    },
    {
      key: "overview.ram",
      pct: memPct,
      value: memLimit > 0 ? `${fmtBytes(memUsed)} / ${fmtBytes(memLimit)}` : "—",
    },
    {
      key: "overview.disk",
      pct: diskPct,
      value: diskLimit > 0 ? `${fmtBytes(diskUsed)} / ${fmtBytes(diskLimit)}` : "—",
    },
  ]);

  const powerButtons: { action: PowerAction; key: string; icon: string; kind: string }[] = [
    { action: "start", key: "power.start", icon: "play", kind: "ok" },
    { action: "restart", key: "power.restart", icon: "restart", kind: "warn" },
    { action: "stop", key: "power.stop", icon: "stop", kind: "warn" },
    { action: "kill", key: "power.kill", icon: "kill", kind: "danger" },
  ];
</script>

<div class="view">
  {#if error}<p class="err selectable">{error}</p>{/if}

  <!-- Statut -->
  <div class="status">
    <span class="dot" style="background:{statusColor(live?.status ?? null)}"></span>
    <span class="slabel">{statusLabel(live?.status ?? null)}</span>
    {#if live?.players}
      <span class="pill selectable">{live.players.current}/{live.players.limit} joueurs</span>
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
        <div class="track">
          <div class="fill" style="width:{g.pct}%; background:{gaugeColor(g.pct)}"></div>
        </div>
      </div>
    {/each}
  </div>

  <!-- Infos -->
  <div class="tiles">
    <div class="tile">
      <small>{t("overview.version")}</small>
      <strong class="selectable">{live?.version ?? "—"}</strong>
    </div>
    <div class="tile">
      <small>{t("overview.players")}</small>
      <strong class="selectable">
        {live?.players ? `${live.players.current}/${live.players.limit}` : "—"}
      </strong>
    </div>
  </div>

  {#if live?.motd}
    <p class="motd selectable">{live.motd}</p>
  {/if}

  <!-- Alimentation -->
  <div class="power">
    {#each powerButtons as b (b.action)}
      <button class={b.kind} disabled={busy !== null} onclick={() => power(b.action)}>
        <Icon name={b.icon} size={18} />
        <span>{busy === b.action ? "…" : t(b.key)}</span>
      </button>
    {/each}
  </div>
</div>

<style>
  .view {
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 18px;
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
    font-size: 18px;
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
