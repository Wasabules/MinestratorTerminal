<script lang="ts">
  import { api, humanizeError } from '$lib/ipc';
  import { t } from '$lib/i18n';
  import { riskColor } from '$lib/copilot/format';
  import type { ProposedAction } from '$lib/types';
  import Icon from './Icon.svelte';

  let { actions, serverId }: { actions: ProposedAction[]; serverId: number } = $props();

  type ApplyState = { status: 'running' | 'done' | 'error'; msg: string };
  let applied = $state<Record<number, ApplyState>>({});
  // Actions « danger » (restauration destructive, kill…) : confirmation explicite en deux temps.
  let confirming = $state<Record<number, boolean>>({});

  async function apply(i: number, action: ProposedAction) {
    if (action.risk === 'danger' && !confirming[i]) {
      confirming[i] = true; // 1er clic : on demande confirmation, on n'exécute PAS
      return;
    }
    confirming[i] = false;
    applied[i] = { status: 'running', msg: '' };
    try {
      const res = await api.copilotApply(serverId, action.tool, action.args);
      applied[i] = { status: 'done', msg: res };
    } catch (e) {
      applied[i] = { status: 'error', msg: humanizeError(e) };
    }
  }
</script>

<div class="actions">
  {#each actions as action, i (i)}
    <div class="action">
      <span class="risk" style="background: {riskColor(action.risk)}">{action.risk}</span>
      <div class="act-body">
        <div class="act-label">{action.label}</div>
        <code class="act-tool">{action.tool}</code>
        {#if applied[i]?.status === 'done'}
          <div class="act-res ok"><Icon name="check" size={14} /> <span>{applied[i].msg}</span></div>
        {:else if applied[i]?.status === 'error'}
          <div class="act-res err"><Icon name="x" size={14} /> <span>{applied[i].msg}</span></div>
        {/if}
      </div>
      <div class="btns">
        {#if confirming[i]}
          <button class="cancel" onclick={() => (confirming[i] = false)}>{t('copilot.cancel')}</button>
        {/if}
        <button
          class="apply"
          class:confirm={confirming[i]}
          disabled={applied[i]?.status === 'running' || applied[i]?.status === 'done'}
          onclick={() => apply(i, action)}
        >
          {#if applied[i]?.status === 'running'}{t('copilot.applying')}
          {:else if applied[i]?.status === 'done'}{t('copilot.applied')}
          {:else if confirming[i]}{t('copilot.confirm')}
          {:else}{t('copilot.apply')}{/if}
        </button>
      </div>
    </div>
  {/each}
</div>

<style>
  .actions {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .action {
    display: flex;
    align-items: flex-start;
    gap: 9px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 10px 12px;
  }
  .risk {
    flex: none;
    font-size: 9.5px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: #fff;
    padding: 2px 6px;
    border-radius: 5px;
    margin-top: 1px;
  }
  .act-body {
    flex: 1;
    min-width: 0;
  }
  .act-label {
    font-size: 13px;
    line-height: 1.4;
  }
  .act-tool {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--text-dim);
  }
  .act-res {
    display: flex;
    align-items: flex-start;
    gap: 5px;
    font-size: 12px;
    margin-top: 4px;
  }
  .act-res :global(.icon) {
    margin-top: 1px;
    flex: none;
  }
  .act-res.ok {
    color: var(--state-ok, #3fb950);
  }
  .act-res.err {
    color: var(--state-danger);
  }
  .btns {
    flex: none;
    display: flex;
    gap: 6px;
    align-items: flex-start;
  }
  .apply {
    flex: none;
    background: var(--brand-primary);
    color: #fff;
    border: none;
    border-radius: 7px;
    cursor: pointer;
    font: inherit;
    font-size: 12px;
    padding: 6px 13px;
  }
  /* En attente de confirmation d'une action destructive : le bouton passe au rouge. */
  .apply.confirm {
    background: var(--state-danger);
  }
  .apply:hover:not(:disabled) {
    filter: brightness(1.08);
  }
  .apply:disabled {
    opacity: 0.55;
    cursor: default;
  }
  .cancel {
    background: none;
    border: 1px solid var(--border);
    border-radius: 7px;
    color: var(--text-dim);
    cursor: pointer;
    font: inherit;
    font-size: 12px;
    padding: 6px 11px;
  }
  .cancel:hover {
    color: var(--text);
  }
</style>
