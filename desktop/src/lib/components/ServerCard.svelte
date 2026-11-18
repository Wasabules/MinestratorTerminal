<script lang="ts">
  import type { ServerListItem } from '#lib/types.ts';
  import { statusMeta } from '#lib/status.ts';
  import { tabs, VIEWS, type ServerView } from '#lib/tabs/tabs.svelte.ts';
  import { biomeVars, serverBiome } from '#lib/servers/colors.svelte.ts';
  import { isDebug } from '#lib/debug.svelte.ts';
  import { t } from '#lib/i18n/index.ts';
  import StatusDot from './StatusDot.svelte';
  import Icon from './Icon.svelte';
  import BiomeLandscape from './BiomeLandscape.svelte';
  import Masked from './Masked.svelte';

  let {
    server,
    players = null,
  }: {
    server: ServerListItem;
    /** Joueurs en ligne (appel groupé du parent) ; `null` tant qu'inconnu ou serveur inactif. */
    players?: { current: number; limit: number } | null;
  } = $props();

  const meta = $derived(statusMeta(server.status));
  const biome = $derived(serverBiome(server.id));
  const monogram = $derived((server.egg_name || server.name || '?').charAt(0).toUpperCase());
  /**
   * L'API renvoie la raison d'indisponibilité comme clé i18n du panel
   * (`PANEL.SERVERS_LIST.SERVER_IS_DISABLED_MYBOX`) : on traduit le dernier segment quand on le
   * connaît, et on n'affiche rien plutôt qu'une clé brute sinon.
   */
  const reason = $derived.by(() => {
    const key = server.unavailable_reason?.split('.').pop();
    if (!key) return null;
    const label = t(`status.reason.${key}`);
    return label === `status.reason.${key}` ? null : label;
  });
  let iconFailed = $state(false); // repli monogramme si l'icône du jeu ne charge pas
  const quick: ServerView[] = ['overview', 'console', 'sftp'];

  function open(view: ServerView) {
    tabs.focusOrOpen(server.id, server.name, view);
  }
</script>

<div class="card srv biome" style={biomeVars(biome)}>
  <!-- Fine bannière : le paysage du biome du serveur (même relief que son tableau de bord). -->
  <button class="cover" onclick={() => open('overview')} title={t('view.overview')} aria-label={server.name}>
    <BiomeLandscape {biome} seed={server.id} height={58} pixel={4} />
    <span class="state" style="color: {meta.ink}" title={reason ?? undefined}>
      <StatusDot color={meta.color} size={7} />{t(`status.${meta.key}`)}
    </span>
  </button>

  <div class="body">
    <button class="head" onclick={() => open('overview')} title={t('view.overview')}>
      {#if server.egg_icon_url && !iconFailed}
        <img
          class="game-icon"
          src={server.egg_icon_url}
          alt=""
          loading="lazy"
          onerror={() => (iconFailed = true)}
        />
      {:else}
        <span class="mono-icon" aria-hidden="true">{monogram}</span>
      {/if}
      <span class="titles">
        <span class="name">{server.name}</span>
        <span class="addr"><Masked value={server.address} kind="address" /></span>
      </span>
    </button>

    <div class="meta">
      <span class="chip">{server.egg_name}{server.bedrock ? ' · Bedrock' : ''}</span>
      {#if players && server.capabilities.players}
        <span class="chip players" title={t('home.playersOnline')}>
          <Icon name="users" size={12} stroke={2.2} /><span class="mono">{players.current}/{players.limit}</span>
        </span>
      {/if}
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
        <button class="act" onclick={() => open(view)}>
          <span class="g"><Icon name={vm?.icon ?? 'gauge'} size={15} stroke={2} /></span>{t(`view.${view}`)}
        </button>
      {/each}
    </div>
  </div>
</div>

<style>
  .srv {
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .cover {
    position: relative;
    display: block;
    padding: 0;
    border: none;
    background: none;
    cursor: pointer;
  }
  /* Pastille d'état posée sur le paysage : couleurs du thème sombre dans les deux thèmes. */
  .state {
    --state-running: #5fd68c;
    --state-running-text: #8fe8b0;
    --state-pending: #f2b84b;
    --state-offline: #6b7a7f;
    --state-danger: #ff6b63;
    --state-hibernate: #c79bff;
    --text-dim: #8c9a94;
    position: absolute;
    top: 9px;
    right: 10px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 3px 9px;
    border-radius: 999px;
    background: #0d1113;
    font: inherit;
    font-size: 12px;
    font-weight: 800;
  }
  .body {
    padding: 0 14px 14px;
    margin-top: -2px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .head {
    display: flex;
    align-items: flex-start;
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
  /* L'icône du jeu chevauche le bas de la bannière. */
  .mono-icon,
  .game-icon {
    position: relative; /* au-dessus du paysage (positionné) qu'elle chevauche */
    width: 44px;
    height: 44px;
    margin-top: -20px;
    border-radius: 10px;
    flex: none;
    border: 3px solid var(--surface);
    box-sizing: border-box;
  }
  .mono-icon {
    display: grid;
    place-items: center;
    font-weight: 900;
    font-size: 17px;
    background: var(--accent);
    color: var(--on-accent);
  }
  .game-icon {
    object-fit: contain;
    background: var(--inset);
    padding: 3px;
  }
  .titles {
    display: flex;
    flex-direction: column;
    min-width: 0;
    flex: 1;
    padding-top: 6px;
  }
  .name {
    font-weight: 900;
    font-size: 16px;
    line-height: 1.25;
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
  .meta {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chip {
    font-size: 12px;
    font-weight: 700;
    color: var(--text-muted);
    background: var(--inset);
    border-radius: 999px;
    padding: 3px 9px;
  }
  .chip.players {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
  .chip.players .mono {
    font-size: 11.5px;
  }
  .chip.sub {
    color: var(--state-hibernate);
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
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 7px 8px;
    border: none;
    border-radius: var(--radius);
    background: var(--btn-secondary-bg);
    color: var(--btn-secondary-ink);
    font: inherit;
    font-weight: 800;
    font-size: 12.5px;
    cursor: pointer;
  }
  .act:hover {
    background: var(--accent-soft);
    color: var(--accent-text);
  }
  .act .g {
    display: inline-flex;
    align-items: center;
    color: var(--accent-text);
  }
</style>
