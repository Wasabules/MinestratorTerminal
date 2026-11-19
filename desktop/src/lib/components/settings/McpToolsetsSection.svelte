<script lang="ts">
  /**
   * Groupes d'outils exposés aux clients MCP (Réglages → MCP) : tout, ou une sélection.
   * Réglage `McpConfig.toolsets` (vide = tout) ; `--toolsets` et `MINESTRATOR_MCP_TOOLSETS`
   * priment côté client. `core` (identifiants des serveurs) reste toujours actif.
   * La liste doit suivre `TOOLSETS` de crates/minestrator-core/src/mcp_catalog.rs (un test du cœur
   * le vérifie).
   */
  import { t } from '#lib/i18n/index.ts';
  import type { McpConfig } from '#lib/types.ts';

  const GROUPS = [
    'core',
    'console',
    'files',
    'config',
    'content',
    'players',
    'backups',
    'diagnostics',
    'mybox',
    'schedules',
    'fleet',
    'agent',
    'build',
    'guard',
    'official',
  ] as const;
  type Group = (typeof GROUPS)[number];
  const ALWAYS_ON: Group = 'core';

  let { config = $bindable(), onsave }: { config: McpConfig; onsave: () => void } = $props();

  const all = $derived(config.toolsets.length === 0);

  function setAll(value: boolean) {
    if (value === all) return;
    // Passage en sélection : tout coché au départ, rien ne change tant qu'on ne décoche pas.
    config.toolsets = value ? [] : [...GROUPS];
    onsave();
  }

  function enabled(g: Group): boolean {
    return all || g === ALWAYS_ON || config.toolsets.includes(g);
  }

  function toggle(g: Group) {
    if (g === ALWAYS_ON) return;
    const next = config.toolsets.includes(g)
      ? config.toolsets.filter((x) => x !== g)
      : [...config.toolsets, g];
    // Garder au moins `core` : une liste vide voudrait dire « tout ».
    config.toolsets = next.length === 0 ? [ALWAYS_ON] : next;
    onsave();
  }
</script>

<h2>{t('mcpToolsets.title')}</h2>
<p class="desc dim">{t('mcpToolsets.desc')}</p>
<div class="seg" role="radiogroup" aria-label={t('mcpToolsets.title')}>
  <button role="radio" aria-checked={all} class:on={all} onclick={() => setAll(true)}>
    {t('mcpToolsets.all')}
  </button>
  <button role="radio" aria-checked={!all} class:on={!all} onclick={() => setAll(false)}>
    {t('mcpToolsets.pick')}
  </button>
</div>
{#if !all}
  <div class="groups">
    {#each GROUPS as g (g)}
      <label class="row" class:locked={g === ALWAYS_ON}>
        <input
          type="checkbox"
          checked={enabled(g)}
          disabled={g === ALWAYS_ON}
          onchange={() => toggle(g)}
        />
        <span class="name">{t(`mcpToolsets.group.${g}`)}</span>
        <span class="what dim">{t(`mcpToolsets.groupDesc.${g}`)}</span>
      </label>
    {/each}
  </div>
{/if}
<p class="hint dim">{t('mcpToolsets.restart')}</p>

<style>
  h2 {
    margin: 26px 0 12px;
    font-size: 16px;
    color: var(--text);
    font-weight: 900;
  }
  .desc {
    font-size: 13px;
    margin: 0 0 10px;
    line-height: 1.55;
  }
  .hint {
    font-size: 12.5px;
    margin: 8px 0 10px;
    line-height: 1.5;
  }
  .seg {
    display: inline-flex;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
  }
  .seg button {
    background: none;
    border: none;
    border-right: 1px solid var(--border);
    color: var(--text-muted);
    font: inherit;
    font-size: 12.5px;
    padding: 7px 12px;
    cursor: pointer;
  }
  .seg button:last-child {
    border-right: none;
  }
  .seg button.on {
    background: color-mix(in srgb, var(--brand-primary) 18%, transparent);
    color: var(--text);
    font-weight: 800;
  }
  .groups {
    display: flex;
    flex-direction: column;
    margin-top: 10px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
  }
  .row {
    display: grid;
    grid-template-columns: auto minmax(110px, 160px) 1fr;
    align-items: baseline;
    gap: 10px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
    font-size: 13px;
    cursor: pointer;
  }
  .row:last-child {
    border-bottom: none;
  }
  .row.locked {
    cursor: default;
  }
  .name {
    color: var(--text);
    font-weight: 700;
  }
  .what {
    font-size: 12.5px;
    line-height: 1.45;
  }
  @media (max-width: 560px) {
    .row {
      grid-template-columns: auto 1fr;
    }
    .what {
      grid-column: 2;
    }
  }
</style>
