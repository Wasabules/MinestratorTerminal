<script lang="ts">
  /**
   * Barre du bas de la vue serveur, sur le chrome de l'app. L'élément actif prend l'accent du
   * biome du serveur (pastille pleine derrière l'icône, libellé appuyé), à la manière de l'onglet
   * actif de la barre d'onglets desktop.
   */
  import Icon from "./Icon.svelte";
  interface Tab {
    id: string;
    label: string;
    icon: string; // nom d'icône (voir icons.ts)
  }
  let {
    tabs,
    active,
    onSelect,
  }: { tabs: Tab[]; active: string; onSelect: (id: string) => void } = $props();
</script>

<nav>
  {#each tabs as tab (tab.id)}
    <button
      class:active={tab.id === active}
      onclick={() => onSelect(tab.id)}
      aria-label={tab.label}
      aria-current={tab.id === active ? "page" : undefined}
    >
      <span class="pill"><Icon name={tab.icon} size={21} stroke={tab.id === active ? 2.4 : 2} /></span>
      <span class="lbl">{tab.label}</span>
    </button>
  {/each}
</nav>

<style>
  nav {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    display: flex;
    gap: 2px;
    padding: 0 4px var(--safe-bottom);
    background: var(--chrome);
    border-top: 1px solid var(--border);
    z-index: 10;
  }
  button {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 3px;
    height: var(--nav-height);
    padding: 0;
    border: none;
    background: transparent;
    color: var(--text-dim);
  }
  .pill {
    display: grid;
    place-items: center;
    width: 54px;
    height: 30px;
    border-radius: 999px;
    transition: background 0.15s ease, color 0.15s ease;
  }
  .lbl {
    max-width: 100%;
    padding: 0 1px;
    font-size: 11px;
    font-weight: 700;
    letter-spacing: -0.01em;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  button.active {
    color: var(--text);
  }
  button.active .pill {
    background: var(--accent);
    color: var(--on-accent);
  }
  button.active .lbl {
    font-weight: 800;
  }
  @media (prefers-reduced-motion: reduce) {
    .pill {
      transition: none;
    }
  }
</style>
