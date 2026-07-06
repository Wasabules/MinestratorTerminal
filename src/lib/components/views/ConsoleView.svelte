<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { Terminal } from '@xterm/xterm';
  import { FitAddon } from '@xterm/addon-fit';
  import { WebLinksAddon } from '@xterm/addon-web-links';
  import '@xterm/xterm/css/xterm.css';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { api, humanizeError } from '$lib/ipc';
  import { consoleEvents } from '$lib/events';
  import { openCopilotMenu } from '$lib/copilot/menu.svelte';
  import { PASTE_SERVICES, pasteExport } from '$lib/paste';
  import { redactLine } from '$lib/redact';
  import { t } from '$lib/i18n';
  import { isRunning } from '$lib/status';
  import PowerControl from '../PowerControl.svelte';
  import Icon from '../Icon.svelte';
  import type { ServerTab } from '$lib/tabs/tabs.svelte';

  let { tab }: { tab: ServerTab } = $props();
  const serverId = $derived(tab.serverId);
  const connId = $derived(tab.id);

  let container: HTMLDivElement;
  let term: Terminal | undefined;
  let fit: FitAddon | undefined;
  let resizeObs: ResizeObserver | undefined;
  const unlisteners: UnlistenFn[] = [];

  let phase = $state('connecting');
  let runState = $state('offline');
  let atBottom = $state(true);
  let command = $state('');
  const history: string[] = [];
  let histIndex = 0;

  // --- Auto-complétion du 1er mot (nom de commande) : catalogue MC + historique. ---
  const MC_COMMANDS: string[] = [
    'advancement', 'attribute', 'ban', 'ban-ip', 'banlist', 'bossbar', 'clear', 'clone', 'data',
    'datapack', 'debug', 'defaultgamemode', 'deop', 'difficulty', 'effect', 'enchant', 'execute',
    'experience', 'fill', 'forceload', 'function', 'gamemode', 'gamerule', 'give', 'help', 'item',
    'jfr', 'kick', 'kill', 'list', 'locate', 'loot', 'me', 'msg', 'op', 'pardon', 'pardon-ip',
    'particle', 'perf', 'place', 'playsound', 'publish', 'recipe', 'reload', 'save-all', 'save-off',
    'save-on', 'say', 'schedule', 'scoreboard', 'seed', 'setblock', 'setidletimeout', 'setworldspawn',
    'spawnpoint', 'spectate', 'spreadplayers', 'stop', 'stopsound', 'summon', 'tag', 'team',
    'teleport', 'tell', 'tellraw', 'time', 'title', 'tp', 'trigger', 'weather', 'whitelist',
    'worldborder', 'xp',
    'plugins', 'pl', 'version', 'ver', 'tps', 'mspt', 'spark', 'timings', 'restart',
  ];
  // Commandes dont le 1er argument est un joueur → complétion par les joueurs connectés.
  const PLAYER_CMDS = new Set([
    'op', 'deop', 'kick', 'ban', 'ban-ip', 'pardon', 'pardon-ip', 'tp', 'teleport',
    'tell', 'msg', 'w', 'kill', 'spectate', 'give',
  ]);
  let onlinePlayers = $state<string[]>([]); // alimenté par l'API (refreshPlayers)
  let sugIndex = $state(-1);
  let sugDismissed = $state(false);
  const suggestions = $derived.by(() => {
    const parts = command.split(' ');
    const frag = (parts[parts.length - 1] ?? '').toLowerCase();
    if (parts.length === 1) {
      // 1er mot : nom de commande (catalogue MC + historique)
      if (!frag) return [];
      const hist = history.map((h) => h.split(' ')[0]);
      return [...new Set([...MC_COMMANDS, ...hist])]
        .filter((x) => x.toLowerCase().startsWith(frag) && x.toLowerCase() !== frag)
        .slice(0, 8);
    }
    // 1er argument d'une commande « joueur » : joueurs connectés (frag vide = tous)
    if (parts.length === 2 && PLAYER_CMDS.has(parts[0].toLowerCase())) {
      return onlinePlayers
        .filter((p) => p.toLowerCase().startsWith(frag) && p.toLowerCase() !== frag)
        .slice(0, 8);
    }
    return [];
  });
  const showSug = $derived(suggestions.length > 0 && !sugDismissed);
  function applySuggestion(i: number) {
    const s = suggestions[i];
    if (s == null) return;
    const parts = command.split(' ');
    parts[parts.length - 1] = s; // remplace le mot en cours (commande OU joueur)
    command = `${parts.join(' ')} `;
    sugIndex = -1;
  }

  type Level = 'error' | 'warn' | 'info';
  let filter = $state<Record<Level, boolean>>({ error: true, warn: true, info: true });
  const buffer: string[] = [];
  const MAX_BUFFER = 5000;
  let redactConsole = false; // anonymisation d'affichage (réglage confidentialité)
  // Menu contextuel console (analyse sélection + export du log) et toast de retour d'export.
  let cmenu = $state<{ x: number; y: number; selection: string } | null>(null);
  let toast = $state('');
  let toastTimer: ReturnType<typeof setTimeout> | undefined;

  const running = $derived(isRunning(runState));
  const phaseColor = $derived(
    phase === 'open'
      ? 'var(--state-running)'
      : phase === 'closed' || phase === 'hibernated'
        ? 'var(--state-offline)'
        : 'var(--state-pending)'
  );

  function levelOf(line: string): Level | 'other' {
    if (/\b(ERROR|SEVERE|FATAL)\b/.test(line)) return 'error';
    if (/\bWARN(?:ING)?\b/.test(line)) return 'warn';
    if (/\bINFO\b/.test(line)) return 'info';
    return 'other';
  }
  function passes(line: string): boolean {
    const l = levelOf(line);
    return l === 'other' || filter[l];
  }
  function pushLine(line: string) {
    if (redactConsole) line = redactLine(line);
    buffer.push(line);
    if (buffer.length > MAX_BUFFER) buffer.shift();
    if (passes(line)) term?.writeln(line);
  }

  // Joueurs connectés via l'API (server_details.players.list) — fiable, contrairement à un parse
  // console. Rafraîchi à la demande (throttle 8 s) quand on saisit l'argument d'une commande « joueur ».
  let lastPlayerFetch = 0;
  async function refreshPlayers() {
    lastPlayerFetch = Date.now();
    try {
      const live = await api.liveLight(serverId);
      onlinePlayers = live.players?.list ?? [];
    } catch {
      /* indisponible (serveur hors-ligne, etc.) */
    }
  }
  function maybeRefreshPlayers() {
    const parts = command.split(' ');
    if (parts.length >= 2 && PLAYER_CMDS.has(parts[0].toLowerCase()) && Date.now() - lastPlayerFetch > 8000) {
      void refreshPlayers();
    }
  }

  function rerender() {
    if (!term) return;
    term.clear();
    for (const line of buffer) if (passes(line)) term.writeln(line);
  }
  function toggleLevel(l: Level) {
    filter[l] = !filter[l];
    rerender();
  }

  let destroyed = false;
  onMount(() => {
    init();
    void refreshPlayers();
  });
  onDestroy(() => {
    destroyed = true;
    clearTimeout(toastTimer);
    unlisteners.forEach((u) => u());
    resizeObs?.disconnect();
    api.consoleDisconnect(connId).catch(() => {});
    term?.dispose();
  });

  async function init() {
    term = new Terminal({
      convertEol: true,
      cursorBlink: false,
      disableStdin: true,
      fontFamily: 'ui-monospace, "Cascadia Code", "JetBrains Mono", Consolas, monospace',
      fontSize: 13,
      scrollback: MAX_BUFFER,
      theme: {
        background: '#0b0f11',
        foreground: '#d6e2e6',
        cursor: '#009b72',
        selectionBackground: '#264b40',
      },
    });
    fit = new FitAddon();
    term.loadAddon(fit);
    // Liens cliquables : ouverture dans le navigateur via le plugin opener (le webview est sous CSP).
    term.loadAddon(new WebLinksAddon((_event, uri) => void openUrl(uri)));
    term.open(container);
    // La console est un canvas xterm → le Ctrl+C natif ne copie pas la sélection. On la copie
    // explicitement (Ctrl+C / Ctrl+Shift+C, ou Cmd+C sur macOS). Détaché avec le terminal (dispose).
    term.attachCustomKeyEventHandler((e) => {
      if (e.type === 'keydown' && (e.ctrlKey || e.metaKey) && (e.key === 'c' || e.key === 'C')) {
        const sel = term?.getSelection() ?? '';
        if (sel) {
          e.preventDefault();
          void navigator.clipboard.writeText(sel);
          return false; // événement consommé : pas de traitement xterm ni de copie DOM (vide)
        }
      }
      return true;
    });
    safeFit();

    // Détecte si la vue est collée au bas (pour afficher la flèche de retour).
    term.onScroll(() => {
      const b = term?.buffer.active;
      if (b) atBottom = b.viewportY >= b.baseY;
    });

    resizeObs = new ResizeObserver(() => safeFit());
    resizeObs.observe(container);

    redactConsole = await api
      .getPrivacyConfig()
      .then((c) => c.redact_console)
      .catch(() => false);
    if (destroyed) return;

    try {
      const logs = await api.consoleLogs(serverId);
      for (const line of logs) pushLine(line);
    } catch {
      /* pas de logs */
    }
    if (destroyed) return;

    unlisteners.push(
      await consoleEvents.output((p) => {
        if (p.conn_id === connId) pushLine(p.line);
      })
    );
    unlisteners.push(
      await consoleEvents.status((p) => {
        if (p.conn_id === connId) runState = p.state;
      })
    );
    unlisteners.push(
      await consoleEvents.connection((p) => {
        if (p.conn_id === connId) phase = p.phase;
      })
    );
    // Démonté pendant l'enregistrement → on détache les listeners et on n'ouvre pas la connexion.
    if (destroyed) {
      unlisteners.forEach((u) => u());
      return;
    }

    await api.consoleConnect(connId, serverId).catch(() => {});
    if (destroyed) api.consoleDisconnect(connId).catch(() => {});
  }

  function safeFit() {
    try {
      if (container && container.clientWidth > 0) fit?.fit();
    } catch {
      /* ignore */
    }
  }

  function scrollBottom() {
    term?.scrollToBottom();
    atBottom = true;
  }

  function onContextMenu(e: MouseEvent) {
    e.preventDefault();
    cmenu = { x: e.clientX, y: e.clientY, selection: term?.getSelection() ?? '' };
  }
  function analyzeSel(m: { x: number; y: number; selection: string }) {
    const sel = m.selection;
    cmenu = null;
    if (sel.trim()) openCopilotMenu({ x: m.x, y: m.y, text: sel, serverId, serverName: tab.serverName });
  }
  function showToast(msg: string, ms: number) {
    toast = msg;
    clearTimeout(toastTimer);
    if (ms > 0) toastTimer = setTimeout(() => (toast = ''), ms);
  }
  async function exportLog(service: string) {
    cmenu = null;
    const content = buffer.join('\n');
    if (!content.trim()) return;
    showToast(t('console.exporting'), 0);
    try {
      await pasteExport(service, content);
      showToast(t('console.exported'), 4000);
    } catch (e) {
      showToast(humanizeError(e), 5000);
    }
  }

  function submit(event: Event) {
    event.preventDefault();
    const cmd = command.trim();
    if (!cmd) return;
    api.sendCommand(serverId, cmd).catch(() => {});
    history.push(cmd);
    histIndex = history.length;
    command = '';
  }

  function onKey(event: KeyboardEvent) {
    // Dropdown de complétion ouvert : les flèches/Tab/Entrée le pilotent.
    if (showSug) {
      if (event.key === 'ArrowDown') {
        sugIndex = (sugIndex + 1) % suggestions.length;
        event.preventDefault();
        return;
      }
      if (event.key === 'ArrowUp') {
        sugIndex = (sugIndex <= 0 ? suggestions.length : sugIndex) - 1;
        event.preventDefault();
        return;
      }
      if (event.key === 'Tab') {
        applySuggestion(sugIndex < 0 ? 0 : sugIndex);
        event.preventDefault();
        return;
      }
      if (event.key === 'Enter' && sugIndex >= 0) {
        applySuggestion(sugIndex);
        event.preventDefault();
        return;
      }
      if (event.key === 'Escape') {
        sugDismissed = true;
        sugIndex = -1;
        event.preventDefault();
        return;
      }
    }
    // Sinon : navigation dans l'historique des commandes.
    if (event.key === 'ArrowUp' && histIndex > 0) {
      histIndex -= 1;
      command = history[histIndex] ?? '';
      event.preventDefault();
    } else if (event.key === 'ArrowDown' && histIndex < history.length) {
      histIndex += 1;
      command = history[histIndex] ?? '';
    }
  }
</script>

<div class="console">
  <div class="statusbar">
    <div class="sb-left">
      <span class="chip"><span class="dot" style="background: {phaseColor}"></span>{t(`console.${phase}`)}</span>
      <span class="srv">{tab.serverName}</span>
    </div>
    <div class="sb-right">
      <div class="filters" role="group">
        <button class="lv err" class:off={!filter.error} onclick={() => toggleLevel('error')}>ERROR</button>
        <button class="lv warn" class:off={!filter.warn} onclick={() => toggleLevel('warn')}>WARN</button>
        <button class="lv info" class:off={!filter.info} onclick={() => toggleLevel('info')}>INFO</button>
      </div>
      <PowerControl serverId={serverId} running={running} disabled={phase === 'hibernated'} compact />
    </div>
  </div>

  <div class="termwrap" oncontextmenu={onContextMenu} role="presentation">
    <div class="term" bind:this={container}></div>
    {#if !atBottom}
      <button class="tobottom" onclick={scrollBottom} title={t('console.toBottom')}><Icon name="arrow-down" size={16} /></button>
    {/if}
  </div>

  <form class="cmdbar" onsubmit={submit}>
    {#if showSug}
      <div class="suggest" role="listbox">
        {#each suggestions as s, i (s)}
          <button
            type="button"
            class="sug"
            class:on={i === sugIndex}
            onmousedown={(e) => e.preventDefault()}
            onclick={() => applySuggestion(i)}
          >{s}</button>
        {/each}
      </div>
    {/if}
    <span class="prompt">›</span>
    <input
      class="cmd"
      bind:value={command}
      onkeydown={onKey}
      oninput={() => { sugDismissed = false; sugIndex = -1; maybeRefreshPlayers(); }}
      placeholder={t('console.placeholder')}
      spellcheck="false"
      autocomplete="off"
    />
    <button class="send" type="submit" disabled={command.trim().length === 0}>{t('console.send')}</button>
  </form>
</div>

{#if cmenu}
  {@const m = cmenu}
  <button class="cbackdrop" onclick={() => (cmenu = null)} aria-label={t('common.close')}></button>
  <div class="cmenu" style="left: {m.x}px; top: {m.y}px" role="menu">
    {#if m.selection.trim()}
      <button class="cmi" onclick={() => analyzeSel(m)}>{t('console.analyzeSel')}</button>
      <div class="cmsep"></div>
    {/if}
    <div class="cmlbl">{t('console.exportLog')}</div>
    {#each PASTE_SERVICES as s (s.id)}
      <button class="cmi" onclick={() => exportLog(s.id)}>{s.label}</button>
    {/each}
  </div>
{/if}
{#if toast}<div class="ctoast">{toast}</div>{/if}

<style>
  .console {
    height: 100%;
    display: flex;
    flex-direction: column;
    background: #0b0f11;
    overflow: hidden;
  }
  .statusbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 14px;
    border-bottom: 1px solid #1c2429;
    flex: none;
    flex-wrap: wrap;
  }
  .sb-left {
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
  }
  .sb-right {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    font-family: var(--font-mono);
    color: #9fb0b5;
    flex: none;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }
  .srv {
    font-size: 12px;
    color: #6f8085;
    font-family: var(--font-mono);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .filters {
    display: flex;
    gap: 4px;
  }
  .lv {
    font-family: var(--font-mono);
    font-size: 10.5px;
    font-weight: 700;
    letter-spacing: 0.03em;
    border: 1px solid transparent;
    border-radius: 6px;
    padding: 3px 8px;
    cursor: pointer;
    background: transparent;
  }
  .lv.err {
    color: #ff6b6b;
    border-color: color-mix(in srgb, #ff6b6b 40%, transparent);
  }
  .lv.warn {
    color: #f0b429;
    border-color: color-mix(in srgb, #f0b429 40%, transparent);
  }
  .lv.info {
    color: #56d4ac;
    border-color: color-mix(in srgb, #56d4ac 40%, transparent);
  }
  .lv.off {
    color: #55636a;
    border-color: transparent;
    background: #141b1e;
  }
  .termwrap {
    flex: 1;
    min-height: 0;
    min-width: 0;
    position: relative;
    overflow: hidden;
  }
  .term {
    position: absolute;
    inset: 0;
    overflow: hidden;
    padding: 8px 6px 8px 12px;
  }
  /* xterm ne défile jamais horizontalement (il wrappe) : on coupe toute scrollbar horizontale parasite. */
  .term :global(.xterm-viewport) {
    overflow-x: hidden !important;
  }
  .tobottom {
    position: absolute;
    right: 16px;
    bottom: 14px;
    width: 34px;
    height: 34px;
    border-radius: 50%;
    border: 1px solid #2a343a;
    background: #141b1e;
    color: #d6e2e6;
    font-size: 16px;
    cursor: pointer;
    display: grid;
    place-items: center;
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.4);
    z-index: 3;
  }
  .tobottom:hover {
    background: #1e262a;
    color: #fff;
  }
  .cmdbar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border-top: 1px solid #1c2429;
    flex: none;
    position: relative;
  }
  .suggest {
    position: absolute;
    left: 30px;
    bottom: calc(100% + 6px);
    display: flex;
    flex-direction: column;
    min-width: 190px;
    max-height: 240px;
    overflow-y: auto;
    background: #141b1e;
    border: 1px solid #2a343a;
    border-radius: 9px;
    box-shadow: 0 -8px 24px rgba(0, 0, 0, 0.45);
    padding: 4px;
    z-index: 6;
  }
  .sug {
    text-align: left;
    background: none;
    border: none;
    cursor: pointer;
    font: inherit;
    font-family: var(--font-mono);
    font-size: 12.5px;
    color: #d6e2e6;
    padding: 6px 10px;
    border-radius: 6px;
  }
  .sug:hover,
  .sug.on {
    background: #1e262a;
    color: #fff;
  }
  .prompt {
    font-family: var(--font-mono);
    color: var(--brand-primary);
    font-weight: 700;
  }
  .cmd {
    flex: 1;
    min-width: 0;
    background: #0f1417;
    border: 1px solid #1c2429;
    border-radius: 8px;
    color: #d6e2e6;
    font-family: var(--font-mono);
    font-size: 13px;
    padding: 9px 12px;
  }
  .cmd:focus {
    outline: none;
    border-color: var(--brand-primary);
  }
  .send {
    background: var(--brand-primary);
    color: #fff;
    border: none;
    border-radius: 8px;
    padding: 9px 16px;
    font: inherit;
    font-weight: 600;
    cursor: pointer;
  }
  .send:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  /* Menu contextuel console (analyse + export) + toast de retour */
  .cbackdrop {
    position: fixed;
    inset: 0;
    background: none;
    border: none;
    z-index: 40;
    cursor: default;
  }
  .cmenu {
    position: fixed;
    z-index: 41;
    min-width: 190px;
    background: #141b1e;
    border: 1px solid #2a343a;
    border-radius: 10px;
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.5);
    padding: 5px;
    display: flex;
    flex-direction: column;
  }
  .cmi {
    text-align: left;
    background: none;
    border: none;
    cursor: pointer;
    font: inherit;
    font-size: 13px;
    color: #d6e2e6;
    padding: 8px 11px;
    border-radius: 7px;
  }
  .cmi:hover {
    background: #1e262a;
  }
  .cmsep {
    height: 1px;
    background: #2a343a;
    margin: 5px 4px;
  }
  .cmlbl {
    font-size: 10.5px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: #6f8085;
    padding: 6px 11px 3px;
    font-family: var(--font-mono);
  }
  .ctoast {
    position: fixed;
    left: 50%;
    bottom: 72px;
    transform: translateX(-50%);
    background: #141b1e;
    border: 1px solid #2a343a;
    color: #d6e2e6;
    font-size: 12.5px;
    padding: 9px 16px;
    border-radius: 999px;
    box-shadow: 0 6px 20px rgba(0, 0, 0, 0.45);
    z-index: 45;
    max-width: 80vw;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
