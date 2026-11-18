<script lang="ts">
  /**
   * Bouton du mode anonyme (barre du haut) : masque IP, adresses et e-mail à l'écran le temps
   * d'une capture (`#lib/anon/anon.svelte.ts`). Actif, il passe en pastille verte libellée
   * « Anonyme » avec l'œil barré : un mode qu'on ne doit pas oublier, dans un sens comme dans
   * l'autre.
   *
   * Raccourci Ctrl+Shift+H (⇧⌘H), écouté en phase de capture comme celui de la palette : il passe
   * avant les champs qui consomment le clavier (console, éditeur).
   */
  import { onMount, untrack } from 'svelte';
  import { t } from '#lib/i18n/index.ts';
  import {
    anonShortcutAria,
    anonShortcutLabel,
    isAnon,
    isAnonShortcut,
    toggleAnon,
  } from '#lib/anon/anon.svelte.ts';
  import Icon from './Icon.svelte';

  const keys = anonShortcutLabel();
  const on = $derived(isAnon());

  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!isAnonShortcut(e)) return;
      e.preventDefault();
      e.stopPropagation();
      if (!e.repeat) toggleAnon(); // touche maintenue : un seul basculement
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  });

  // Annonce aux lecteurs d'écran : le raccourci bascule le mode sans que le bouton ait le focus.
  let announce = $state('');
  let initial = true;
  $effect(() => {
    const now = isAnon();
    if (initial) {
      initial = false;
      return;
    }
    announce = untrack(() => t(now ? 'anon.announceOn' : 'anon.announceOff'));
  });
</script>

<button
  class="anon"
  class:on
  onclick={toggleAnon}
  aria-pressed={on}
  aria-label={t('anon.title')}
  aria-keyshortcuts={anonShortcutAria()}
  title={t(on ? 'anon.hintOn' : 'anon.hintOff', { keys })}
>
  <Icon name={on ? 'eye-off' : 'eye'} size={16} stroke={2} />
  {#if on}<span class="label">{t('anon.badge')}</span>{/if}
</button>
<span class="sr" role="status" aria-live="polite">{announce}</span>

<style>
  .anon {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 32px;
    padding: 0 9px;
    background: none;
    border: 1px solid transparent;
    border-radius: var(--radius);
    color: var(--text-muted);
    cursor: pointer;
    font: inherit;
    font-size: 12.5px;
    font-weight: 800;
    flex: none;
  }
  .anon:hover {
    color: var(--text);
    background: color-mix(in srgb, var(--text) 6%, transparent);
  }
  /* Actif : pastille verte de la marque (état « allumé » de la charte), visible d'un coup d'œil. */
  .anon.on {
    color: var(--brand-primary);
    background: color-mix(in srgb, var(--brand-primary) 14%, transparent);
    border-color: color-mix(in srgb, var(--brand-primary) 45%, transparent);
  }
  .anon.on:hover {
    background: color-mix(in srgb, var(--brand-primary) 22%, transparent);
  }
  @media (max-width: 960px) {
    .label {
      display: none;
    }
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: -1px;
    padding: 0;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }
</style>
