<script lang="ts">
  import { api, humanizeError } from "../../ipc";
  import { t } from "../../i18n";
  import type { LiveLight, PowerAction } from "../../types";

  let { serverId }: { serverId: number } = $props();

  let live = $state<LiveLight | null>(null);
  let error = $state<string | null>(null);
  let busy = $state<PowerAction | null>(null);

  async function refresh() {
    try {
      live = await api.liveLight(serverId);
      error = null;
    } catch (err) {
      error = humanizeError(err);
    }
  }

  async function power(action: PowerAction) {
    if (busy) return;
    busy = action;
    try {
      await api.powerAction(serverId, action);
      // Laisse le serveur changer d'état, puis rafraîchit.
      setTimeout(refresh, 1500);
    } catch (err) {
      error = humanizeError(err);
    } finally {
      busy = null;
    }
  }

  function statusColor(status: string | null): string {
    switch (status) {
      case "running":
        return "var(--state-running)";
      case "starting":
      case "stopping":
        return "var(--state-pending)";
      case "offline":
        return "var(--state-offline)";
      default:
        return "var(--state-offline)";
    }
  }

  $effect(() => {
    refresh();
    const id = setInterval(refresh, 5000);
    return () => clearInterval(id);
  });

  const powerButtons: { action: PowerAction; key: string; kind: string }[] = [
    { action: "start", key: "power.start", kind: "ok" },
    { action: "restart", key: "power.restart", kind: "warn" },
    { action: "stop", key: "power.stop", kind: "warn" },
    { action: "kill", key: "power.kill", kind: "danger" },
  ];
</script>

<div class="view">
  {#if error}
    <p class="err selectable">{error}</p>
  {/if}

  {#if live}
    <div class="status">
      <span class="dot" style="background:{statusColor(live.status)}"></span>
      <span class="selectable">{live.status ?? "—"}</span>
    </div>

    <div class="grid">
      <div class="tile">
        <small>{t("overview.players")}</small>
        <strong class="selectable">
          {live.players ? `${live.players.current}/${live.players.limit}` : "—"}
        </strong>
      </div>
      <div class="tile">
        <small>{t("overview.version")}</small>
        <strong class="selectable">{live.version ?? "—"}</strong>
      </div>
      <div class="tile">
        <small>{t("overview.ram")}</small>
        <strong class="selectable">{live.memory.limit} MB</strong>
      </div>
      <div class="tile">
        <small>{t("overview.disk")}</small>
        <strong class="selectable">{live.disk.limit} MB</strong>
      </div>
    </div>

    {#if live.motd}
      <p class="motd selectable">{live.motd}</p>
    {/if}
  {:else if !error}
    <p class="dim">…</p>
  {/if}

  <div class="power">
    {#each powerButtons as b (b.action)}
      <button class={b.kind} disabled={busy !== null} onclick={() => power(b.action)}>
        {busy === b.action ? "…" : t(b.key)}
      </button>
    {/each}
  </div>
</div>

<style>
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
    font-size: 15px;
    text-transform: capitalize;
  }
  .dot {
    width: 12px;
    height: 12px;
    border-radius: 50%;
  }
  .grid {
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
    font-size: 20px;
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
    margin-top: 4px;
  }
  .power button {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 14px;
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
  .dim {
    color: var(--text-dim);
  }
</style>
