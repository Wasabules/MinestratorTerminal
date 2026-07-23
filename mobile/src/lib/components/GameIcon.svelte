<script lang="ts">
  // Icône du jeu (egg_icon MineStrator). URL https autorisée par la CSP ; repli sur un carré neutre.
  let { src, size = 32 }: { src: string | null; size?: number } = $props();
  let failed = $state(false);
</script>

{#if src && !failed}
  <img src={src} alt="" width={size} height={size} onerror={() => (failed = true)} />
{:else}
  <span class="ph" style="width:{size}px;height:{size}px"></span>
{/if}

<style>
  img,
  .ph {
    border-radius: 8px;
    flex: none;
    object-fit: cover;
    background: var(--elevated);
  }
  .ph {
    display: inline-block;
    border: 1px solid var(--border);
  }
</style>
