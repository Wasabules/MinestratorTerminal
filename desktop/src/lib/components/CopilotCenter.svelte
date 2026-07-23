<script lang="ts">
  import { t } from '$lib/i18n';
  import { tabs } from '$lib/tabs/tabs.svelte';
  import { trigIcon } from '$lib/copilot/format';
  import Icon from './Icon.svelte';
  import {
    diagnosisItems,
    unreadDiagnoses,
    markDiagnosesRead,
    activeRuns,
    activeCount,
  } from '$lib/copilot/diagnoses.svelte';

  let open = $state(false);

  const count = $derived(unreadDiagnoses());
  const busy = $derived(activeCount());

  function toggle() {
    open = !open;
    if (open) markDiagnosesRead();
  }

  function openTab() {
    tabs.openCopilot();
    open = false;
  }
</script>

<div class="cc">
  <button
    class="bell"
    class:busy={busy > 0}
    onclick={toggle}
    title={busy > 0 ? t('copilot.analyzing') : t('copilot.title')}
    aria-label={t('copilot.title')}
  >
    <span class="ico"><Icon name="activity" size={17} /></span>
    {#if busy > 0}
      <span class="pulse" aria-hidden="true"></span>
    {:else if count > 0}
      <span class="badge">{count > 9 ? '9+' : count}</span>
    {/if}
  </button>

  {#if open}
    <button class="backdrop" aria-label={t('common.close')} onclick={() => (open = false)}></button>
    <div class="panel" role="menu">
      <div class="head">
        <span class="title">{t('copilot.title')}</span>
        <button class="link" onclick={openTab}>{t('copilot.openTab')}</button>
      </div>

      {#if busy > 0}
        <div class="grp">
          {#each activeRuns() as r (r.id)}
            <div class="run">
              <div class="run-top">
                <span class="ti"><Icon name={trigIcon(r.trigger)} size={15} /></span>
                <span class="srv">{r.server_name}</span>
              </div>
              <div class="bar"><span class="bar-fill"></span></div>
              <div class="phase dim">{r.phase || t('copilot.analyzing')}</div>
            </div>
          {/each}
        </div>
      {/if}

      {#if diagnosisItems().length === 0 && busy === 0}
        <div class="empty dim">{t('copilot.empty')}</div>
      {:else if diagnosisItems().length > 0}
        <div class="list">
          {#each diagnosisItems().slice(0, 6) as d (d.id)}
            <button class="item" onclick={openTab}>
              <span class="ti"><Icon name={trigIcon(d.trigger)} size={15} /></span>
              <div class="body">
                <span class="srv">{d.server_name}</span>
                <span class="summary dim">{d.summary}</span>
              </div>
            </button>
          {/each}
        </div>
        <button class="all" onclick={openTab}>{t('copilot.openTab')} →</button>
      {/if}
    </div>
  {/if}
</div>

<style>
  .cc {
    position: relative;
    flex: none;
  }
  .bell {
    position: relative;
    background: none;
    border: 1px solid transparent;
    border-radius: var(--radius);
    cursor: pointer;
    padding: 5px 9px;
    color: var(--text-muted);
    font-size: 15px;
    line-height: 1;
  }
  .bell:hover {
    background: color-mix(in srgb, var(--text) 6%, transparent);
  }
  .badge {
    position: absolute;
    top: -2px;
    right: -2px;
    min-width: 16px;
    height: 16px;
    padding: 0 4px;
    border-radius: 999px;
    background: var(--brand-primary);
    color: #fff;
    font-size: 10px;
    font-weight: 700;
    display: grid;
    place-items: center;
    box-shadow: 0 0 0 2px var(--surface);
  }
  /* Point « live » qui bat (comme la pastille de notification), coin haut-droit de l'icône. */
  .pulse {
    position: absolute;
    top: 2px;
    right: 2px;
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--brand-primary);
    box-shadow: 0 0 0 2px var(--bg);
    pointer-events: none;
    animation: beat 1.3s ease-in-out infinite;
  }
  .pulse::after {
    content: '';
    position: absolute;
    inset: 0;
    border-radius: 50%;
    background: var(--brand-primary);
    animation: ping 1.3s cubic-bezier(0, 0, 0.2, 1) infinite;
  }
  @keyframes beat {
    0%,
    100% {
      transform: scale(1);
    }
    50% {
      transform: scale(1.2);
    }
  }
  @keyframes ping {
    0% {
      transform: scale(1);
      opacity: 0.6;
    }
    100% {
      transform: scale(2.8);
      opacity: 0;
    }
  }
  @keyframes slide {
    0% {
      transform: translateX(-120%);
    }
    100% {
      transform: translateX(340%);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .pulse,
    .pulse::after,
    .bar-fill {
      animation: none;
    }
    .pulse::after {
      display: none;
    }
    .bar-fill {
      width: 100%;
    }
  }
  .backdrop {
    position: fixed;
    inset: 0;
    background: none;
    border: none;
    z-index: 30;
    cursor: default;
  }
  .panel {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 31;
    width: 340px;
    max-height: 460px;
    display: flex;
    flex-direction: column;
    background: var(--elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow);
    overflow: hidden;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 14px;
    border-bottom: 1px solid var(--border);
  }
  .title {
    font-weight: 700;
    font-size: 14px;
  }
  .link {
    background: none;
    border: none;
    color: var(--brand-primary);
    cursor: pointer;
    font: inherit;
    font-size: 12px;
  }
  .empty {
    padding: 30px 16px;
    text-align: center;
    font-size: 13px;
  }
  .grp {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px 14px;
    border-bottom: 1px solid var(--border);
    background: color-mix(in srgb, var(--brand-primary) 6%, transparent);
  }
  .run {
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .run-top {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .ico {
    display: inline-flex;
    align-items: center;
  }
  .ti {
    display: inline-flex;
    align-items: center;
    color: var(--text-muted);
    flex: none;
  }
  .srv {
    font-weight: 600;
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .bar {
    height: 4px;
    border-radius: 3px;
    background: color-mix(in srgb, var(--brand-primary) 16%, transparent);
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
  .phase {
    font-size: 11.5px;
  }
  .list {
    overflow-y: auto;
  }
  .item {
    display: flex;
    gap: 10px;
    width: 100%;
    text-align: left;
    background: none;
    border: none;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 55%, transparent);
    cursor: pointer;
    font: inherit;
    color: inherit;
    padding: 10px 14px;
    align-items: flex-start;
  }
  .item:hover {
    background: color-mix(in srgb, var(--text) 4%, transparent);
  }
  .body {
    min-width: 0;
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .summary {
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .all {
    background: none;
    border: none;
    border-top: 1px solid var(--border);
    color: var(--brand-primary);
    cursor: pointer;
    font: inherit;
    font-size: 12.5px;
    padding: 10px;
    text-align: center;
  }
  .all:hover {
    background: color-mix(in srgb, var(--brand-primary) 8%, transparent);
  }
</style>
