<script lang="ts">
  import { t } from '$lib/i18n';
  import type { PlayerAction } from '$lib/types';

  let {
    actions,
    label = '⋯',
    disabled = false,
    align = 'right',
    onPick,
  }: {
    actions: PlayerAction[];
    label?: string;
    disabled?: boolean;
    align?: 'left' | 'right';
    onPick: (action: PlayerAction) => void;
  } = $props();

  let open = $state(false);
  let menuStyle = $state('');
  const ITEM_H = 37; // hauteur approx. d'un item (décide de l'ouverture vers le haut)

  const LABEL: Record<PlayerAction, string> = {
    kick: 'players.kick',
    ban: 'players.ban',
    unban: 'players.unban',
    op_add: 'players.opAdd',
    op_remove: 'players.opRemove',
    whitelist_add: 'players.whitelistAdd',
    whitelist_remove: 'players.whitelistRemove',
  };

  // Positionne le menu en `fixed` (coordonnées viewport) : il échappe ainsi à l'`overflow: hidden`
  // de la liste des joueurs — qui le tronquait — et passe au-dessus de tout le reste.
  function toggle(e: MouseEvent) {
    if (open) {
      open = false;
      return;
    }
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const estH = actions.length * ITEM_H + 12;
    // Vers le haut si ça déborderait en bas et qu'il y a la place au-dessus.
    const up = r.bottom + 6 + estH > vh && r.top - 6 - estH > 0;
    const horiz =
      align === 'left' ? `left: ${Math.round(r.left)}px` : `right: ${Math.round(vw - r.right)}px`;
    const vert = up ? `bottom: ${Math.round(vh - r.top + 6)}px` : `top: ${Math.round(r.bottom + 6)}px`;
    menuStyle = `${horiz}; ${vert}`;
    open = true;
  }

  function pick(action: PlayerAction) {
    open = false;
    onPick(action);
  }
</script>

<div class="pam">
  <button class="trigger" {disabled} onclick={toggle}>{label}</button>
  {#if open}
    <button class="backdrop" aria-label={t('common.close')} onclick={() => (open = false)}></button>
    <div class="menu" role="menu" style={menuStyle}>
      {#each actions as action (action)}
        <button
          class="item"
          class:danger={action === 'kick' || action === 'ban'}
          role="menuitem"
          onclick={() => pick(action)}
        >
          {t(LABEL[action])}
        </button>
      {/each}
    </div>
  {/if}
</div>

<style>
  .pam {
    position: relative;
    display: inline-flex;
  }
  .trigger {
    background: none;
    border: 1px solid var(--border);
    border-radius: 7px;
    cursor: pointer;
    font: inherit;
    font-size: 13px;
    color: var(--text-muted);
    padding: 6px 11px;
    white-space: nowrap;
  }
  .trigger:hover:not(:disabled) {
    color: var(--text);
    background: color-mix(in srgb, var(--text) 6%, transparent);
  }
  .trigger:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .backdrop {
    position: fixed;
    inset: 0;
    background: none;
    border: none;
    z-index: 90;
    cursor: default;
  }
  .menu {
    position: fixed;
    z-index: 91;
    min-width: 200px;
    max-height: 70vh;
    overflow-y: auto;
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
    white-space: nowrap;
  }
  .item:hover {
    background: color-mix(in srgb, var(--text) 8%, transparent);
  }
  .item.danger {
    color: var(--state-danger);
  }
</style>
