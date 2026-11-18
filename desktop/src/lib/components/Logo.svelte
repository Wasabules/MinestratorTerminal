<script lang="ts">
  /**
   * Logo « curseur » : un chevron vert (l'invite d'une console) suivi d'un bloc corail (le
   * curseur). Le corail de la marque ne sert qu'ici. Sans fond : le conteneur fournit sa tuile.
   */
  let { size = 20 }: { size?: number } = $props();
</script>

<svg width={size} height={size} viewBox="0 0 32 32" aria-hidden="true">
  <path
    d="M8 10l6 6-6 6"
    fill="none"
    stroke="var(--brand-green)"
    stroke-width="3"
    stroke-linecap="round"
    stroke-linejoin="round"
  />
  <rect x="17" y="9" width="7" height="14" rx="1" fill="var(--brand-coral)" />
</svg>
