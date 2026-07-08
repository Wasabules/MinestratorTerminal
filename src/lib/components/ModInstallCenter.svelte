<script lang="ts">
  /**
   * Indicateur global des installations de mods (bandeau) : spinner + nombre en cours, popover
   * listant chaque installation (phase + barre de progression). Alimenté par le store global
   * `installs.svelte.ts` (event `mods://install-progress` centralisé dans Workspace). Calqué sur
   * AlertCenter. Masqué tant qu'aucune installation n'a eu lieu.
   */
  import { t } from '$lib/i18n';
  import {
    runItems,
    activeRunCount,
    clearFinishedRuns,
    type InstallRun,
  } from '$lib/mods/installs.svelte';
  import Icon from './Icon.svelte';

  let open = $state(false);
  const active = $derived(activeRunCount());

  function pct(r: InstallRun): number {
    const frac = r.total > 0 ? r.done / r.total : 0;
    switch (r.phase) {
      case 'resolving':
        return 8;
      case 'downloading':
        return 10 + frac * 40;
      case 'stopping':
        return 55;
      case 'uploading':
        return 58 + frac * 34;
      case 'restarting':
        return 95;
      case 'done':
        return 100;
      default:
        return 0;
    }
  }
</script>

{#if runItems().length > 0}
  <div class="mc">
    <button
      class="trg"
      onclick={() => (open = !open)}
      title={t('ficsit.installsTitle')}
      aria-label={t('ficsit.installsTitle')}
    >
      <span class="ico">
        {#if active > 0}<span class="spinner"></span>{:else}<Icon name="download" size={17} />{/if}
      </span>
      {#if active > 0}<span class="badge">{active > 9 ? '9+' : active}</span>{/if}
    </button>

    {#if open}
      <button class="backdrop" aria-label={t('common.close')} onclick={() => (open = false)}></button>
      <div class="panel" role="menu">
        <div class="head">
          <span class="title">{t('ficsit.installsTitle')}</span>
          <button class="clear" onclick={clearFinishedRuns}>{t('ficsit.clear')}</button>
        </div>
        <div class="list">
          {#each runItems() as r (r.id)}
            <div class="item">
              <div class="top">
                <span class="lbl">{r.label}</span>
                {#if r.serverName}<span class="srv dim">{r.serverName}</span>{/if}
              </div>
              {#if r.status === 'error'}
                <div class="err">{r.error ?? t('ficsit.installErr')}</div>
              {:else}
                <div class="phz dim">{t(`ficsit.phase_${r.phase}`)}</div>
                <div class="pbar">
                  <div class="pfill" class:done={r.status === 'done'} style="width:{pct(r)}%"></div>
                </div>
              {/if}
            </div>
          {/each}
        </div>
      </div>
    {/if}
  </div>
{/if}

<style>
  .mc {
    position: relative;
    flex: none;
  }
  .trg {
    position: relative;
    background: none;
    border: 1px solid transparent;
    border-radius: var(--radius);
    cursor: pointer;
    padding: 5px 9px;
    color: var(--text-muted);
    line-height: 1;
  }
  .trg:hover {
    background: color-mix(in srgb, var(--text) 6%, transparent);
  }
  .ico {
    display: inline-flex;
    align-items: center;
  }
  .spinner {
    width: 15px;
    height: 15px;
    border: 2px solid var(--border);
    border-top-color: var(--brand-primary);
    border-radius: 50%;
    animation: spin 0.7s linear infinite;
    display: inline-block;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
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
    width: 320px;
    max-height: 420px;
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
  .clear {
    background: none;
    border: none;
    color: var(--text-dim);
    cursor: pointer;
    font: inherit;
    font-size: 12px;
  }
  .clear:hover {
    color: var(--text);
  }
  .list {
    overflow-y: auto;
  }
  .item {
    display: flex;
    flex-direction: column;
    gap: 5px;
    padding: 11px 14px;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 55%, transparent);
  }
  .item:last-child {
    border-bottom: none;
  }
  .top {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    align-items: baseline;
  }
  .lbl {
    font-weight: 600;
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .srv {
    font-size: 11px;
    flex: none;
  }
  .phz {
    font-size: 12px;
  }
  .pbar {
    height: 6px;
    border-radius: 999px;
    background: var(--surface);
    overflow: hidden;
  }
  .pfill {
    height: 100%;
    background: var(--brand-primary);
    border-radius: 999px;
    transition: width 0.4s ease;
  }
  .pfill.done {
    background: var(--state-running);
  }
  .err {
    font-size: 12px;
    color: var(--state-danger);
  }
</style>
