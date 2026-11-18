<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { Terminal } from '@xterm/xterm';
  import { FitAddon } from '@xterm/addon-fit';
  import { WebLinksAddon } from '@xterm/addon-web-links';
  import '@xterm/xterm/css/xterm.css';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { api, humanizeError } from '#lib/ipc.ts';
  import { consoleEvents } from '#lib/events.ts';
  import { openCopilotMenu } from '#lib/copilot/menu.svelte.ts';
  import { PASTE_SERVICES, pasteExport } from '#lib/paste.ts';
  import { redactLine } from '#lib/redact.ts';
  import { t } from '#lib/i18n/index.ts';
  import { isRunning } from '#lib/status.ts';
  import { serverCaps, serverItem } from '#lib/games/capabilities.svelte.ts';
  import { isFake, fakeText } from '#lib/fake.svelte.ts';
  import PowerControl from '../PowerControl.svelte';
  import Icon from '../Icon.svelte';
  import type { ServerTab } from '#lib/tabs/tabs.svelte.ts';

  let { tab }: { tab: ServerTab } = $props();
  const serverId = $derived(tab.serverId);
  const connId = $derived(tab.id);
  // Autocomplétion de commandes seulement pour Minecraft (le catalogue MC n'a pas de sens ailleurs).
  const mcAutocomplete = $derived(
    serverCaps(serverId)?.console_autocomplete === 'minecraft_java' ||
      serverCaps(serverId)?.console_autocomplete === 'minecraft_bedrock'
  );
  // Format des logs (détection des niveaux → filtres + couleurs), selon le jeu.
  const logFormat = $derived(serverCaps(serverId)?.log_format ?? 'generic');

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
      // 1er mot : nom de commande (catalogue MC si applicable + historique)
      if (!frag) return [];
      const hist = history.map((h) => h.split(' ')[0]);
      const catalog = mcAutocomplete ? MC_COMMANDS : [];
      return [...new Set([...catalog, ...hist])]
        .filter((x) => x.toLowerCase().startsWith(frag) && x.toLowerCase() !== frag)
        .slice(0, 8);
    }
    // 1er argument d'une commande « joueur » : joueurs connectés (frag vide = tous)
    if (parts.length === 2 && mcAutocomplete && PLAYER_CMDS.has(parts[0].toLowerCase())) {
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
      : phase === 'suspended'
        ? 'var(--state-danger)'
        : phase === 'closed' || phase === 'hibernated'
          ? 'var(--state-offline)'
          : 'var(--state-pending)'
  );

  // Détection du niveau selon le format de log du jeu (source : capacités).
  function levelOf(line: string): Level | 'other' {
    switch (logFormat) {
      case 'unreal':
        // Logs Unreal (Satisfactory) : « LogX: Error/Warning/Display: … », « Fatal error: ».
        if (/\b(Error|Fatal)\b/.test(line)) return 'error';
        if (/\bWarning\b/.test(line)) return 'warn';
        if (/\bDisplay\b/.test(line)) return 'info';
        return 'other';
      case 'minecraft':
        if (/\b(ERROR|SEVERE|FATAL)\b/.test(line)) return 'error';
        if (/\bWARN(?:ING)?\b/.test(line)) return 'warn';
        if (/\bINFO\b/.test(line)) return 'info';
        return 'other';
      default:
        // Générique : insensible à la casse.
        if (/\b(error|fatal|severe)\b/i.test(line)) return 'error';
        if (/\bwarn(?:ing)?\b/i.test(line)) return 'warn';
        if (/\binfo\b/i.test(line)) return 'info';
        return 'other';
    }
  }
  // Colorie une ligne selon son niveau (rouge/jaune) — seulement si le serveur ne l'a pas déjà
  // colorée (pas d'ANSI) : les erreurs/avertissements ressortent même dans des logs bruts.
  function colorize(line: string, level: Level | 'other'): string {
    if (line.includes('\u001b')) return line;
    if (level === 'error') return `\u001b[91m${line}\u001b[0m`;
    if (level === 'warn') return `\u001b[93m${line}\u001b[0m`;
    return line;
  }
  function pushLine(line: string) {
    if (redactConsole) line = redactLine(line);
    // Mode démo : la console est le flux le plus bavard en pseudos et adresses. Appliqué après
    // l'anonymisation de confidentialité, qui masque, là où celui-ci remplace par du plausible.
    if (isFake()) line = fakeText(line);
    buffer.push(line);
    if (buffer.length > MAX_BUFFER) buffer.shift();
    const lvl = levelOf(line);
    if (lvl === 'other' || filter[lvl]) term?.writeln(colorize(line, lvl));
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
    for (const line of buffer) {
      const lvl = levelOf(line);
      if (lvl === 'other' || filter[lvl]) term.writeln(colorize(line, lvl));
    }
  }
  function toggleLevel(l: Level) {
    filter[l] = !filter[l]; // le re-rendu est piloté par l'effet réactif ci-dessous
  }
  // Re-classe/re-colorie tout le tampon quand un filtre change OU quand le format de log (capacités)
  // arrive après les premières lignes. Les lignes live, elles, sont écrites directement par pushLine.
  $effect(() => {
    rerender();
  });

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
    // La police embarquée doit être chargée avant que xterm mesure ses cellules, sinon il
    // les calcule sur la police de repli et le texte se décale une fois la vraie police prête.
    try {
      await document.fonts.load('13px "JetBrains Mono Variable"');
    } catch {
      /* police indisponible : xterm prend la police de repli */
    }
    if (destroyed) return;
    term = new Terminal({
      convertEol: true,
      cursorBlink: false,
      disableStdin: true,
      fontFamily: '"JetBrains Mono Variable", "JetBrains Mono", ui-monospace, "Cascadia Code", Consolas, monospace',
      fontSize: 13,
      scrollback: MAX_BUFFER,
      // Canvas : les variables CSS n'y passent pas, d'où les valeurs de la charte en dur
      // (fond de la barre d'onglets, texte, vert « en ligne »).
      theme: {
        background: '#0d1113',
        foreground: '#e4ebe8',
        cursor: '#5fd68c',
        selectionBackground: '#1f4a33',
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

  /**
   * Coller multi-ligne = **liste de commandes** : chaque ligne non vide est envoyée séparément et
   * **dans l'ordre** (on attend l'envoi de chacune pour préserver l'ordre côté serveur), au lieu
   * d'être aplatie en une seule commande par l'input mono-ligne. Un collage d'une seule commande
   * garde le comportement normal (remplit le champ, l'utilisateur valide).
   */
  async function onPaste(event: ClipboardEvent) {
    const text = event.clipboardData?.getData('text') ?? '';
    const lines = text
      .split(/\r?\n/)
      .map((l) => l.trim())
      .filter((l) => l.length > 0);
    if (lines.length <= 1) return; // 0-1 commande effective → coller normal
    event.preventDefault();
    sugDismissed = true;
    sugIndex = -1;
    for (const line of lines) {
      await api.sendCommand(serverId, line).catch(() => {});
      history.push(line);
    }
    histIndex = history.length;
    showToast(t('console.batchSent', { n: lines.length }), 3000);
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

<div class="console force-dark">
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
      <PowerControl
        serverId={serverId}
        running={running}
        disabled={phase === 'hibernated' || phase === 'suspended'}
        restartWarning={serverItem(serverId)?.restart_warning}
        stopWarning={serverItem(serverId)?.stop_warning}
        compact
      />
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
      onpaste={onPaste}
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
    background: var(--chrome);
    overflow: hidden;
  }
  .statusbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 14px;
    border-bottom: 1px solid var(--border);
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
    color: var(--text-muted);
    flex: none;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }
  .srv {
    font-size: 12px;
    color: var(--text-dim);
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
    font-weight: 800;
    letter-spacing: 0.03em;
    border: 1px solid transparent;
    border-radius: 6px;
    padding: 3px 8px;
    cursor: pointer;
    background: transparent;
  }
  .lv.err {
    color: var(--state-danger);
    border-color: color-mix(in srgb, var(--state-danger) 40%, transparent);
  }
  .lv.warn {
    color: var(--state-pending);
    border-color: color-mix(in srgb, var(--state-pending) 40%, transparent);
  }
  .lv.info {
    color: var(--state-running);
    border-color: color-mix(in srgb, var(--state-running) 40%, transparent);
  }
  .lv.off {
    color: var(--state-offline);
    border-color: transparent;
    background: var(--inset);
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
    border: 1px solid var(--border-strong);
    background: var(--surface);
    color: var(--text);
    font-size: 16px;
    cursor: pointer;
    display: grid;
    place-items: center;
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.4);
    z-index: 3;
  }
  .tobottom:hover {
    background: var(--surface);
    color: var(--text);
  }
  .cmdbar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border-top: 1px solid var(--border);
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
    background: var(--elevated);
    border: 1px solid var(--border-strong);
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
    color: var(--text);
    padding: 6px 10px;
    border-radius: 6px;
  }
  .sug:hover,
  .sug.on {
    background: var(--surface);
    color: var(--text);
  }
  .prompt {
    font-family: var(--font-mono);
    color: var(--brand-primary);
    font-weight: 800;
  }
  .cmd {
    flex: 1;
    min-width: 0;
    background: var(--inset);
    border: 1px solid var(--border);
    border-radius: 8px;
    color: var(--text);
    font-family: var(--font-mono);
    font-size: 13px;
    padding: 9px 12px;
  }
  .cmd:focus {
    outline: none;
    border-color: var(--brand-primary);
  }
  .send {
    background: var(--btn-primary-bg);
    color: var(--btn-primary-ink);
    border: none;
    border-radius: 8px;
    padding: 9px 16px;
    font: inherit;
    font-weight: 800;
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
    background: var(--elevated);
    border: 1px solid var(--border-strong);
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
    color: var(--text);
    padding: 8px 11px;
    border-radius: 7px;
  }
  .cmi:hover {
    background: var(--surface);
  }
  .cmsep {
    height: 1px;
    background: var(--border-strong);
    margin: 5px 4px;
  }
  .cmlbl {
    font-size: 10.5px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-dim);
    padding: 6px 11px 3px;
    font-family: var(--font-mono);
  }
  .ctoast {
    position: fixed;
    left: 50%;
    bottom: 72px;
    transform: translateX(-50%);
    background: var(--elevated);
    border: 1px solid var(--border-strong);
    color: var(--text);
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
