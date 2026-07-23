<script lang="ts">
  import type { ServerListItem } from '$lib/types';
  import { statusMeta } from '$lib/status';
  import { tabs, VIEWS, type ServerView } from '$lib/tabs/tabs.svelte';
  import { serverColor } from '$lib/servers/colors.svelte';
  import { isDebug } from '$lib/debug.svelte';
  import { t } from '$lib/i18n';
  import StatusDot from './StatusDot.svelte';
  import Icon from './Icon.svelte';

  let { server }: { server: ServerListItem } = $props();

  const meta = $derived(statusMeta(server.status));
  const color = $derived(serverColor(server.id));
  const monogram = $derived((server.egg_name || server.name || '?').charAt(0).toUpperCase());
  let iconFailed = $state(false); // repli monogramme si l'icône du jeu ne charge pas
  const quick: ServerView[] = ['overview', 'console', 'sftp'];

  function open(view: ServerView) {
    tabs.focusOrOpen(server.id, server.name, view);
  }
</script>

<div class="card srv">
  <button class="head" onclick={() => open('overview')} title={t('view.overview')}>
    {#if server.egg_icon && !iconFailed}
      <img
        class="game-icon"
        style="--ring: {color ?? 'var(--brand-primary)'}"
        src={server.egg_icon}
        alt=""
        loading="lazy"
        onerror={() => (iconFailed = true)}
      />
    {:else}
      <span class="mono-icon" style="background: {color ?? 'var(--brand-gradient)'}" aria-hidden="true">
        {monogram}
      </span>
    {/if}
    <span class="titles">
      <span class="name">{server.name}</span>
      <span class="addr">{server.address}</span>
    </span>
    <span class="badge status" style="color: {meta.color}">
      <StatusDot color={meta.color} />{t(`status.${meta.key}`)}
    </span>
  </button>

  <div class="meta">
    <span class="chip">{server.egg_name}{server.bedrock ? ' · Bedrock' : ''}</span>
    {#if !server.owner}<span class="chip sub">{t('home.subUser')}</span>{/if}
    {#if isDebug()}
      <span class="chip dbg"
        >{server.capabilities.family} · mods={server.capabilities.mods}</span
      >
    {/if}
  </div>

  <div class="actions">
    {#each quick as view (view)}
      {@const vm = VIEWS.find((v) => v.id === view)}
      <button class="btn btn--ghost act" onclick={() => open(view)}>
        <span class="g"><Icon name={vm?.icon ?? 'gauge'} size={15} /></span>{t(`view.${view}`)}
      </button>
    {/each}
  </div>
</div>

<style>
  .srv {
    padding: 14px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 12px;
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
    font: inherit;
    color: inherit;
    text-align: left;
    width: 100%;
  }
  .mono-icon {
    width: 38px;
    height: 38px;
    border-radius: 9px;
    flex: none;
    display: grid;
    place-items: center;
    font-family: var(--font-mono);
    font-weight: 700;
    color: #fff;
  }
  .game-icon {
    width: 38px;
    height: 38px;
    border-radius: 9px;
    flex: none;
    object-fit: contain;
    background: var(--bg);
    padding: 3px;
    box-sizing: border-box;
    border: 2px solid color-mix(in srgb, var(--ring) 70%, transparent);
  }
  .titles {
    display: flex;
    flex-direction: column;
    min-width: 0;
    flex: 1;
  }
  .name {
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .addr {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text-dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .status {
    flex: none;
  }
  .meta {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chip {
    font-size: 11.5px;
    color: var(--text-muted);
    background: var(--elevated);
    border-radius: 999px;
    padding: 3px 9px;
  }
  .chip.sub {
    color: var(--brand-strator);
  }
  .chip.dbg {
    font-family: var(--font-mono);
    font-size: 10.5px;
    color: var(--text-dim);
    border: 1px dashed var(--border);
    background: none;
  }
  .actions {
    display: flex;
    gap: 6px;
  }
  .act {
    flex: 1;
    padding: 7px 8px;
    font-size: 12.5px;
    gap: 6px;
  }
  .act .g {
    display: inline-flex;
    align-items: center;
    opacity: 0.8;
  }
</style>
