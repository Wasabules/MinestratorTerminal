<script lang="ts">
  /**
   * Choix du biome d'un serveur (feuille du bas), avec un mini-paysage par biome, dessiné sur le
   * relief du serveur. « Automatique » rend le biome tiré de l'id, le même que sur le desktop.
   *
   * Le choix est mémorisé sur le téléphone (`servers/colors.svelte.ts`) : il n'est pas
   * synchronisé avec le desktop, qui garde le sien.
   */
  import { t } from "../i18n";
  import { portal } from "../portal";
  import {
    BIOMES,
    defaultBiomeKey,
    hasChosenBiome,
    serverBiome,
    setServerBiome,
    type BiomeKey,
  } from "../servers/colors.svelte";
  import BiomeLandscape from "./BiomeLandscape.svelte";
  import Icon from "./Icon.svelte";

  let { serverId, onClose }: { serverId: number; onClose: () => void } = $props();

  const current = $derived(serverBiome(serverId).key);
  const chosen = $derived(hasChosenBiome(serverId));

  function choose(key: BiomeKey | null) {
    setServerBiome(serverId, key);
  }
</script>

<div
  class="scrim"
  use:portal
  role="button"
  tabindex="-1"
  aria-label={t("common.close")}
  onclick={onClose}
  onkeydown={(e) => e.key === "Escape" && onClose()}
></div>
<div class="sheet" use:portal role="dialog" aria-modal="true" aria-label={t("biome.title")}>
  <div class="sheet-handle"></div>
  <div class="head">
    <span class="title">{t("biome.title")}</span>
    <button class="close" onclick={onClose} aria-label={t("common.close")}>
      <Icon name="close" size={20} />
    </button>
  </div>

  <div class="grid" role="group" aria-label={t("biome.choose")}>
    {#each BIOMES as bio (bio.key)}
      <button
        class="bcard"
        class:sel={current === bio.key}
        style="--sel: {bio.accent}; --sel-l: {bio.accentLight}"
        aria-pressed={current === bio.key}
        onclick={() => choose(bio.key)}
      >
        <BiomeLandscape biome={bio} seed={serverId} height={48} pixel={4} />
        <span class="bname">
          {t(`biome.${bio.key}`)}
          {#if current === bio.key}<Icon name="check" size={16} stroke={2.6} />{/if}
        </span>
      </button>
    {/each}
  </div>

  <button class="auto" class:on={!chosen} aria-pressed={!chosen} onclick={() => choose(null)}>
    <span class="atxt">
      <span class="alabel">
        {t("biome.auto")} · {t(`biome.${defaultBiomeKey(serverId)}`)}
      </span>
      <span class="ahint">{t("biome.autoHint")}</span>
    </span>
    {#if !chosen}<Icon name="check" size={18} stroke={2.6} />{/if}
  </button>

  <p class="note">{t("biome.local")}</p>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 40;
    background: var(--scrim);
  }
  .sheet {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 41;
    display: flex;
    flex-direction: column;
    gap: 12px;
    max-height: 88dvh;
    overflow-y: auto;
    padding: 8px 16px calc(var(--safe-bottom) + 16px);
    background: var(--elevated);
    border-top-left-radius: var(--radius-lg);
    border-top-right-radius: var(--radius-lg);
    box-shadow: var(--shadow);
    color: var(--text);
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .title {
    font-size: 17px;
    font-weight: 900;
  }
  .close {
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    margin-right: -8px;
    border: none;
    border-radius: var(--radius);
    background: transparent;
    color: var(--text-muted);
  }
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
  }
  /* Carte de biome : mini-paysage, nom ; la sélection prend l'accent du biome proposé. */
  .bcard {
    display: flex;
    flex-direction: column;
    padding: 0;
    overflow: hidden;
    border: 2px solid var(--border);
    border-radius: 12px;
    background: var(--surface);
    color: var(--text);
    text-align: left;
  }
  .bcard.sel {
    border-color: var(--sel);
  }
  :global(:root[data-theme="light"]) .bcard.sel {
    border-color: var(--sel-l);
  }
  .bname {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
    padding: 9px 11px;
    font-size: 14.5px;
    font-weight: 800;
  }
  .auto {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    padding: 10px 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--inset);
    color: var(--text);
    text-align: left;
  }
  .auto.on {
    border-color: color-mix(in srgb, var(--brand-primary) 45%, var(--border));
  }
  .auto :global(svg) {
    color: var(--brand-primary);
    flex: none;
  }
  .atxt {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .alabel {
    font-size: 15px;
    font-weight: 800;
  }
  .ahint,
  .note {
    font-size: 12.5px;
    color: var(--text-dim);
    line-height: 1.4;
  }
  .note {
    margin: 0 2px;
  }
</style>
