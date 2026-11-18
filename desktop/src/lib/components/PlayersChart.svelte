<script lang="ts">
  /**
   * Joueurs connectés sur la fenêtre de l'historique, d'après les statistiques à la minute de
   * l'hébergeur (le WebSocket ne les fournit pas : la surveillance locale ne les enregistre pas).
   * Paliers (le nombre de joueurs change par sauts), trous laissés vides comme sur le graphe
   * CPU / RAM. Chaque point regroupé garde le maximum de sa tranche.
   */
  import { t } from '#lib/i18n/index.ts';
  import type { PlayersPoint } from '#lib/types.ts';

  let {
    players,
    from,
    to,
    stepS,
  }: { players: PlayersPoint[]; from: number; to: number; stepS: number } = $props();

  const H = 92;
  const PAD_L = 30;
  const PAD_R = 10;
  const PAD_T = 8;
  const PAD_B = 8;

  let w = $state(600);
  const chartW = $derived(Math.max(1, w - PAD_L - PAD_R));
  const chartH = H - PAD_T - PAD_B;
  const t1 = $derived(to > from ? to : from + 1);
  const gapS = $derived(Math.max(3 * stepS, 150));

  const peak = $derived(players.reduce((m, p) => Math.max(m, p.count), 0));
  const avg = $derived(players.length ? players.reduce((s, p) => s + p.count, 0) / players.length : 0);
  /** Haut de l'axe : un palier « rond » au-dessus du pic (1, 2, 5, 10, 20, 50…). */
  const yMax = $derived.by(() => {
    const steps = [1, 2, 5];
    for (let mag = 1; ; mag *= 10) {
      for (const s of steps) if (s * mag >= peak) return s * mag;
    }
  });

  function x(ts: number): number {
    return PAD_L + ((ts - from) / (t1 - from)) * chartW;
  }
  function y(n: number): number {
    return PAD_T + (1 - Math.min(n, yMax) / yMax) * chartH;
  }

  /** Paliers : horizontale jusqu'au point suivant, puis verticale ; un trou coupe le tracé. */
  const path = $derived.by(() => {
    let d = '';
    let prev: PlayersPoint | null = null;
    for (const p of players) {
      if (!prev || p.ts - prev.ts > gapS) {
        if (prev) d += ` H${(x(prev.ts) + 2).toFixed(1)}`;
        d += ` M${x(p.ts).toFixed(1)} ${y(p.count).toFixed(1)}`;
      } else {
        d += ` H${x(p.ts).toFixed(1)} V${y(p.count).toFixed(1)}`;
      }
      prev = p;
    }
    if (prev) d += ` H${(x(prev.ts) + 2).toFixed(1)}`;
    return d.trim();
  });

  let hoverIdx = $state<number | null>(null);
  const hover = $derived(hoverIdx != null && hoverIdx < players.length ? players[hoverIdx] : null);
  function onMove(e: MouseEvent) {
    if (players.length === 0) return;
    const rect = (e.currentTarget as Element).getBoundingClientRect();
    const frac = Math.max(0, Math.min(1, (e.clientX - rect.left - PAD_L) / chartW));
    const ts = from + frac * (t1 - from);
    let best = 0;
    for (let i = 1; i < players.length; i++) {
      if (Math.abs(players[i].ts - ts) < Math.abs(players[best].ts - ts)) best = i;
    }
    hoverIdx = best;
  }
  function fmtTime(ts: number): string {
    const d = new Date(ts * 1000);
    return t1 - from > 86400
      ? d.toLocaleString(undefined, { day: '2-digit', month: 'short', hour: '2-digit', minute: '2-digit' })
      : d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  }
</script>

<div class="chart" bind:clientWidth={w}>
  <div class="head">
    <span class="title">{t('overview.playersHistory')}</span>
    <span class="stats dim">
      {t('overview.playersPeak', { n: peak })} · {t('overview.playersAvg', { n: avg.toFixed(1) })}
    </span>
  </div>
  <svg
    width={w}
    height={H}
    role="img"
    aria-label={t('overview.playersHistory')}
    onmousemove={onMove}
    onmouseleave={() => (hoverIdx = null)}
  >
    {#each [0, yMax] as g (g)}
      <line class="grid" x1={PAD_L} x2={w - PAD_R} y1={y(g)} y2={y(g)} />
      <text class="axis" x={PAD_L - 6} y={y(g) + 3} text-anchor="end">{g}</text>
    {/each}
    <path class="serie" d={path} />
    {#if hover}
      <line class="cross" x1={x(hover.ts)} x2={x(hover.ts)} y1={PAD_T} y2={PAD_T + chartH} />
      <circle class="dot" cx={x(hover.ts)} cy={y(hover.count)} r="3.5" />
    {/if}
  </svg>
  {#if hover}
    <div class="tip" class:flip={x(hover.ts) > w / 2} style="left: {x(hover.ts)}px">
      <span class="dim">{fmtTime(hover.ts)}</span>
      <b>{t('overview.playersAt', { n: hover.count })}</b>
    </div>
  {/if}
</div>

<style>
  .chart {
    width: 100%;
    position: relative;
  }
  .head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 4px;
    font-size: 12px;
  }
  .title {
    color: var(--text-muted);
    font-weight: 600;
  }
  .stats {
    font-family: var(--font-mono);
    font-size: 11.5px;
  }
  svg {
    display: block;
    cursor: crosshair;
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
    stroke: var(--brand-tertiary);
    stroke-width: 1.75;
    stroke-linejoin: round;
  }
  .cross {
    stroke: var(--text-dim);
    stroke-width: 1;
    stroke-dasharray: 3 3;
    pointer-events: none;
  }
  .dot {
    fill: var(--surface);
    stroke: var(--brand-tertiary);
    stroke-width: 2;
  }
  .tip {
    position: absolute;
    top: 18px;
    transform: translateX(9px);
    pointer-events: none;
    display: flex;
    gap: 8px;
    align-items: baseline;
    background: var(--elevated);
    border: 1px solid var(--border);
    border-radius: 7px;
    box-shadow: var(--shadow);
    padding: 5px 9px;
    font-size: 11.5px;
    white-space: nowrap;
    z-index: 5;
  }
  .tip.flip {
    transform: translateX(calc(-100% - 9px));
  }
  .tip b {
    color: var(--text);
  }
</style>
