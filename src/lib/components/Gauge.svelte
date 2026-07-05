<script lang="ts">
  let {
    label,
    percent,
    detail,
  }: { label: string; percent: number; detail?: string } = $props();

  const pct = $derived(Math.max(0, Math.min(100, percent)));
  const color = $derived(
    pct > 90 ? 'var(--state-danger)' : pct > 70 ? 'var(--state-pending)' : 'var(--brand-primary)'
  );
</script>

<div class="gauge">
  <div class="top">
    <span class="label">{label}</span>
    <span class="pct">{pct.toFixed(0)}%</span>
  </div>
  <div class="track">
    <div class="fill" style="width: {pct}%; background: {color}"></div>
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
  }
  .label {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.07em;
    color: var(--text-muted);
    font-weight: 600;
  }
  .pct {
    font-family: var(--font-mono);
    font-size: 15px;
    font-weight: 700;
    font-variant-numeric: tabular-nums;
  }
  .track {
    height: 7px;
    border-radius: 4px;
    background: var(--elevated);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    border-radius: 4px;
    transition: width 0.4s ease;
  }
  @media (prefers-reduced-motion: reduce) {
    .fill {
      transition: none;
    }
  }
  .detail {
    font-size: 11.5px;
    color: var(--text-dim);
    font-family: var(--font-mono);
  }
</style>
