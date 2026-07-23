<script lang="ts">
  interface Tab {
    id: string;
    label: string;
    icon: string;
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
    >
      <span class="ico">{tab.icon}</span>
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
    background: var(--surface);
    border-top: 1px solid var(--border);
    padding-bottom: var(--safe-bottom);
    z-index: 10;
  }
  button {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 3px;
    background: transparent;
    border: none;
    color: var(--text-dim);
    height: var(--nav-height);
    padding: 6px 0;
  }
  button.active {
    color: var(--brand-primary);
  }
  .ico {
    font-size: 20px;
    line-height: 1;
  }
  .lbl {
    font-size: 11px;
  }
</style>
