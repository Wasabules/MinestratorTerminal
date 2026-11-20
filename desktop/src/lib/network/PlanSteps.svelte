<script lang="ts">
  /**
   * Étapes d'un plan de réseau (liaison ou préréglage), regroupées par serveur : état, résumé,
   * diff dépliable (secrets déjà masqués par le cœur) et, après application, le résultat.
   */
  import { t } from '#lib/i18n/index.ts';
  import type { NetworkStep } from '#lib/types.ts';
  import Icon from '#lib/components/Icon.svelte';
  import Masked from '#lib/components/Masked.svelte';
  import BiomeBlock from '#lib/components/BiomeBlock.svelte';
  import { serverBiome } from '#lib/servers/colors.svelte.ts';
  import { diffClass, groupSteps } from './util.ts';

  let { steps }: { steps: NetworkStep[] } = $props();

  const groups = $derived(groupSteps(steps));

  const STATUS_ICON: Record<NetworkStep['status'], string> = {
    ok: 'check',
    change: 'pencil',
    manual: 'info',
    refused: 'x',
  };
</script>

<div class="groups">
  {#each groups as g (g.serverId)}
    <section class="group" aria-label={g.server}>
      <h4><BiomeBlock biome={serverBiome(g.serverId)} size={10} /> {g.server}</h4>
      <ul>
        {#each g.steps as s (s.id)}
          <li class="step {s.status}">
            <span class="st" title={t(`network.plan.status.${s.status}`)}>
              <Icon name={STATUS_ICON[s.status]} size={12} />
              <span>{t(`network.plan.status.${s.status}`)}</span>
            </span>
            <div class="body">
              <p class="summary"><Masked value={s.summary} /></p>
              {#if s.outcome}
                <p class="outcome {s.outcome}">
                  {t(`network.plan.outcome.${s.outcome}`)}{#if s.detail}&nbsp;: <Masked value={s.detail} />{/if}
                </p>
              {/if}
              {#if s.diff}
                <details>
                  <summary>{t('network.plan.showDiff')}</summary>
                  <pre class="diff">{#each s.diff.split('\n') as line, i (i)}<span class={diffClass(line)}><Masked value={line} /></span>
{/each}</pre>
                </details>
              {/if}
            </div>
          </li>
        {/each}
      </ul>
    </section>
  {/each}
</div>

<style>
  .groups {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .group h4 {
    display: flex;
    align-items: center;
    gap: 7px;
    margin: 0 0 6px;
    font-size: 13.5px;
    font-weight: 900;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .step {
    display: grid;
    grid-template-columns: 116px 1fr;
    gap: 10px;
    align-items: start;
    padding: 8px 10px;
    border-radius: var(--radius);
    background: var(--inset);
    border: 1px solid var(--border);
    font-size: 13px;
  }
  .st {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: 12px;
    font-weight: 800;
    color: var(--text-muted);
  }
  .step.change .st {
    color: var(--accent-text);
  }
  .step.ok .st {
    color: var(--state-running-text);
  }
  .step.manual .st {
    color: var(--state-pending);
  }
  .step.refused {
    border-color: color-mix(in srgb, var(--state-danger) 50%, var(--border));
  }
  .step.refused .st {
    color: var(--state-danger);
  }
  .body {
    min-width: 0;
  }
  .summary {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .outcome {
    margin: 4px 0 0;
    font-size: 12.5px;
    font-weight: 700;
    overflow-wrap: anywhere;
  }
  .outcome.done {
    color: var(--state-running-text);
  }
  .outcome.failed {
    color: var(--state-danger);
  }
  .outcome.not_run {
    color: var(--text-dim);
  }
  details {
    margin-top: 6px;
  }
  details summary {
    cursor: pointer;
    font-size: 12px;
    font-weight: 800;
    color: var(--text-muted);
  }
  .diff {
    margin: 6px 0 0;
    padding: 8px 10px;
    max-height: 280px;
    overflow: auto;
    background: var(--chrome);
    border-radius: var(--radius-sm);
    font-family: var(--font-mono);
    font-size: 11.5px;
    line-height: 1.5;
    white-space: pre;
  }
  .diff .add {
    color: var(--state-running-text);
  }
  .diff .del {
    color: var(--state-danger);
  }
  .diff .hunk {
    color: var(--state-hibernate);
  }
  .diff .meta {
    color: var(--text-dim);
  }
</style>
