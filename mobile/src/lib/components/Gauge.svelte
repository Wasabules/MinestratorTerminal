<script lang="ts">
  /**
   * Barre de ressource (aperçu serveur, MyBox) : libellé et pourcentage, barre, détail chiffré.
   * Couleur selon le seuil (charte) : vert < 70 %, ambre 70–90 %, rouge au-delà.
   */
  let {
    label,
    percent,
    detail,
  }: { label: string; percent: number; detail?: string } = $props();

  const pct = $derived(Math.max(0, Math.min(100, percent)));
  const color = $derived(
    pct > 90 ? 'var(--state-danger)' : pct > 70 ? 'var(--state-pending)' : 'var(--chart-cpu)'
  );
</script>

<div class="gauge">
  <div class="top">
    <span class="label">{label}</span>
    <span class="pct">{pct.toFixed(0)} %</span>
  </div>
  <div class="track">
    <!-- Largeur plancher : une barre vide reste visible comme barre. -->
    <div class="fill" style="width: {Math.max(pct, 2)}%; background: {color}"></div>
  </div>
  {#if detail}<span class="detail">{detail}</span>{/if}
</div>

<style>
  .gauge {
    display: flex;
    flex-direction: column;
    gap: 7px;
  }
  .top {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 10px;
  }
  .label {
    font-size: 14.5px;
    font-weight: 800;
  }
  .pct {
    font-family: var(--font-mono);
    font-size: 13px;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }
  .track {
    height: 10px;
    border-radius: 5px;
    background: var(--border-strong);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    border-radius: 5px;
    transition: width 0.4s ease;
  }
  @media (prefers-reduced-motion: reduce) {
    .fill {
      transition: none;
    }
  }
  .detail {
    font-size: 12px;
    color: var(--text-dim);
    font-family: var(--font-mono);
  }
</style>
