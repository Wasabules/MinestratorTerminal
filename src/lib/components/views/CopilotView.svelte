<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from '$lib/i18n';
  import { renderMarkdown } from '$lib/markdown';
  import { ago, elapsed, trigIcon, trigLabel } from '$lib/copilot/format';
  import ActionList from '../ActionList.svelte';
  import Icon from '../Icon.svelte';
  import {
    diagnosisItems,
    activeRuns,
    clearDiagnoses,
    markDiagnosesRead,
  } from '$lib/copilot/diagnoses.svelte';

  let expanded = $state<Record<string, boolean>>({});
  // Force la ré-évaluation des durées « il y a … » chaque seconde.
  let now = $state(Date.now());

  onMount(() => {
    markDiagnosesRead();
    const iv = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(iv);
  });
</script>

<div class="page">
  <header class="head">
    <div class="ttl">
      <span class="ico"><Icon name="activity" size={20} /></span>
      <h1>{t('copilot.title')}</h1>
    </div>
    {#if diagnosisItems().length > 0}
      <button class="clear" onclick={clearDiagnoses}>{t('alerts.clear')}</button>
    {/if}
  </header>

  {#if activeRuns().length > 0}
    <section class="sec">
      <h2>{t('copilot.inProgress')}</h2>
      <div class="list">
        {#each activeRuns() as r (r.id)}
          <div class="card running">
            <div class="row">
              <span class="trig"><Icon name={trigIcon(r.trigger)} size={14} /> {trigLabel(r.trigger)}</span>
              <span class="srv">{r.server_name}</span>
              <span class="time dim">{elapsed(now, r.started)}</span>
            </div>
            <div class="bar"><span class="bar-fill"></span></div>
            <div class="phase">{r.phase || t('copilot.analyzing')}</div>
            {#if r.log.length > 1}
              <ul class="log">
                {#each r.log.slice(0, -1) as step, i (i)}
                  <li class="done-step">{step}</li>
                {/each}
              </ul>
            {/if}
          </div>
        {/each}
      </div>
    </section>
  {/if}

  <section class="sec">
    <h2>{t('copilot.historyTitle')}</h2>
    {#if diagnosisItems().length === 0}
      <div class="empty dim">{t('copilot.empty')}</div>
    {:else}
      <div class="list">
        {#each diagnosisItems() as d (d.id)}
          <div class="card">
            <button class="chead" onclick={() => (expanded[d.id] = !expanded[d.id])}>
              <span class="chev">{expanded[d.id] ? '▾' : '▸'}</span>
              <span class="trig"><Icon name={trigIcon(d.trigger)} size={14} /> {trigLabel(d.trigger)}</span>
              <div class="chead-body">
                <div class="top">
                  <span class="srv">{d.server_name}</span>
                  <span class="time dim">{ago(now, d.ts)}</span>
                </div>
                <div class="summary">{d.summary}</div>
              </div>
            </button>

            {#if expanded[d.id]}
              <div class="detail">
                {#if d.cause}
                  <div class="sec-label">{t('copilot.cause')}</div>
                  <div class="prose md">{@html renderMarkdown(d.cause)}</div>
                {/if}
                {#if d.suggested_fix}
                  <div class="sec-label">{t('copilot.fix')}</div>
                  <div class="prose md">{@html renderMarkdown(d.suggested_fix)}</div>
                {/if}

                {#if d.actions.length > 0}
                  <div class="sec-label">{t('copilot.actions')}</div>
                  <ActionList actions={d.actions} serverId={d.server_id} />
                {/if}

                {#if d.log.length > 0}
                  <div class="sec-label">{t('copilot.log')}</div>
                  <ul class="log">
                    {#each d.log as step, i (i)}<li class="done-step">{step}</li>{/each}
                  </ul>
                {/if}
              </div>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  </section>
</div>

<style>
  .page {
    max-width: 820px;
    margin: 0 auto;
    padding: 26px 24px 60px;
    display: flex;
    flex-direction: column;
    gap: 26px;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
  }
  .ttl {
    display: flex;
    align-items: center;
    gap: 11px;
  }
  .ttl h1 {
    margin: 0;
    font-size: 23px;
    letter-spacing: -0.02em;
  }
  .ico {
    display: inline-flex;
    align-items: center;
    color: var(--brand-primary);
  }
  .clear {
    background: none;
    border: 1px solid var(--border);
    border-radius: 8px;
    color: var(--text-dim);
    cursor: pointer;
    font: inherit;
    font-size: 12.5px;
    padding: 6px 12px;
  }
  .clear:hover {
    color: var(--text);
  }
  .sec {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .sec h2 {
    margin: 0;
    font-size: 13px;
    text-transform: uppercase;
    letter-spacing: 0.09em;
    color: var(--text-muted);
    font-family: var(--font-mono);
  }
  .empty {
    padding: 30px 0;
    text-align: center;
    font-size: 13.5px;
  }
  .list {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .card {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    overflow: hidden;
  }
  .card.running {
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 9px;
    border-color: color-mix(in srgb, var(--brand-primary) 40%, var(--border));
  }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .trig {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-muted);
    white-space: nowrap;
    flex: none;
  }
  .srv {
    font-weight: 600;
    font-size: 13.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .row .time {
    margin-left: auto;
  }
  .time {
    font-size: 11.5px;
    flex: none;
  }
  .bar {
    height: 5px;
    border-radius: 3px;
    background: color-mix(in srgb, var(--brand-primary) 15%, transparent);
    overflow: hidden;
  }
  .bar-fill {
    display: block;
    height: 100%;
    width: 35%;
    border-radius: 3px;
    background: var(--brand-gradient, var(--brand-primary));
    animation: slide 1.3s ease-in-out infinite;
  }
  @keyframes slide {
    0% {
      transform: translateX(-120%);
    }
    100% {
      transform: translateX(340%);
    }
  }
  .phase {
    font-size: 12.5px;
    color: var(--text);
  }
  .log {
    margin: 2px 0 0;
    padding-left: 16px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .done-step {
    font-size: 11.5px;
    color: var(--text-dim);
    list-style: '✓ ';
  }
  .chead {
    display: flex;
    gap: 10px;
    width: 100%;
    text-align: left;
    background: none;
    border: none;
    cursor: pointer;
    font: inherit;
    color: inherit;
    padding: 13px 16px;
    align-items: flex-start;
  }
  .chead:hover {
    background: color-mix(in srgb, var(--text) 4%, transparent);
  }
  .chev {
    color: var(--text-dim);
    font-size: 12px;
    margin-top: 2px;
    flex: none;
  }
  .chead-body {
    flex: 1;
    min-width: 0;
  }
  .top {
    display: flex;
    justify-content: space-between;
    gap: 10px;
  }
  .summary {
    font-size: 13px;
    color: var(--text-muted);
    margin-top: 3px;
  }
  .detail {
    padding: 2px 16px 16px 40px;
  }
  .sec-label {
    font-size: 10.5px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--text-dim);
    font-family: var(--font-mono);
    margin: 14px 0 6px;
  }
  .prose {
    font-size: 13px;
    line-height: 1.6;
    color: var(--text-muted);
    word-break: break-word;
  }
</style>
