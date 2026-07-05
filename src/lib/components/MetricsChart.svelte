<script lang="ts">
  import { t } from '$lib/i18n';
  import type { MetricSample } from '$lib/types';

  let {
    samples,
    cpuLimit,
  }: { samples: MetricSample[]; cpuLimit: number } = $props();

  const H = 150;
  const PAD_L = 30;
  const PAD_R = 10;
  const PAD_T = 10;
  const PAD_B = 12;

  let w = $state(600);

  const chartW = $derived(Math.max(1, w - PAD_L - PAD_R));
  const chartH = H - PAD_T - PAD_B;

  function clamp(v: number): number {
    return Math.max(0, Math.min(100, v));
  }
  function cpuPct(s: MetricSample): number {
    return cpuLimit > 0 ? (s.cpu / cpuLimit) * 100 : 0;
  }
  function ramPct(s: MetricSample): number {
    return s.mem_limit > 0 ? (s.mem / s.mem_limit) * 100 : 0;
  }

  const bounds = $derived.by(() => {
    if (samples.length === 0) return { t0: 0, t1: 1 };
    const t0 = samples[0].ts;
    const t1 = samples[samples.length - 1].ts;
    return { t0, t1: t1 > t0 ? t1 : t0 + 1 };
  });

  function x(ts: number): number {
    return PAD_L + ((ts - bounds.t0) / (bounds.t1 - bounds.t0)) * chartW;
  }
  function y(pct: number): number {
    return PAD_T + (1 - clamp(pct) / 100) * chartH;
  }

  function line(fn: (s: MetricSample) => number): string {
    return samples.map((s) => `${x(s.ts).toFixed(1)},${y(fn(s)).toFixed(1)}`).join(' ');
  }

  const cpuLine = $derived(line(cpuPct));
  const ramLine = $derived(line(ramPct));
  const last = $derived(samples.length ? samples[samples.length - 1] : null);
  const gridY = [0, 50, 100];

  // --- Survol : point le plus proche du curseur + infobulle ---
  let hoverIdx = $state<number | null>(null);
  const hover = $derived(hoverIdx != null && hoverIdx < samples.length ? samples[hoverIdx] : null);

  function nearestIndex(ts: number): number {
    // Recherche binaire (samples triés par ts croissant).
    let lo = 0;
    let hi = samples.length - 1;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (samples[mid].ts < ts) lo = mid + 1;
      else hi = mid;
    }
    if (lo > 0 && Math.abs(samples[lo - 1].ts - ts) <= Math.abs(samples[lo].ts - ts)) return lo - 1;
    return lo;
  }
  function onMove(e: MouseEvent) {
    if (samples.length === 0) return;
    const rect = (e.currentTarget as Element).getBoundingClientRect();
    const frac = Math.max(0, Math.min(1, (e.clientX - rect.left - PAD_L) / chartW));
    hoverIdx = nearestIndex(bounds.t0 + frac * (bounds.t1 - bounds.t0));
  }
  function onLeave() {
    hoverIdx = null;
  }
  function fmtTime(ts: number): string {
    const d = new Date(ts * 1000);
    return bounds.t1 - bounds.t0 > 86400
      ? d.toLocaleString(undefined, { day: '2-digit', month: 'short', hour: '2-digit', minute: '2-digit' })
      : d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  }
</script>

<div class="chart" bind:clientWidth={w}>
  {#if samples.length === 0}
    <div class="empty dim">{t('overview.noHistory')}</div>
  {:else}
    <svg
      width={w}
      height={H}
      role="img"
      aria-label="{t('overview.history')} CPU / RAM"
      onmousemove={onMove}
      onmouseleave={onLeave}
    >
      {#each gridY as g (g)}
        <line class="grid" x1={PAD_L} x2={w - PAD_R} y1={y(g)} y2={y(g)} />
        <text class="axis" x={PAD_L - 6} y={y(g) + 3} text-anchor="end">{g}</text>
      {/each}
      <polyline class="serie cpu" points={cpuLine} />
      <polyline class="serie ram" points={ramLine} />
      {#if hover}
        <line class="cross" x1={x(hover.ts)} x2={x(hover.ts)} y1={PAD_T} y2={H - PAD_B} />
        <circle class="dot cpu" cx={x(hover.ts)} cy={y(cpuPct(hover))} r="3.5" />
        <circle class="dot ram" cx={x(hover.ts)} cy={y(ramPct(hover))} r="3.5" />
      {:else if last}
        <circle class="dot cpu" cx={x(last.ts)} cy={y(cpuPct(last))} r="3" />
        <circle class="dot ram" cx={x(last.ts)} cy={y(ramPct(last))} r="3" />
      {/if}
    </svg>
    {#if hover}
      <div class="tip" class:flip={x(hover.ts) > w / 2} style="left: {x(hover.ts)}px">
        <div class="tip-time">{fmtTime(hover.ts)}</div>
        <div class="tip-row">
          <span class="sw cpu"></span><span class="lbl">{t('overview.cpu')}</span>
          <b>{cpuPct(hover).toFixed(0)}%</b>
        </div>
        <div class="tip-row">
          <span class="sw ram"></span><span class="lbl">{t('overview.ram')}</span>
          <b>{ramPct(hover).toFixed(0)}%</b>
        </div>
      </div>
    {/if}
  {/if}

  <div class="legend">
    <span class="item"><span class="sw cpu"></span>{t('overview.cpu')}
      {#if last}<b>{cpuPct(last).toFixed(0)}%</b>{/if}</span>
    <span class="item"><span class="sw ram"></span>{t('overview.ram')}
      {#if last}<b>{ramPct(last).toFixed(0)}%</b>{/if}</span>
  </div>
</div>

<style>
  .chart {
    width: 100%;
    position: relative;
  }
  .empty {
    height: 150px;
    display: grid;
    place-items: center;
    font-size: 13px;
    text-align: center;
    padding: 0 20px;
  }
  svg {
    display: block;
    cursor: crosshair;
  }
  .cross {
    stroke: var(--text-dim);
    stroke-width: 1;
    stroke-dasharray: 3 3;
    pointer-events: none;
  }
  .tip {
    position: absolute;
    top: 4px;
    transform: translateX(9px);
    pointer-events: none;
    background: var(--elevated);
    border: 1px solid var(--border);
    border-radius: 7px;
    box-shadow: var(--shadow);
    padding: 6px 9px;
    font-size: 11.5px;
    white-space: nowrap;
    z-index: 5;
  }
  .tip.flip {
    transform: translateX(calc(-100% - 9px));
  }
  .tip-time {
    color: var(--text-dim);
    font-family: var(--font-mono);
    font-size: 10.5px;
    margin-bottom: 4px;
  }
  .tip-row {
    display: flex;
    align-items: center;
    gap: 7px;
  }
  .tip-row .lbl {
    color: var(--text-muted);
  }
  .tip-row b {
    color: var(--text);
    font-family: var(--font-mono);
    margin-left: auto;
    padding-left: 12px;
  }
  .grid {
    stroke: var(--border);
    stroke-width: 1;
  }
  .axis {
    fill: var(--text-dim);
    font-size: 10px;
    font-family: var(--font-mono);
  }
  .serie {
    fill: none;
    stroke-width: 1.75;
    stroke-linejoin: round;
    stroke-linecap: round;
  }
  .serie.cpu,
  .dot.cpu {
    stroke: var(--brand-primary);
  }
  .serie.ram,
  .dot.ram {
    stroke: var(--brand-accent);
  }
  .dot {
    fill: var(--surface);
    stroke-width: 2;
  }
  .legend {
    display: flex;
    gap: 18px;
    margin-top: 6px;
    padding-left: 30px;
    font-size: 12px;
    color: var(--text-muted);
  }
  .item {
    display: inline-flex;
    align-items: center;
    gap: 7px;
  }
  .item b {
    color: var(--text);
    font-family: var(--font-mono);
  }
  .sw {
    width: 14px;
    height: 3px;
    border-radius: 2px;
    display: inline-block;
  }
  .sw.cpu {
    background: var(--brand-primary);
  }
  .sw.ram {
    background: var(--brand-accent);
  }
</style>
