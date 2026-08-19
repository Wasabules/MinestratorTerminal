<script lang="ts">
  /**
   * Icône du jeu. Le core compose l'URL CDN complète (`egg_icon_url`) à partir du nom de fichier
   * renvoyé par l'API (`minecraft.webp`) — voir `config::EGG_ICON_BASE`. Repli **monogramme**
   * (1ʳᵉ lettre du jeu, fond coloré dérivé du nom) si l'URL manque ou si l'image ne charge pas.
   */
  let { src = null, name = "", size = 32 }: { src?: string | null; name?: string; size?: number } =
    $props();

  let failed = $state(false);
  const isUrl = $derived(!!src && /^https?:\/\//i.test(src));
  const mono = $derived((name || "?").trim().charAt(0).toUpperCase() || "?");

  function hue(s: string): number {
    let h = 0;
    for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) % 360;
    return h;
  }
  const h = $derived(hue(name || "?"));
</script>

{#if isUrl && !failed}
  <img src={src} alt="" width={size} height={size} onerror={() => (failed = true)} />
{:else}
  <span
    class="mono"
    style="width:{size}px;height:{size}px;font-size:{size * 0.5}px;
      background:linear-gradient(135deg, hsl({h} 48% 42%), hsl({(h + 32) % 360} 52% 32%))"
    aria-hidden="true">{mono}</span>
{/if}

<style>
  img,
  .mono {
    border-radius: 8px;
    flex: none;
    object-fit: cover;
    background: var(--elevated);
  }
  .mono {
    display: grid;
    place-items: center;
    color: #fff;
    font-weight: 700;
    line-height: 1;
  }
</style>
