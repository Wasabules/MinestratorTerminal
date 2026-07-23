<script lang="ts">
  import { api, humanizeError } from "../../ipc";
  import { consoleEvents } from "../../events";
  import { t } from "../../i18n";
  import type { UnlistenFn } from "@tauri-apps/api/event";

  let { serverId }: { serverId: number } = $props();

  const MAX_LINES = 800;
  let lines = $state<string[]>([]);
  let phase = $state<string>("connecting");
  let input = $state("");
  let error = $state<string | null>(null);
  let logEl: HTMLDivElement | null = $state(null);

  // Supprime les séquences ANSI (xterm côté desktop ; ici affichage texte simple).
  // eslint-disable-next-line no-control-regex
  const ANSI = /\[[0-9;]*m/g;
  function push(line: string) {
    lines.push(line.replace(ANSI, ""));
    if (lines.length > MAX_LINES) lines.splice(0, lines.length - MAX_LINES);
  }

  async function send(e: Event) {
    e.preventDefault();
    const cmd = input.trim();
    if (cmd === "") return;
    input = "";
    try {
      await api.sendCommand(serverId, cmd);
    } catch (err) {
      error = humanizeError(err);
    }
  }

  $effect(() => {
    // conn_id unique par (montage de) vue : plusieurs serveurs/onglets restent indépendants.
    const connId = `mobile-${serverId}-${crypto.randomUUID?.() ?? Math.random().toString(36).slice(2)}`;
    let unlisteners: UnlistenFn[] = [];
    let disposed = false;

    (async () => {
      unlisteners.push(
        await consoleEvents.output((p) => {
          if (p.conn_id === connId) push(p.line);
        }),
      );
      unlisteners.push(
        await consoleEvents.connection((p) => {
          if (p.conn_id === connId) phase = p.phase;
        }),
      );

      // Amorce avec les derniers logs (REST), puis branche le WebSocket live.
      try {
        const recent = await api.consoleLogs(serverId);
        for (const l of recent) push(l);
      } catch {
        /* pas bloquant */
      }
      if (disposed) return;
      try {
        await api.consoleConnect(connId, serverId);
      } catch (err) {
        error = humanizeError(err);
      }
    })();

    return () => {
      disposed = true;
      for (const u of unlisteners) u();
      api.consoleDisconnect(connId);
    };
  });

  // Auto-scroll en bas à chaque nouvelle ligne.
  $effect(() => {
    void lines.length;
    if (logEl) logEl.scrollTop = logEl.scrollHeight;
  });
</script>

<div class="console">
  <div class="log selectable" bind:this={logEl}>
    {#each lines as line, i (i)}
      <div class="line">{line}</div>
    {/each}
    {#if phase !== "open" && lines.length === 0}
      <div class="dim">{t("console.connecting")}</div>
    {/if}
  </div>

  {#if error}<p class="err selectable">{error}</p>{/if}

  <form onsubmit={send}>
    <input
      class="selectable"
      type="text"
      autocapitalize="off"
      autocomplete="off"
      spellcheck="false"
      placeholder={t("console.placeholder")}
      bind:value={input}
    />
    <button type="submit" aria-label={t("console.send")}>▶</button>
  </form>
</div>

<style>
  .console {
    display: flex;
    flex-direction: column;
    height: 100%;
  }
  .log {
    flex: 1;
    overflow-y: auto;
    padding: 12px;
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.5;
    background: #0d1114;
    -webkit-overflow-scrolling: touch;
  }
  .line {
    white-space: pre-wrap;
    word-break: break-word;
    color: var(--text-muted);
  }
  .dim {
    color: var(--text-dim);
  }
  form {
    display: flex;
    gap: 8px;
    padding: 8px;
    border-top: 1px solid var(--border);
    background: var(--surface);
  }
  input {
    flex: 1;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--text);
    padding: 12px;
    font-family: var(--font-mono);
    font-size: 14px;
    outline: none;
  }
  input:focus {
    border-color: var(--brand-primary);
  }
  button {
    background: var(--brand-primary);
    color: #fff;
    border: none;
    border-radius: var(--radius-sm);
    width: 48px;
    font-size: 16px;
  }
  .err {
    margin: 0;
    padding: 4px 12px;
    color: var(--state-danger);
    font-size: 13px;
  }
</style>
