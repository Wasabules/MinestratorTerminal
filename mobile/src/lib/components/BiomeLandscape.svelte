<script lang="ts">
  /**
   * Paysage en pixels d'un biome (bannière du tableau de bord, cartes de l'accueil, sélecteur de
   * biome). Dessiné par l'app, sans image : `servers/landscape.ts` produit un chemin par couleur,
   * rendu en `crispEdges`. Les pixels restent carrés (`pixel` px de côté) : la largeur mesurée
   * fixe le nombre de colonnes, le paysage est ancré en bas et rogné en haut si besoin.
   * Purement décoratif (`aria-hidden`).
   */
  import type { Biome } from "../servers/colors.svelte";
  import { landscape } from "../servers/landscape";

  let {
    biome,
    seed,
    height,
    pixel = 6,
  }: {
    biome: Biome;
    /** Graine du relief : l'id du serveur. */
    seed: number;
    /** Hauteur affichée, en px CSS. */
    height: number;
    /** Côté d'un pixel du paysage, en px CSS. */
    pixel?: number;
  } = $props();

  let width = $state(0);
  const rows = $derived(Math.max(6, Math.ceil(height / pixel)));
  const cols = $derived(Math.max(8, Math.ceil(width / pixel)));
  const land = $derived(width > 0 ? landscape(biome, seed, cols, rows) : null);
</script>

<div
  class="land"
  style="height: {height}px; background: {biome.sky}"
  bind:clientWidth={width}
  aria-hidden="true"
>
  {#if land}
    <svg
      width={cols * pixel}
      height={rows * pixel}
      viewBox="0 0 {cols} {rows}"
      shape-rendering="crispEdges"
    >
      <path d={land.stars} fill={biome.stars} />
      <path d={land.far} fill={biome.far} />
      <path d={land.near} fill={biome.near} />
      <path d={land.stem} fill={biome.deco.stem} />
      <path d={land.body} fill={biome.deco.body} />
      <path d={land.top} fill={biome.deco.top} />
    </svg>
  {/if}
</div>

<style>
  .land {
    position: relative;
    overflow: hidden;
    width: 100%;
  }
  svg {
    position: absolute;
    left: 0;
    bottom: 0;
    display: block;
  }
</style>
