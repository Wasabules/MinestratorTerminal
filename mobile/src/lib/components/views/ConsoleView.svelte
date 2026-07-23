<script lang="ts">
  import { api, humanizeError } from "../../ipc";
  import { consoleEvents } from "../../events";
  import { t } from "../../i18n";
  import { suggest } from "../../mcCommands";
  import Icon from "../Icon.svelte";
  import type { UnlistenFn } from "@tauri-apps/api/event";

  let {
    serverId,
    active,
    onFocusChange,
  }: { serverId: number; active: boolean; onFocusChange?: (f: boolean) => void } = $props();

  const MAX_LINES = 800;
  let lines = $state<string[]>([]);
  let phase = $state<string>("connecting");
  let input = $state("");
  let error = $state<string | null>(null);
  let logEl: HTMLDivElement | null = $state(null);
  let inputEl: HTMLInputElement | null = $state(null);

  let fontSize = $state(clampFont(Number(readLS("console.font")) || 12));
  let filter = $state<"all" | "warn" | "error">("all");
  let showFilter = $state(false);
  let inputFocused = $state(false);

  function readLS(k: string) {
    try {
      return localStorage.getItem(k);
    } catch {
      return null;
    }
  }
  function clampFont(n: number) {
    return Math.max(9, Math.min(20, n));
  }
  function setFont(n: number) {
    fontSize = clampFont(n);
    try {
      localStorage.setItem("console.font", String(fontSize));
    } catch {
      /* ignore */
    }
  }

  // eslint-disable-next-line no-control-regex
  const ANSI = /\[[0-9;]*m|\[[0-9;]*m/g;
  function push(line: string) {
    lines.push(line.replace(ANSI, ""));
    if (lines.length > MAX_LINES) lines.splice(0, lines.length - MAX_LINES);
  }

  function level(line: string): "error" | "warn" | "info" {
    if (/\b(ERROR|SEVERE|FATAL)\b/i.test(line)) return "error";
    if (/\bWARN(ING)?\b/i.test(line)) return "warn";
    return "info";
  }

  const filtered = $derived(
    filter === "all"
      ? lines
      : filter === "error"
        ? lines.filter((l) => level(l) === "error")
        : lines.filter((l) => level(l) !== "info"),
  );

  const suggestions = $derived(inputFocused ? suggest(input) : []);

  function applySuggestion(s: string) {
    input = s.endsWith(" ") ? s : s + " ";
    inputEl?.focus();
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
    inputEl?.focus();
  }

  function onFocus() {
    inputFocused = true;
    onFocusChange?.(true);
  }
  function onBlur() {
    inputFocused = false;
    onFocusChange?.(false);
  }

  const filterLabel = $derived(
    filter === "all" ? t("console.all") : filter === "warn" ? t("console.warn") : t("console.error"),
  );

  $effect(() => {
    if (!active) return;
    lines = [];
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
      onFocusChange?.(false);
    };
  });

  // Auto-scroll en bas à chaque nouvelle ligne (si non filtré activement).
  $effect(() => {
    void filtered.length;
    if (logEl) logEl.scrollTop = logEl.scrollHeight;
  });
</script>

<div class="console">
  <header>
    <span class="phase" class:open={phase === "open"}>
      {phase === "open" ? "●" : "○"}
      {phase !== "open" ? t("console.connecting") : ""}
    </span>
    <div class="tools">
      <button class="tool" aria-label="A-" onclick={() => setFont(fontSize - 1)}>A−</button>
      <button class="tool" aria-label="A+" onclick={() => setFont(fontSize + 1)}>A+</button>
      <div class="filterwrap">
        <button
          class="tool"
          class:active={filter !== "all"}
          aria-label={t("console.filter")}
          onclick={() => (showFilter = !showFilter)}
        >
          <Icon name="filter" size={16} />
          <span class="flabel">{filterLabel}</span>
        </button>
        {#if showFilter}
          <div class="menu">
            {#each [["all", "console.all"], ["warn", "console.warn"], ["error", "console.error"]] as [val, key] (val)}
              <button
                class:sel={filter === val}
                onclick={() => {
                  filter = val as typeof filter;
                  showFilter = false;
                }}
              >
                {t(key)}
              </button>
            {/each}
          </div>
        {/if}
      </div>
    </div>
  </header>

  <div class="log selectable" bind:this={logEl} style="font-size:{fontSize}px">
    {#each filtered as line, i (i)}
      <div class="line {level(line)}">{line}</div>
    {/each}
    {#if filtered.length === 0}
      <div class="dim">{phase === "open" ? "—" : t("console.connecting")}</div>
    {/if}
  </div>

  {#if error}<p class="err selectable">{error}</p>{/if}

  {#if suggestions.length > 0}
    <div class="suggest">
      {#each suggestions as s (s)}
        <button type="button" onmousedown={(e) => e.preventDefault()} onclick={() => applySuggestion(s)}>
          {s.trim()}
        </button>
      {/each}
    </div>
  {/if}

  <form onsubmit={send}>
    <input
      class="selectable"
      bind:this={inputEl}
      type="text"
      autocapitalize="off"
      autocomplete="off"
      spellcheck="false"
      placeholder={t("console.placeholder")}
      bind:value={input}
      onfocus={onFocus}
      onblur={onBlur}
    />
    <button type="submit" class="sendbtn" aria-label="Envoyer"><Icon name="send" size={18} /></button>
  </form>
</div>

<style>
  .console {
    display: flex;
    flex-direction: column;
    height: 100%;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 6px 10px;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
    flex: none;
  }
  .phase {
    font-size: 12px;
    color: var(--text-dim);
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .phase.open {
    color: var(--state-running);
  }
  .tools {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .tool {
    display: flex;
    align-items: center;
    gap: 5px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--text-muted);
    padding: 6px 9px;
    font-size: 13px;
    min-height: 34px;
  }
  .tool.active {
    color: var(--brand-primary);
    border-color: var(--brand-primary);
  }
  .flabel {
    font-size: 12px;
  }
  .filterwrap {
    position: relative;
  }
  .menu {
    position: absolute;
    right: 0;
    top: calc(100% + 4px);
    background: var(--elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    box-shadow: 0 6px 20px rgba(0, 0, 0, 0.35);
    z-index: 20;
    overflow: hidden;
    min-width: 130px;
  }
  .menu button {
    display: block;
    width: 100%;
    text-align: left;
    background: transparent;
    border: none;
    color: var(--text);
    padding: 11px 14px;
    font-size: 14px;
  }
  .menu button.sel {
    color: var(--brand-primary);
    font-weight: 600;
  }
  .log {
    flex: 1;
    overflow-y: auto;
    padding: 10px 12px;
    font-family: var(--font-mono);
    line-height: 1.5;
    background: #0d1114;
    -webkit-overflow-scrolling: touch;
  }
  .line {
    white-space: pre-wrap;
    word-break: break-word;
    color: var(--text-muted);
  }
  .line.warn {
    color: var(--state-pending);
  }
  .line.error {
    color: var(--state-danger);
  }
  .dim {
    color: var(--text-dim);
  }
  .suggest {
    display: flex;
    gap: 6px;
    padding: 8px;
    overflow-x: auto;
    background: var(--surface);
    border-top: 1px solid var(--border);
    scrollbar-width: none;
  }
  .suggest::-webkit-scrollbar {
    display: none;
  }
  .suggest button {
    flex: none;
    background: var(--elevated);
    border: 1px solid var(--border);
    border-radius: 999px;
    color: var(--text);
    padding: 7px 13px;
    font-family: var(--font-mono);
    font-size: 13px;
    white-space: nowrap;
  }
  form {
    display: flex;
    gap: 8px;
    padding: 8px;
    border-top: 1px solid var(--border);
    background: var(--surface);
    flex: none;
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
  .sendbtn {
    display: grid;
    place-items: center;
    background: var(--brand-primary);
    color: #fff;
    border: none;
    border-radius: var(--radius-sm);
    width: 48px;
  }
  .err {
    margin: 0;
    padding: 4px 12px;
    color: var(--state-danger);
    font-size: 13px;
  }
</style>
