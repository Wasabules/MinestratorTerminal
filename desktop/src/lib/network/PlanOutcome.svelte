<script lang="ts">
  /**
   * Bilan d'une application (liaison ou préréglage) : état, étapes écrites, snapshots prises,
   * rappel du redémarrage et de l'annulation (garde des agents, Réglages → MCP).
   */
  import { t } from '#lib/i18n/index.ts';
  import type { NetworkPlanStatus, NetworkSnapshot } from '#lib/types.ts';
  import Icon from '#lib/components/Icon.svelte';
  import { openSettingsSection } from '#lib/settings/nav.svelte.ts';

  let {
    status,
    applied,
    snapshots,
    restart,
  }: {
    status: NetworkPlanStatus;
    applied: number;
    snapshots: NetworkSnapshot[];
    /** Rappel du redémarrage propre au plan (liaison, LuckPerms, Geyser). */
    restart: string;
  } = $props();

  const tone = $derived(
    status === 'done' ? 'ok' : status === 'failed' || status === 'refused' ? 'bad' : 'warn',
  );
</script>

<div class="outcome {tone}" role="status">
  <p class="title">
    <Icon name={tone === 'ok' ? 'check' : 'triangle-alert'} size={16} />
    {t(`network.result.${status}`, { n: applied })}
  </p>
  {#if status === 'done' || status === 'failed' || status === 'cancelled'}
    {#if status === 'done'}<p>{restart}</p>{/if}
    {#if snapshots.length > 0}
      <p class="dim">
        {t('network.result.snapshots', { names: snapshots.map((s) => s.server).join(', ') })}
      </p>
    {/if}
    {#if applied > 0}
      <p class="dim">
        {t('network.result.undo')}
        <button class="linkish" onclick={() => openSettingsSection('mcp')}>{t('network.result.openGuard')}</button>
      </p>
    {/if}
  {/if}
</div>

<style>
  .outcome {
    border-radius: var(--radius);
    border: 1px solid var(--border);
    background: var(--inset);
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: 13.5px;
  }
  .outcome p {
    margin: 0;
  }
  .title {
    display: flex;
    align-items: center;
    gap: 8px;
    font-weight: 900;
  }
  .ok .title {
    color: var(--state-running-text);
  }
  .bad .title {
    color: var(--state-danger);
  }
  .warn .title {
    color: var(--state-pending);
  }
  .linkish {
    border: none;
    background: none;
    padding: 0;
    font: inherit;
    font-weight: 800;
    color: var(--brand-primary);
    cursor: pointer;
    text-decoration: underline;
  }
</style>
