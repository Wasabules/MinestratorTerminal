<script lang="ts">
  /**
   * Bouton œil du mode anonyme (accueil, en-tête de la vue serveur) : masque IP, adresses et
   * e-mail à l'écran le temps d'une capture (`../anon/anon.svelte.ts`). Actif, il passe en
   * pastille verte avec l'œil barré : un mode qu'on ne doit pas oublier, dans un sens comme dans
   * l'autre. Un court message confirme chaque bascule (l'icône seule ne dit pas ce qu'elle fait).
   *
   * `tone="banner"` : posé sur un paysage, toujours sombre — couleurs du thème sombre dans les
   * deux thèmes.
   */
  import { untrack } from "svelte";
  import { t } from "../i18n";
  import { isAnon, toggleAnon } from "../anon/anon.svelte";
  import { portal } from "../portal";
  import Icon from "./Icon.svelte";

  let { tone = "page" }: { tone?: "page" | "banner" } = $props();

  const on = $derived(isAnon());

  /** Message affiché après une bascule faite ici (annoncé aussi aux lecteurs d'écran). */
  let toast = $state<string | null>(null);
  let timer: ReturnType<typeof setTimeout> | undefined;

  function toggle() {
    toggleAnon();
    toast = untrack(() => t(isAnon() ? "anon.announceOn" : "anon.announceOff"));
    clearTimeout(timer);
    timer = setTimeout(() => (toast = null), 1800);
  }

  $effect(() => () => clearTimeout(timer));
</script>

<button
  class="anon"
  class:on
  class:banner={tone === "banner"}
  onclick={toggle}
  aria-pressed={on}
  aria-label={t("anon.title")}
  title={t(on ? "anon.hintOn" : "anon.hintOff")}
>
  <Icon name={on ? "eyeOff" : "eye"} size={20} />
</button>

{#if toast}
  <div class="toast" use:portal role="status" aria-live="polite">
    <Icon name={on ? "eyeOff" : "eye"} size={16} />
    {toast}
  </div>
{/if}

<style>
  .anon {
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    flex: none;
    padding: 0;
    border-radius: var(--radius);
    border: 1px solid transparent;
    background: var(--surface);
    color: var(--text-muted);
  }
  /* Sur un paysage : pastille sombre, comme les puces de la bannière. */
  .anon.banner {
    background: rgba(13, 17, 19, 0.72);
    color: #e4ebe8;
  }
  /* Actif : pastille verte de la marque (état « allumé » de la charte), visible d'un coup d'œil. */
  .anon.on {
    color: var(--brand-primary);
    background: color-mix(in srgb, var(--brand-primary) 14%, var(--surface));
    border-color: color-mix(in srgb, var(--brand-primary) 45%, transparent);
  }
  .anon.banner.on {
    color: #5fd68c;
    background: #0d1113;
    border-color: rgba(95, 214, 140, 0.55);
  }
  .toast {
    position: fixed;
    z-index: 60;
    left: 50%;
    top: calc(var(--safe-top) + 64px);
    transform: translateX(-50%);
    display: inline-flex;
    align-items: center;
    gap: 8px;
    max-width: calc(100vw - 32px);
    padding: 9px 14px;
    border-radius: 999px;
    background: var(--elevated);
    border: 1px solid var(--border);
    box-shadow: var(--shadow);
    color: var(--text);
    font-size: 13.5px;
    font-weight: 800;
    pointer-events: none;
  }
</style>
