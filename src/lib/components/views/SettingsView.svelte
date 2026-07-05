<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { api, humanizeError } from '$lib/ipc';
  import { t, getLocale, setLocale, LOCALES } from '$lib/i18n';
  import { toggleTheme } from '$lib/theme';
  import type {
    Autonomy,
    CliStatus,
    CopilotConfig,
    LlmProvider,
    McpConfig,
    PrivacyConfig,
    ServerListItem,
    SupervisorConfig,
  } from '$lib/types';
  import Icon from '../Icon.svelte';

  type Section = 'general' | 'supervisor' | 'mcp' | 'copilot' | 'privacy';
  let section = $state<Section>('general');

  let config = $state<SupervisorConfig | null>(null);
  let servers = $state<ServerListItem[]>([]);
  let mcp = $state<McpConfig | null>(null);
  let privacy = $state<PrivacyConfig | null>(null);

  // Catalogue d'outils IA (miroir de READ_TOOLS/WRITE_TOOLS) pour la case « permissions ».
  // Le libellé vient de l'i18n (clé `settings.tool_<name>`) ; ici on ne garde que name + write.
  const TOOL_CATALOG: { name: string; write: boolean }[] = [
    { name: 'list_servers', write: false },
    { name: 'server_status', write: false },
    { name: 'server_metrics', write: false },
    { name: 'read_console', write: false },
    { name: 'list_files', write: false },
    { name: 'read_file', write: false },
    { name: 'read_startup', write: false },
    { name: 'list_installed_mods', write: false },
    { name: 'list_installed_plugins', write: false },
    { name: 'market_search', write: false },
    { name: 'list_mod_versions', write: false },
    { name: 'power_action', write: true },
    { name: 'send_command', write: true },
    { name: 'player_action', write: true },
    { name: 'write_file', write: true },
    { name: 'create_dir', write: true },
    { name: 'delete_path', write: true },
    { name: 'rename_path', write: true },
    { name: 'set_startup_params', write: true },
    { name: 'install_mod', write: true },
  ];
  let exePath = $state('');
  let copied = $state(false);
  let copyTimer: ReturnType<typeof setTimeout> | undefined;
  let saved = $state(false);
  let savedTimer: ReturnType<typeof setTimeout> | undefined;
  let errorMsg = $state<string | null>(null);
  let errorTimer: ReturnType<typeof setTimeout> | undefined;

  // --- Copilote ---
  let copilot = $state<CopilotConfig | null>(null);
  let hasKey = $state(false);
  let keyInput = $state('');
  let keyBusy = $state(false);
  let testServer = $state<number | null>(null);
  let testSent = $state(false);
  let testTimer: ReturnType<typeof setTimeout> | undefined;

  // Préréglages OpenAI-compatible (remplissent URL + modèle ; la clé reste à saisir).
  const PRESETS: { label: string; base_url: string; model: string }[] = [
    { label: 'OpenAI', base_url: 'https://api.openai.com/v1', model: 'gpt-4o' },
    {
      label: 'Google Gemini',
      base_url: 'https://generativelanguage.googleapis.com/v1beta/openai',
      model: 'gemini-2.0-flash',
    },
    { label: 'Mistral', base_url: 'https://api.mistral.ai/v1', model: 'mistral-large-latest' },
    { label: 'Groq', base_url: 'https://api.groq.com/openai/v1', model: 'llama-3.3-70b-versatile' },
    { label: 'Ollama (local)', base_url: 'http://localhost:11434/v1', model: 'llama3.1' },
  ];

  const AUTONOMIES: { value: Autonomy; label: string; desc: string }[] = [
    { value: 'suggest_only', label: 'copilotSuggestOnly', desc: 'copilotSuggestOnlyDesc' },
    { value: 'apply_on_confirm', label: 'copilotApplyConfirm', desc: 'copilotApplyConfirmDesc' },
    { value: 'auto_safe', label: 'copilotAutoSafe', desc: 'copilotAutoSafeDesc' },
  ];

  // Agents CLI supportés (binaire par défaut + format de modèle attendu).
  const CLI_AGENTS: {
    value: 'claude_code' | 'open_code' | 'gemini';
    label: string;
    command: string;
    modelPlaceholder: string;
  }[] = [
    { value: 'claude_code', label: 'Claude Code', command: 'claude', modelPlaceholder: 'opus · sonnet · claude-opus-4-8…' },
    { value: 'open_code', label: 'OpenCode', command: 'opencode', modelPlaceholder: 'anthropic/claude-sonnet-4-5' },
    { value: 'gemini', label: 'Gemini CLI', command: 'gemini', modelPlaceholder: 'gemini-2.5-flash' },
  ];
  const currentCliAgent = $derived(CLI_AGENTS.find((a) => a.value === copilot?.cli_agent) ?? CLI_AGENTS[0]);
  // Détection des agents CLI installés (claude / opencode / gemini) → message si absent.
  let cliStatuses = $state<CliStatus[]>([]);
  let cliChecking = $state(false);
  const cliStatus = $derived(cliStatuses.find((s) => s.agent === copilot?.cli_agent));
  async function refreshClis() {
    cliChecking = true;
    try {
      cliStatuses = await api.detectClis();
    } catch {
      /* détection indisponible */
    } finally {
      cliChecking = false;
    }
  }
  const EFFORTS: { value: 'low' | 'medium' | 'high'; label: string }[] = [
    { value: 'low', label: 'effortLow' },
    { value: 'medium', label: 'effortMedium' },
    { value: 'high', label: 'effortHigh' },
  ];

  onMount(async () => {
    try {
      config = await api.getSupervisorConfig();
    } catch {
      /* défauts */
    }
    try {
      servers = (await api.listServers()).servers;
    } catch {
      /* liste indispo */
    }
    try {
      mcp = await api.getMcpConfig();
      exePath = await api.appExePath();
    } catch {
      /* défauts */
    }
    try {
      copilot = await api.getCopilotConfig();
      hasKey = await api.hasCopilotKey();
    } catch {
      /* défauts */
    }
    try {
      privacy = await api.getPrivacyConfig();
    } catch {
      /* défauts */
    }
    void refreshClis();
  });
  onDestroy(() => {
    clearTimeout(savedTimer);
    clearTimeout(errorTimer);
    clearTimeout(copyTimer);
    clearTimeout(testTimer);
  });

  function flashSaved() {
    saved = true;
    clearTimeout(savedTimer);
    savedTimer = setTimeout(() => (saved = false), 1500);
  }

  function flashError(e: unknown) {
    errorMsg = humanizeError(e);
    clearTimeout(errorTimer);
    errorTimer = setTimeout(() => (errorMsg = null), 4000);
  }

  async function save() {
    if (!config) return;
    try {
      await api.setSupervisorConfig($state.snapshot(config));
      flashSaved();
    } catch (e) {
      flashError(e);
    }
  }

  async function saveMcp() {
    if (!mcp) return;
    try {
      await api.setMcpConfig($state.snapshot(mcp));
      flashSaved();
    } catch (e) {
      flashError(e);
    }
  }

  const mcpSnippet = $derived(
    JSON.stringify(
      { mcpServers: { minestrator: { command: exePath, args: ['--mcp'] } } },
      null,
      2
    )
  );

  async function copySnippet() {
    try {
      await navigator.clipboard.writeText(mcpSnippet);
      copied = true;
      clearTimeout(copyTimer);
      copyTimer = setTimeout(() => (copied = false), 1500);
    } catch {
      /* clipboard indispo */
    }
  }

  function isMonitored(id: number): boolean {
    return config ? !config.disabled_servers.includes(id) : true;
  }
  function toggleMonitored(id: number) {
    if (!config) return;
    config.disabled_servers = config.disabled_servers.includes(id)
      ? config.disabled_servers.filter((x) => x !== id)
      : [...config.disabled_servers, id];
    void save();
  }

  // --- Copilote ---
  async function saveCopilot() {
    if (!copilot) return;
    try {
      await api.setCopilotConfig($state.snapshot(copilot));
      flashSaved();
    } catch (e) {
      flashError(e);
    }
  }

  async function savePrivacy() {
    if (!privacy) return;
    try {
      await api.setPrivacyConfig($state.snapshot(privacy));
      flashSaved();
    } catch (e) {
      flashError(e);
    }
  }

  function toolAllowed(name: string): boolean {
    return copilot ? !copilot.disabled_tools.includes(name) : true;
  }
  function toggleTool(name: string) {
    if (!copilot) return;
    copilot.disabled_tools = copilot.disabled_tools.includes(name)
      ? copilot.disabled_tools.filter((x) => x !== name)
      : [...copilot.disabled_tools, name];
    void saveCopilot();
  }

  async function setProvider(p: LlmProvider) {
    if (!copilot || copilot.provider === p) return;
    copilot.provider = p;
    if (p === 'anthropic') {
      copilot.base_url = '';
      copilot.model = 'claude-sonnet-5';
    } else if (p === 'local_cli') {
      // CLI : vide = modèle par défaut de l'agent (Claude Code) ; l'utilisateur peut le fixer.
      copilot.model = '';
    }
    await saveCopilot();
    // La clé est propre à chaque fournisseur.
    try {
      hasKey = await api.hasCopilotKey();
    } catch {
      hasKey = false;
    }
  }

  function applyPreset(p: (typeof PRESETS)[number]) {
    if (!copilot) return;
    copilot.base_url = p.base_url;
    copilot.model = p.model;
    void saveCopilot();
  }

  function applyCliPreset() {
    if (!copilot) return;
    copilot.cli_command = 'claude';
    copilot.cli_args = ['-p'];
    void saveCopilot();
  }

  function setCliAgent(a: 'claude_code' | 'open_code' | 'gemini') {
    if (!copilot) return;
    const meta = CLI_AGENTS.find((x) => x.value === a);
    copilot.cli_agent = a;
    copilot.cli_command = meta?.command ?? '';
    copilot.model = ''; // le format de modèle diffère selon l'agent → on repart de « défaut »
    void saveCopilot();
  }

  function setCliArgs(text: string) {
    if (!copilot) return;
    copilot.cli_args = text.trim() ? text.trim().split(/\s+/) : [];
    void saveCopilot();
  }

  async function saveKey() {
    if (!keyInput.trim()) return;
    keyBusy = true;
    try {
      await api.setCopilotKey(keyInput.trim());
      keyInput = '';
      hasKey = true;
      flashSaved();
    } catch (e) {
      flashError(e);
    } finally {
      keyBusy = false;
    }
  }

  async function clearKey() {
    keyBusy = true;
    try {
      await api.clearCopilotKey();
      hasKey = false;
    } catch (e) {
      flashError(e);
    } finally {
      keyBusy = false;
    }
  }

  function isCopilotOn(id: number): boolean {
    return copilot ? !copilot.disabled_servers.includes(id) : true;
  }
  function toggleCopilotServer(id: number) {
    if (!copilot) return;
    copilot.disabled_servers = copilot.disabled_servers.includes(id)
      ? copilot.disabled_servers.filter((x) => x !== id)
      : [...copilot.disabled_servers, id];
    void saveCopilot();
  }

  async function runTest() {
    if (testServer == null) return;
    const srv = servers.find((s) => s.id === testServer);
    try {
      await api.copilotDiagnoseNow(testServer, srv?.name ?? `#${testServer}`);
      testSent = true;
      clearTimeout(testTimer);
      testTimer = setTimeout(() => (testSent = false), 4000);
    } catch (e) {
      flashError(e);
    }
  }
</script>

<div class="settings">
  <nav class="nav">
    <button class="nav-item" class:on={section === 'general'} onclick={() => (section = 'general')}>
      {t('settings.general')}
    </button>
    <button
      class="nav-item"
      class:on={section === 'supervisor'}
      onclick={() => (section = 'supervisor')}
    >
      {t('settings.supervisor')}
    </button>
    <button class="nav-item" class:on={section === 'mcp'} onclick={() => (section = 'mcp')}>
      {t('settings.mcp')}
    </button>
    <button
      class="nav-item"
      class:on={section === 'copilot'}
      onclick={() => (section = 'copilot')}
    >
      {t('settings.copilot')}
    </button>
    <button
      class="nav-item"
      class:on={section === 'privacy'}
      onclick={() => (section = 'privacy')}
    >
      {t('settings.privacy')}
    </button>
  </nav>

  <div class="content">
    {#if section === 'general'}
      <h1>{t('settings.general')}</h1>
      <div class="row">
        <span class="label">{t('common.language')}</span>
        <div class="langs">
          {#each LOCALES as loc (loc.code)}
            <button
              class="chip"
              class:on={getLocale() === loc.code}
              onclick={() => setLocale(loc.code)}
            >
              {loc.label}
            </button>
          {/each}
        </div>
      </div>
      <div class="row">
        <span class="label">{t('common.theme')}</span>
        <button class="btn btn--ghost" onclick={() => toggleTheme()}>{t('common.theme')}</button>
      </div>
    {:else if section === 'supervisor' && config}
      <h1>{t('settings.supervisor')}</h1>

      <label class="toggle-row">
        <div class="tl">
          <div class="tl-title">{t('settings.supervisorEnabled')}</div>
          <div class="tl-desc dim">{t('settings.supervisorDesc')}</div>
        </div>
        <input type="checkbox" bind:checked={config.enabled} onchange={save} />
      </label>

      <div class="sub" class:off={!config.enabled}>
        <label class="toggle-row">
          <div class="tl-title">{t('settings.crashDetection')}</div>
          <input type="checkbox" bind:checked={config.crash_detection} onchange={save} />
        </label>
        <label class="toggle-row">
          <div class="tl-title">{t('settings.expiryAlerts')}</div>
          <input type="checkbox" bind:checked={config.expiry_alerts} onchange={save} />
        </label>

        <h2>{t('settings.thresholds')}</h2>
        <div class="thresholds">
          <label class="th">
            <span>{t('overview.cpu')}</span>
            <input type="number" min="1" max="100" bind:value={config.cpu_threshold} onchange={save} />
          </label>
          <label class="th">
            <span>{t('overview.ram')}</span>
            <input type="number" min="1" max="100" bind:value={config.ram_threshold} onchange={save} />
          </label>
          <label class="th">
            <span>{t('overview.disk')}</span>
            <input type="number" min="1" max="100" bind:value={config.disk_threshold} onchange={save} />
          </label>
        </div>

        <h2>{t('settings.perServer')}</h2>
        <p class="desc dim">{t('settings.perServerDesc')}</p>
        <div class="servers">
          {#each servers as s (s.id)}
            <label class="srv-row">
              <input
                type="checkbox"
                checked={isMonitored(s.id)}
                onchange={() => toggleMonitored(s.id)}
              />
              <span class="srv-name">{s.name}</span>
            </label>
          {/each}
        </div>
      </div>
    {:else if section === 'mcp' && mcp}
      <h1>{t('settings.mcp')}</h1>

      <label class="toggle-row">
        <div class="tl">
          <div class="tl-title">{t('settings.mcpEnabled')}</div>
          <div class="tl-desc dim">{t('settings.mcpDesc')}</div>
        </div>
        <input type="checkbox" bind:checked={mcp.enabled} onchange={saveMcp} />
      </label>

      <div class="sub" class:off={!mcp.enabled}>
        <label class="toggle-row">
          <div class="tl">
            <div class="tl-title">{t('settings.mcpAllowWrites')}</div>
            <div class="tl-desc dim">{t('settings.mcpAllowWritesDesc')}</div>
          </div>
          <input type="checkbox" bind:checked={mcp.allow_writes} onchange={saveMcp} />
        </label>
      </div>

      <h2>{t('settings.mcpConnect')}</h2>
      <p class="desc dim">{t('settings.mcpConfigHint')}</p>
      <div class="snippet">
        <pre>{mcpSnippet}</pre>
        <button class="btn btn--ghost copy" onclick={copySnippet}>
          {copied ? t('settings.copied') : t('settings.copy')}
        </button>
      </div>
      <p class="cmd dim">{t('settings.mcpCommand')} <code>{exePath} --mcp</code></p>
    {:else if section === 'copilot' && copilot}
      <h1>{t('settings.copilot')}</h1>

      <label class="toggle-row">
        <div class="tl">
          <div class="tl-title">{t('settings.copilotEnabled')}</div>
          <div class="tl-desc dim">{t('settings.copilotDesc')}</div>
        </div>
        <input type="checkbox" bind:checked={copilot.enabled} onchange={saveCopilot} />
      </label>

      <div class="sub" class:off={!copilot.enabled}>
        <h2>{t('settings.copilotProvider')}</h2>
        <div class="chips-row">
          <button
            class="chip"
            class:on={copilot.provider === 'anthropic'}
            onclick={() => setProvider('anthropic')}
          >
            {t('settings.copilotProviderAnthropic')}
          </button>
          <button
            class="chip"
            class:on={copilot.provider === 'openai_compatible'}
            onclick={() => setProvider('openai_compatible')}
          >
            {t('settings.copilotProviderOpenai')}
          </button>
          <button
            class="chip"
            class:on={copilot.provider === 'local_cli'}
            onclick={() => setProvider('local_cli')}
          >
            {t('settings.copilotProviderCli')}
          </button>
        </div>

        {#if copilot.provider === 'local_cli'}
          <p class="desc dim" style="margin-top:12px">{t('settings.copilotCliDesc')}</p>

          <h2>{t('settings.cliAgent')}</h2>
          <div class="effort-seg">
            {#each CLI_AGENTS as a (a.value)}
              <button class:on={copilot.cli_agent === a.value} onclick={() => setCliAgent(a.value)}>
                {a.label}
              </button>
            {/each}
          </div>
          {#if copilot.cli_agent === 'open_code'}
            <p class="cmd dim">{t('settings.cliAgentOpenCode')}</p>
          {:else if copilot.cli_agent === 'gemini'}
            <p class="cmd dim">{t('settings.cliAgentGemini')}</p>
          {:else}
            <p class="cmd dim">{t('settings.cliAgentClaude')}</p>
          {/if}

          <div class="cli-detect">
            {#if cliChecking && cliStatuses.length === 0}
              <span class="dim">{t('settings.cliChecking')}</span>
            {:else if cliStatus?.available}
              <span class="ok"
                >✓ {t('settings.cliDetected')}{cliStatus.version ? ` · ${cliStatus.version}` : ''}</span
              >
            {:else}
              <span class="warn">⚠ {t('settings.cliMissing', { cmd: currentCliAgent.command })}</span>
            {/if}
            <button
              class="recheck"
              onclick={refreshClis}
              disabled={cliChecking}
              title={t('common.refresh')}
              aria-label={t('common.refresh')}
            >
              <Icon name="refresh-cw" size={12} />
            </button>
          </div>

          <h2>{t('settings.copilotCliCommand')}</h2>
          <input
            class="txt"
            type="text"
            bind:value={copilot.cli_command}
            onchange={saveCopilot}
            placeholder={currentCliAgent.command}
            spellcheck="false"
          />
          <p class="cmd dim">{t('settings.copilotCliCommandHint')}</p>

          <h2>{t('settings.copilotModel')}</h2>
          <input
            class="txt"
            type="text"
            bind:value={copilot.model}
            onchange={saveCopilot}
            placeholder={currentCliAgent.modelPlaceholder}
            spellcheck="false"
          />
          <p class="cmd dim">{t('settings.cliModelHint')}</p>

          <label class="toggle-row">
            <div class="tl">
              <div class="tl-title">{t('settings.copilotCliAgentic')}</div>
              <div class="tl-desc dim">{t('settings.copilotCliAgenticDesc')}</div>
            </div>
            <input type="checkbox" bind:checked={copilot.cli_agentic} onchange={saveCopilot} />
          </label>

          {#if !copilot.cli_agentic}
            <h2>{t('settings.copilotCliArgs')}</h2>
            <input
              class="txt"
              type="text"
              value={copilot.cli_args.join(' ')}
              onchange={(e) => setCliArgs(e.currentTarget.value)}
              placeholder="-p"
              spellcheck="false"
            />
            <p class="cmd dim">{t('settings.copilotCliArgsHint')}</p>
          {/if}
        {:else}
          {#if copilot.provider === 'openai_compatible'}
            <div class="preset">
              <span class="preset-label dim">{t('settings.copilotPreset')} :</span>
              {#each PRESETS as p (p.label)}
                <button class="chip small" onclick={() => applyPreset(p)}>{p.label}</button>
              {/each}
            </div>
          {/if}

          <h2>{t('settings.copilotModel')}</h2>
          <input
            class="txt"
            type="text"
            bind:value={copilot.model}
            onchange={saveCopilot}
            spellcheck="false"
          />

          <h2>{t('settings.copilotBaseUrl')}</h2>
          <input
            class="txt"
            type="text"
            bind:value={copilot.base_url}
            onchange={saveCopilot}
            placeholder={copilot.provider === 'anthropic'
              ? 'https://api.anthropic.com/v1'
              : 'https://api.openai.com/v1'}
            spellcheck="false"
          />
          <p class="cmd dim">{t('settings.copilotBaseUrlHint')}</p>

          <h2>{t('settings.copilotKey')}</h2>
          <div class="keyrow">
            <input
              class="txt"
              type="password"
              placeholder={t('settings.copilotKeyPlaceholder')}
              bind:value={keyInput}
              spellcheck="false"
            />
            <button
              class="btn btn--ghost"
              onclick={saveKey}
              disabled={keyBusy || !keyInput.trim()}
            >
              {t('settings.copilotSaveKey')}
            </button>
          </div>
          {#if hasKey}
            <p class="cmd dim">
              <Icon name="check" size={13} /> {t('settings.copilotKeySet')} ·
              <button class="linkbtn" onclick={clearKey} disabled={keyBusy}
                >{t('settings.copilotClearKey')}</button
              >
            </p>
          {:else}
            <p class="cmd dim">{t('settings.copilotKeyOptional')}</p>
          {/if}
        {/if}

        <h2>{t('settings.copilotAutonomy')}</h2>
        <div class="autos">
          {#each AUTONOMIES as a (a.value)}
            <button
              class="auto-card"
              class:on={copilot.autonomy === a.value}
              onclick={() => {
                copilot!.autonomy = a.value;
                void saveCopilot();
              }}
            >
              <div class="auto-title">{t(`settings.${a.label}`)}</div>
              <div class="auto-desc dim">{t(`settings.${a.desc}`)}</div>
            </button>
          {/each}
        </div>

        <h2>{t('settings.effort')}</h2>
        <div class="effort-seg">
          {#each EFFORTS as e (e.value)}
            <button
              class:on={copilot.effort === e.value}
              onclick={() => {
                copilot!.effort = e.value;
                void saveCopilot();
              }}
            >
              {t(`settings.${e.label}`)}
            </button>
          {/each}
        </div>
        <p class="cmd dim">{t('settings.effortHint')}</p>

        <h2>{t('settings.aiWebSearch')}</h2>
        <label class="toggle-row">
          <div class="tl">
            <div class="tl-title">{t('settings.aiWebSearchTitle')}</div>
            <div class="tl-desc dim">{t('settings.aiWebSearchDesc')}</div>
          </div>
          <input type="checkbox" bind:checked={copilot.web_search} onchange={saveCopilot} />
        </label>

        <h2>{t('settings.aiTools')}</h2>
        <p class="desc dim">{t('settings.aiToolsHint')}</p>
        <div class="tools">
          {#each TOOL_CATALOG as tool (tool.name)}
            <label class="tool-row">
              <input
                type="checkbox"
                checked={toolAllowed(tool.name)}
                onchange={() => toggleTool(tool.name)}
              />
              <span class="tool-name">{t(`settings.tool_${tool.name}`)}</span>
              <code class="tool-id dim">{tool.name}</code>
              {#if tool.write}<span class="tool-badge">{t('settings.aiToolWrite')}</span>{/if}
            </label>
          {/each}
        </div>

        <h2>{t('settings.copilotTriggers')}</h2>
        <label class="toggle-row">
          <div class="tl-title">{t('settings.copilotOnCrash')}</div>
          <input type="checkbox" bind:checked={copilot.on_crash} onchange={saveCopilot} />
        </label>
        <label class="toggle-row">
          <div class="tl-title">{t('settings.copilotOnThreshold')}</div>
          <input type="checkbox" bind:checked={copilot.on_threshold} onchange={saveCopilot} />
        </label>
        <label class="toggle-row">
          <div class="tl-title">{t('settings.copilotOnError')}</div>
          <input type="checkbox" bind:checked={copilot.on_error} onchange={saveCopilot} />
        </label>
        <label class="toggle-row">
          <div class="tl-title">{t('settings.copilotOnWarn')}</div>
          <input type="checkbox" bind:checked={copilot.on_warn} onchange={saveCopilot} />
        </label>
        <label class="toggle-row">
          <div class="tl">
            <div class="tl-title">{t('settings.copilotPerfOverload')}</div>
            <div class="tl-desc dim">{t('settings.copilotPerfOverloadDesc')}</div>
          </div>
          <input type="checkbox" bind:checked={copilot.perf_on_overload} onchange={saveCopilot} />
        </label>
        {#if copilot.perf_on_overload}
          <div class="thresholds">
            <label class="th">
              <span>{t('settings.copilotPerfPct')}</span>
              <input
                type="number"
                min="1"
                max="100"
                bind:value={copilot.perf_overload_pct}
                onchange={saveCopilot}
              />
            </label>
            <label class="th">
              <span>{t('settings.copilotPerfMinutes')}</span>
              <input
                type="number"
                min="1"
                max="60"
                bind:value={copilot.perf_overload_minutes}
                onchange={saveCopilot}
              />
            </label>
          </div>
        {/if}
        <p class="cmd dim">{t('settings.copilotLogHint')}</p>

        <h2>{t('settings.perServer')}</h2>
        <div class="servers">
          {#each servers as s (s.id)}
            <label class="srv-row">
              <input
                type="checkbox"
                checked={isCopilotOn(s.id)}
                onchange={() => toggleCopilotServer(s.id)}
              />
              <span class="srv-name">{s.name}</span>
            </label>
          {/each}
        </div>

        <h2>{t('settings.copilotTest')}</h2>
        <p class="desc dim">{t('settings.copilotTestHint')}</p>
        <div class="testrow">
          <select class="txt" bind:value={testServer}>
            <option value={null} disabled selected>—</option>
            {#each servers as s (s.id)}
              <option value={s.id}>{s.name}</option>
            {/each}
          </select>
          <button class="btn btn--ghost" onclick={runTest} disabled={testServer == null}>
            {t('settings.copilotTestRun')}
          </button>
        </div>
        {#if testSent}<p class="cmd dim">{t('settings.copilotTestSent')}</p>{/if}
      </div>
    {:else if section === 'privacy' && privacy}
      <h1>{t('settings.privacy')}</h1>
      <p class="desc dim">{t('settings.privacyIntro')}</p>

      <label class="toggle-row">
        <div class="tl">
          <div class="tl-title">{t('settings.privacyAi')}</div>
          <div class="tl-desc dim">{t('settings.privacyAiDesc')}</div>
        </div>
        <input type="checkbox" bind:checked={privacy.redact_ai} onchange={savePrivacy} />
      </label>

      <label class="toggle-row">
        <div class="tl">
          <div class="tl-title">{t('settings.privacyConsole')}</div>
          <div class="tl-desc dim">{t('settings.privacyConsoleDesc')}</div>
        </div>
        <input type="checkbox" bind:checked={privacy.redact_console} onchange={savePrivacy} />
      </label>

      <p class="cmd dim">{t('settings.privacyNote')}</p>
    {/if}

    {#if saved}<div class="saved-toast">{t('settings.saved')}</div>{/if}
    {#if errorMsg}<div class="saved-toast error-toast">{errorMsg}</div>{/if}
  </div>
</div>

<style>
  .settings {
    display: flex;
    height: 100%;
    min-height: 0;
  }
  .nav {
    flex: none;
    width: 190px;
    border-right: 1px solid var(--border);
    padding: 18px 10px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .nav-item {
    text-align: left;
    background: none;
    border: none;
    cursor: pointer;
    font: inherit;
    font-size: 14px;
    color: var(--text-muted);
    padding: 9px 12px;
    border-radius: var(--radius);
  }
  .nav-item:hover {
    color: var(--text);
    background: color-mix(in srgb, var(--text) 5%, transparent);
  }
  .nav-item.on {
    color: var(--text);
    background: color-mix(in srgb, var(--brand-primary) 14%, transparent);
    font-weight: 600;
  }
  .content {
    flex: 1;
    min-width: 0;
    overflow: auto;
    padding: 26px 28px 48px;
    position: relative;
    max-width: 640px;
  }
  h1 {
    margin: 0 0 22px;
    font-size: 22px;
    letter-spacing: -0.02em;
  }
  h2 {
    margin: 26px 0 12px;
    font-size: 13px;
    text-transform: uppercase;
    letter-spacing: 0.09em;
    color: var(--text-muted);
    font-family: var(--font-mono);
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 12px 0;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
  }
  .label {
    font-size: 14px;
    font-weight: 500;
  }
  .langs {
    display: flex;
    gap: 6px;
  }
  .chip {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 8px;
    cursor: pointer;
    font: inherit;
    font-size: 13px;
    color: var(--text-muted);
    padding: 6px 12px;
  }
  .chip.on {
    color: #fff;
    background: var(--brand-primary);
    border-color: transparent;
  }
  .toggle-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 14px 0;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 60%, transparent);
    cursor: pointer;
  }
  .tl {
    min-width: 0;
  }
  .tl-title {
    font-size: 14px;
    font-weight: 500;
  }
  .tl-desc {
    font-size: 12.5px;
    margin-top: 3px;
    max-width: 46ch;
  }
  .sub {
    transition: opacity 0.15s ease;
  }
  .sub.off {
    opacity: 0.45;
    pointer-events: none;
  }
  .thresholds {
    display: flex;
    gap: 14px;
    flex-wrap: wrap;
  }
  .th {
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: 12px;
    color: var(--text-muted);
  }
  .th input {
    width: 90px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    color: var(--text);
    font: inherit;
    font-family: var(--font-mono);
    padding: 8px 10px;
  }
  .th input:focus {
    outline: none;
    border-color: var(--brand-primary);
  }
  .desc {
    font-size: 13px;
    margin: 0 0 10px;
  }
  .servers {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
  }
  .srv-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 55%, transparent);
    cursor: pointer;
  }
  .srv-row:last-child {
    border-bottom: none;
  }
  .srv-name {
    font-size: 13.5px;
  }
  .tools {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
  }
  .tool-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 55%, transparent);
    cursor: pointer;
  }
  .tool-row:last-child {
    border-bottom: none;
  }
  .effort-seg {
    display: inline-flex;
    gap: 2px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 3px;
  }
  .effort-seg button {
    background: none;
    border: none;
    cursor: pointer;
    font: inherit;
    font-size: 13px;
    color: var(--text-muted);
    padding: 6px 16px;
    border-radius: 7px;
  }
  .effort-seg button.on {
    background: var(--brand-primary);
    color: #fff;
    font-weight: 600;
  }
  .tool-name {
    font-size: 13px;
  }
  .tool-id {
    font-family: var(--font-mono);
    font-size: 11px;
  }
  .tool-badge {
    margin-left: auto;
    font-size: 9.5px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--state-pending);
    border: 1px solid color-mix(in srgb, var(--state-pending) 40%, var(--border));
    border-radius: 4px;
    padding: 1px 6px;
  }
  input[type='checkbox'] {
    width: 17px;
    height: 17px;
    accent-color: var(--brand-primary);
    flex: none;
    cursor: pointer;
  }
  .snippet {
    position: relative;
  }
  .snippet pre {
    margin: 0;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 14px 16px;
    overflow-x: auto;
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.6;
    color: var(--text-muted);
  }
  .snippet .copy {
    position: absolute;
    top: 8px;
    right: 8px;
    padding: 5px 12px;
    font-size: 12px;
  }
  .cmd {
    font-size: 12.5px;
    margin: 10px 0 0;
  }
  .cli-detect {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 10px;
    font-size: 12.5px;
  }
  .cli-detect .ok {
    color: var(--state-running);
  }
  .cli-detect .warn {
    color: var(--state-danger);
  }
  .cli-detect .recheck {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: none;
    border: none;
    color: var(--text-dim);
    cursor: pointer;
    padding: 2px;
    border-radius: 5px;
  }
  .cli-detect .recheck:hover:not(:disabled) {
    color: var(--text);
    background: color-mix(in srgb, var(--text) 8%, transparent);
  }
  .cli-detect .recheck:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .cmd code {
    font-family: var(--font-mono);
    font-size: 12px;
    background: var(--elevated);
    padding: 2px 7px;
    border-radius: 5px;
    color: var(--text);
    word-break: break-all;
  }
  .txt {
    width: 100%;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    color: var(--text);
    font: inherit;
    font-family: var(--font-mono);
    font-size: 13px;
    padding: 9px 11px;
  }
  .txt:focus {
    outline: none;
    border-color: var(--brand-primary);
  }
  .chips-row {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .chip.small {
    font-size: 12px;
    padding: 5px 10px;
  }
  .preset {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    margin-top: 10px;
  }
  .preset-label {
    font-size: 12px;
  }
  .keyrow,
  .testrow {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .keyrow .txt,
  .testrow .txt {
    flex: 1;
  }
  .keyrow .btn,
  .testrow .btn {
    flex: none;
  }
  .linkbtn {
    background: none;
    border: none;
    color: var(--brand-primary);
    cursor: pointer;
    font: inherit;
    font-size: 12.5px;
    padding: 0;
    text-decoration: underline;
  }
  .linkbtn:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .autos {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .auto-card {
    text-align: left;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    cursor: pointer;
    font: inherit;
    padding: 11px 13px;
    color: var(--text);
  }
  .auto-card:hover {
    border-color: color-mix(in srgb, var(--brand-primary) 40%, var(--border));
  }
  .auto-card.on {
    border-color: var(--brand-primary);
    background: color-mix(in srgb, var(--brand-primary) 10%, transparent);
  }
  .auto-title {
    font-size: 13.5px;
    font-weight: 600;
  }
  .auto-desc {
    font-size: 12px;
    margin-top: 3px;
  }
  .saved-toast {
    position: fixed;
    bottom: 20px;
    right: 24px;
    background: var(--elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 9px 15px;
    font-size: 13px;
    color: var(--brand-primary);
    box-shadow: var(--shadow);
    z-index: 5;
  }
  .error-toast {
    color: var(--state-danger);
    border-color: color-mix(in srgb, var(--state-danger) 40%, var(--border));
    max-width: 320px;
  }
</style>
