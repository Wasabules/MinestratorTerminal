<script lang="ts">
  /**
   * Petit bloc en pixels d'un biome (tête de groupe d'onglets, listes) : la moitié haute à
   * l'accent, la moitié basse au sol, comme un bloc d'herbe. Décoratif.
   */
  import type { Biome } from "../servers/colors.svelte";

  let { biome, size = 14 }: { biome: Biome; size?: number } = $props();
</script>

<svg
  class="block"
  width={size}
  height={size}
  viewBox="0 0 4 4"
  shape-rendering="crispEdges"
  aria-hidden="true"
>
  <rect width="4" height="4" fill={biome.ground} />
  <rect width="4" height="2" fill={biome.accent} />
  <rect x="1" y="2" width="1" height="1" fill={biome.accent} />
</svg>

<style>
  .block {
    display: block;
    flex: none;
    border-radius: 2px;
  }
</style>
