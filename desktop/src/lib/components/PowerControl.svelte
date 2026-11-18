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
    tone = 'page',
    restartWarning = null,
    stopWarning = null,
  }: {
    serverId: number;
    running: boolean;
    disabled?: boolean;
    /** Barre d'état de la console : une seule action visible, le reste dans le menu. */
    compact?: boolean;
    /**
     * `banner` : posé sur le paysage d'un biome, toujours sombre — les boutons gardent les
     * couleurs du thème sombre (principal clair, secondaire noir) quel que soit le thème.
     */
    tone?: 'page' | 'banner';
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

  const primary = $derived(primaryPowerAction(running));
  /**
   * Boutons visibles : le premier est le bouton principal (clair). Serveur en marche, hors mode
   * compact : « Redémarrer » puis « Arrêter », comme sur la bannière du tableau de bord.
   */
  const buttons = $derived<PowerAction[]>(
    running && !compact ? ['restart', primary] : [primary]
  );
  // Actions secondaires (logique partagée avec la palette) moins celles déjà en bouton :
  // Redémarrer (10 s), Arrêter (10 s), Forcer l'arrêt.
  const secondary = $derived(secondaryPowerActions(running).filter((a) => !buttons.includes(a)));

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
</script>

<div class="power" class:compact class:banner={tone === 'banner'}>
  <div class="group">
    {#each buttons as action, i (action)}
      <button
        class="pbtn"
        class:main={i === 0}
        disabled={disabled || busy}
        onclick={() => request(action)}
      >
        {#if busy && i === 0}<span class="spinner"></span>{/if}
        {t(`power.${action}`)}
      </button>
    {/each}
    {#if secondary.length > 0}
      <button
        class="pbtn caret"
        disabled={disabled || busy}
        aria-label={t('power.more')}
        title={t('power.more')}
        aria-haspopup="menu"
        aria-expanded={menuOpen}
        onclick={() => (menuOpen = !menuOpen)}
      >
        ▾
      </button>
    {/if}
  </div>

  {#if menuOpen}
    <button class="backdrop" aria-label={t('common.close')} onclick={() => (menuOpen = false)}
    ></button>
    <div class="menu" role="menu">
      {#each secondary as action (action)}
        <button
          class="item"
          class:danger={action === 'kill'}
          role="menuitem"
          onclick={() => request(action)}
        >
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
        <button class="btn" class:btn--danger={pending === 'kill'} onclick={() => act(pending!)}
          >{t(`power.${pending}`)}</button
        >
      </div>
    </div>
  {/if}

  {#if error}<span class="err">{error}</span>{/if}
</div>

<style>
  .power {
    position: relative;
    display: inline-flex;
    flex-direction: column;
    gap: 6px;
    align-items: flex-end;
  }
  .group {
    display: inline-flex;
    gap: 8px;
  }
  /* Secondaire sombre par défaut ; le premier bouton est le principal, clair. */
  .pbtn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    height: 40px;
    padding: 0 18px;
    border: none;
    border-radius: var(--radius);
    cursor: pointer;
    font: inherit;
    font-weight: 800;
    font-size: 14.5px;
    color: var(--btn-secondary-ink);
    background: var(--btn-secondary-bg);
    white-space: nowrap;
  }
  .pbtn.main {
    font-weight: 900;
    color: var(--btn-primary-ink);
    background: var(--btn-primary-bg);
  }
  .pbtn:hover:not(:disabled) {
    background: color-mix(in srgb, var(--btn-secondary-bg) 82%, var(--btn-secondary-ink));
  }
  .pbtn.main:hover:not(:disabled) {
    background: color-mix(in srgb, var(--btn-primary-bg) 86%, var(--btn-primary-ink));
  }
  .banner .pbtn {
    color: #f1f5f3;
    background: #0d1113;
  }
  .banner .pbtn.main {
    color: #0d1113;
    background: #f1f5f3;
  }
  .banner .pbtn:hover:not(:disabled) {
    background: #1b2126;
  }
  .banner .pbtn.main:hover:not(:disabled) {
    background: #dfe6e2;
  }
  .pbtn.caret {
    width: 40px;
    padding: 0;
    font-size: 13px;
  }
  .pbtn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .compact .group {
    gap: 5px;
  }
  .compact .pbtn {
    height: 28px;
    padding: 0 11px;
    font-size: 12.5px;
    border-radius: 8px;
  }
  .compact .pbtn.caret {
    width: 28px;
    padding: 0;
    font-size: 11px;
  }
  .compact .spinner {
    width: 12px;
    height: 12px;
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
    top: 46px;
    right: 0;
    z-index: 21;
    min-width: 200px;
    background: var(--elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: var(--shadow);
    padding: 5px;
    display: flex;
    flex-direction: column;
  }
  .compact .menu,
  .compact .confirm {
    top: 34px;
  }
  .item {
    text-align: left;
    background: none;
    border: none;
    cursor: pointer;
    font: inherit;
    font-weight: 700;
    color: var(--text);
    padding: 8px 11px;
    border-radius: 7px;
  }
  .item:hover {
    background: var(--hover);
  }
  .item.danger {
    color: var(--state-danger);
  }
  .item.danger:hover {
    background: var(--danger-bg);
    color: var(--danger-ink);
  }
  .confirm {
    position: absolute;
    z-index: 21;
    top: 46px;
    right: 0;
    width: 300px;
    padding: 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--elevated);
    box-shadow: var(--shadow);
    color: var(--text);
  }
  .confirm .warn {
    margin: 0 0 12px;
    font-size: 13px;
    line-height: 1.45;
    color: var(--text-muted);
  }
  .confirm-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  /* Pilule : lisible sur la page comme sur le paysage de la bannière. */
  .err {
    max-width: 340px;
    padding: 4px 10px;
    border-radius: 999px;
    font-size: 12px;
    font-weight: 700;
    background: var(--danger-bg);
    color: var(--danger-ink);
  }
</style>
