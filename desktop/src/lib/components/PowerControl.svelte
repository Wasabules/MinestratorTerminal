<script lang="ts">
  import { api, humanizeError } from '#lib/ipc.ts';
  import { t } from '#lib/i18n/index.ts';
  import { primaryPowerAction, powerWarning, secondaryPowerActions } from '#lib/servers/power.ts';
  import type { PowerAction } from '#lib/types.ts';

  let {
    serverId,
    running,
    disabled = false,
    compact = false,
    restartWarning = null,
    stopWarning = null,
  }: {
    serverId: number;
    running: boolean;
    disabled?: boolean;
    compact?: boolean;
    /**
     * Avertissements configurés par l'hébergeur pour CE serveur (`poweraction_message_*` de
     * l'API). Quand ils existent, l'action passe par une confirmation qui les affiche.
     */
    restartWarning?: string | null;
    stopWarning?: string | null;
  } = $props();

  let busy = $state(false);
  let menuOpen = $state(false);
  let error = $state<string | null>(null);
  /** Action retenue en attente de confirmation (avertissement de l'hébergeur à lire). */
  let pending = $state<PowerAction | null>(null);

  // Actions secondaires regroupées dans le menu « ⋯ » (logique partagée avec la palette).
  const secondary = $derived(secondaryPowerActions(running));

  /** Avertissement associé à une action, ou `null` si l'hébergeur n'en a pas défini. */
  function warningFor(action: PowerAction): string | null {
    return powerWarning(action, { restartWarning, stopWarning });
  }

  /** Demande confirmation si l'hébergeur a posé un avertissement, sinon exécute directement. */
  function request(action: PowerAction) {
    menuOpen = false;
    if (warningFor(action)) {
      pending = action;
      return;
    }
    void act(action);
  }

  async function act(action: PowerAction) {
    menuOpen = false;
    pending = null;
    busy = true;
    error = null;
    try {
      await api.powerAction(serverId, action);
    } catch (e) {
      error = humanizeError(e);
    } finally {
      busy = false;
    }
  }

  const primary = $derived(primaryPowerAction(running));
</script>

<div class="power">
  <div class="group" class:danger={running} class:compact>
    <button class="main" disabled={disabled || busy} onclick={() => request(primary)}>
      {#if busy}<span class="spinner"></span>{/if}
      {t(`power.${primary}`)}
    </button>
    <button
      class="caret"
      disabled={disabled || busy}
      aria-label={t('power.more')}
      onclick={() => (menuOpen = !menuOpen)}
    >
      ⋯
    </button>
  </div>

  {#if menuOpen}
    <button class="backdrop" aria-label={t('common.close')} onclick={() => (menuOpen = false)}
    ></button>
    <div class="menu" role="menu">
      {#each secondary as action (action)}
        <button class="item" role="menuitem" onclick={() => request(action)}>
          {t(`power.${action}`)}
        </button>
      {/each}
    </div>
  {/if}

  {#if pending}
    <button class="backdrop" aria-label={t('common.close')} onclick={() => (pending = null)}
    ></button>
    <div class="confirm" role="dialog" aria-modal="true">
      <p class="warn">{warningFor(pending)}</p>
      <div class="confirm-actions">
        <button class="btn btn--ghost" onclick={() => (pending = null)}>{t('common.cancel')}</button>
        <button class="btn" onclick={() => act(pending!)}>{t(`power.${pending}`)}</button>
      </div>
    </div>
  {/if}

  {#if error}<span class="err">{error}</span>{/if}
</div>

<style>
  .confirm {
    position: absolute;
    z-index: 20;
    top: calc(100% + 6px);
    right: 0;
    width: 280px;
    padding: 12px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--elevated);
    box-shadow: 0 10px 30px rgb(0 0 0 / 35%);
  }
  .confirm .warn {
    margin: 0 0 10px;
    font-size: 13px;
    line-height: 1.45;
    color: var(--text-muted);
  }
  .confirm-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  .power {
    position: relative;
    display: inline-flex;
    flex-direction: column;
    gap: 6px;
    align-items: flex-start;
  }
  .group {
    display: inline-flex;
    border-radius: var(--radius);
    overflow: hidden;
    box-shadow: 0 1px 0 rgba(0, 0, 0, 0.2);
  }
  .main {
    border: none;
    cursor: pointer;
    font: inherit;
    font-weight: 600;
    color: #fff;
    background: var(--brand-primary);
    padding: 9px 16px;
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }
  .group.danger .main {
    background: var(--brand-pro);
  }
  .caret {
    border: none;
    border-left: 1px solid rgba(255, 255, 255, 0.18);
    cursor: pointer;
    color: #fff;
    background: var(--brand-primary);
    padding: 0 12px;
    font-size: 15px;
  }
  .group.danger .caret {
    background: var(--brand-pro);
  }
  .main:hover:not(:disabled),
  .caret:hover:not(:disabled) {
    filter: brightness(1.1);
  }
  .main:disabled,
  .caret:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }
  .group.compact {
    border-radius: 7px;
  }
  .group.compact .main {
    padding: 3px 11px;
    font-size: 11.5px;
    gap: 6px;
  }
  .group.compact .caret {
    padding: 0 9px;
    font-size: 12px;
  }
  .backdrop {
    position: fixed;
    inset: 0;
    background: none;
    border: none;
    z-index: 20;
    cursor: default;
  }
  .menu {
    position: absolute;
    top: calc(100% + 6px);
    left: 0;
    z-index: 21;
    min-width: 190px;
    background: var(--elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: var(--shadow);
    padding: 5px;
    display: flex;
    flex-direction: column;
  }
  .item {
    text-align: left;
    background: none;
    border: none;
    cursor: pointer;
    font: inherit;
    color: var(--text);
    padding: 8px 11px;
    border-radius: 7px;
  }
  .item:hover {
    background: color-mix(in srgb, var(--text) 8%, transparent);
  }
  .err {
    font-size: 12px;
    color: var(--state-danger);
  }
</style>
