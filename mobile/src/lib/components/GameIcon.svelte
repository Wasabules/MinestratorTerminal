<script lang="ts">
  /**
   * Icône du jeu. Le core compose l'URL CDN complète (`egg_icon_url`) à partir du nom de fichier
   * renvoyé par l'API (`minecraft.webp`) — voir `config::EGG_ICON_BASE`. Repli **monogramme**
   * (1ʳᵉ lettre du jeu) si l'URL manque ou si l'image ne charge pas : à plat sur l'accent du biome
   * qui l'entoure (charte Biome : aucun dégradé).
   */
  let {
    src = null,
    name = "",
    size = 32,
    radius = 8,
  }: { src?: string | null; name?: string; size?: number; radius?: number } = $props();

  let failed = $state(false);
  const isUrl = $derived(!!src && /^https?:\/\//i.test(src));
  const mono = $derived((name || "?").trim().charAt(0).toUpperCase() || "?");
</script>

{#if isUrl && !failed}
  <img
    {src}
    alt=""
    width={size}
    height={size}
    loading="lazy"
    style="border-radius:{radius}px"
    onerror={() => (failed = true)}
  />
{:else}
  <span
    class="mono"
    style="width:{size}px;height:{size}px;font-size:{Math.round(size * 0.45)}px;border-radius:{radius}px"
    aria-hidden="true">{mono}</span
  >
{/if}

<style>
  img,
  .mono {
    flex: none;
    display: block;
  }
  img {
    object-fit: contain;
    background: var(--inset);
  }
  .mono {
    display: grid;
    place-items: center;
    background: var(--accent);
    color: var(--on-accent);
    font-weight: 900;
    line-height: 1;
  }
</style>
