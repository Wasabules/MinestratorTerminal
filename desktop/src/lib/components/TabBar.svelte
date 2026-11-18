<script lang="ts">
  import { tabs, viewMeta, type ServerTab, type Tab } from '#lib/tabs/tabs.svelte.ts';
  import {
    BIOMES,
    biomeVars,
    hasChosenBiome,
    serverBiome,
    setServerBiome,
    type BiomeKey,
  } from '#lib/servers/colors.svelte.ts';
  import { serverRuntime } from '#lib/servers/runtime.svelte.ts';
  import { runtimeMeta } from '#lib/status.ts';
  import { isCompactTabs } from '#lib/tabs/mode.svelte.ts';
  import { detachTab, isPointerOutsideWindow } from '#lib/windows.ts';
  import { t } from '#lib/i18n/index.ts';
  import Icon from './Icon.svelte';
  import BiomeBlock from './BiomeBlock.svelte';
  import BiomeLandscape from './BiomeLandscape.svelte';

  type Menu = { x: number; y: number; tab: ServerTab };
  let menu = $state<Menu | null>(null);
  // Mode compact : onglets réduits aux icônes de vue (le nom du serveur reste en tête de groupe).
  const compact = $derived(isCompactTabs());

  /** Dimensions approximatives du menu, pour le garder dans la fenêtre. */
  const MENU_W = 304;
  const MENU_H = 430;

  function openMenuAt(x: number, y: number, tab: ServerTab) {
    menu = {
      x: Math.max(8, Math.min(x, window.innerWidth - MENU_W - 8)),
      y: Math.max(8, Math.min(y, window.innerHeight - MENU_H - 8)),
      tab,
    };
  }
  function openMenu(event: MouseEvent, tab: ServerTab) {
    event.preventDefault();
    openMenuAt(event.clientX, event.clientY, tab);
  }
  /** Le chevron de la tête de groupe ouvre le même menu que le clic droit, sous lui. */
  function openMenuFrom(event: MouseEvent, tab: ServerTab) {
    const r = (event.currentTarget as HTMLElement).getBoundingClientRect();
    openMenuAt(r.left, r.bottom + 4, tab);
  }
  function closeMenu() {
    menu = null;
  }
  function run(fn: () => void) {
    fn();
    closeMenu();
  }

  async function detach(tab: ServerTab) {
    await detachTab({ serverId: tab.serverId, serverName: tab.serverName, view: tab.view });
    tabs.close(tab.id);
  }

  // Réordonnancement + détachement par événements POINTEUR (le drag HTML5 est cassé
  // par l'intercepteur drag-drop natif de Tauri, requis par ailleurs pour le SFTP).
  let bar: HTMLElement;
  let drag = $state<{ id: string; startX: number; moved: boolean } | null>(null);
  let dropIndex = $state<number | null>(null);

  function computeDropIndex(clientX: number): number {
    const els = Array.from(bar.querySelectorAll<HTMLElement>('.tab'));
    for (let i = 0; i < els.length; i++) {
      const r = els[i].getBoundingClientRect();
      if (clientX < r.left + r.width / 2) return Math.max(1, i);
    }
    return els.length; // après le dernier onglet
  }

  function onPointerDown(event: PointerEvent, tab: Tab) {
    if (event.button !== 0) return; // clic gauche uniquement
    drag = { id: tab.id, startX: event.clientX, moved: false };
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
  }

  function onPointerMove(event: PointerEvent) {
    if (!drag) return;
    const t = tabs.tabs.find((x) => x.id === drag!.id);
    if (!t || t.kind !== 'server') return; // Home non déplaçable
    if (!drag.moved && Math.abs(event.clientX - drag.startX) < 5) return; // seuil anti-clic
    drag.moved = true;
    dropIndex = computeDropIndex(event.clientX);
  }

  async function onPointerUp(event: PointerEvent, tab: Tab) {
    try {
      (event.currentTarget as HTMLElement).releasePointerCapture(event.pointerId);
    } catch {
      /* capture déjà relâchée */
    }
    const d = drag;
    const di = dropIndex;
    drag = null;
    dropIndex = null;
    if (!d) return;
    if (!d.moved) {
      tabs.activate(tab.id); // simple clic
      return;
    }
    if (tab.kind === 'server') {
      if (await isPointerOutsideWindow(event.screenX, event.screenY)) await detach(tab);
      else if (di !== null) tabs.moveTo(d.id, di);
    }
  }

  // Regroupe les onglets ADJACENTS du même serveur derrière une tête de groupe (visuel seul ;
  // l'ordre à plat est conservé, donc drag/fermeture/détachement inchangés). Un onglet serveur
  // seul forme un groupe d'un onglet : la tête porte le nom, l'onglet la vue. Chaque entrée garde
  // son index à plat (pour `drop-before`).
  type Entry = { tab: Tab; index: number };
  type Unit =
    | { type: 'solo'; entry: Entry }
    | { type: 'group'; serverId: number; serverName: string; entries: { tab: ServerTab; index: number }[] };
  const units = $derived.by(() => {
    const out: Unit[] = [];
    tabs.tabs.forEach((tab, index) => {
      const last = out[out.length - 1];
      if (tab.kind === 'server') {
        if (last?.type === 'group' && last.serverId === tab.serverId) {
          last.entries.push({ tab, index });
        } else {
          out.push({ type: 'group', serverId: tab.serverId, serverName: tab.serverName, entries: [{ tab, index }] });
        }
        return;
      }
      out.push({ type: 'solo', entry: { tab, index } });
    });
    return out;
  });

  /** Libellé d'un onglet (infobulle, nom accessible). */
  function tabTitle(tab: Tab): string {
    switch (tab.kind) {
      case 'server':
        return `${tab.serverName} · ${t(`view.${tab.view}`)}`;
      case 'settings':
        return t('settings.title');
      case 'copilot':
        return t('copilot.title');
      case 'notifications':
        return t('notifications.title');
      case 'myboxes':
        return t('mybox.title');
      default:
        return t('common.home');
    }
  }

  /** Icône et libellé des onglets neutres (hors serveur). */
  const NEUTRAL: Record<Exclude<Tab['kind'], 'server'>, { icon: string; key: string }> = {
    home: { icon: 'home', key: 'common.home' },
    settings: { icon: 'settings', key: 'settings.title' },
    copilot: { icon: 'activity', key: 'copilot.title' },
    notifications: { icon: 'bell', key: 'notifications.title' },
    myboxes: { icon: 'cpu', key: 'mybox.title' },
  };

  function chooseBiome(serverId: number, key: BiomeKey | null) {
    run(() => setServerBiome(serverId, key));
  }
</script>

{#snippet tabEl(tab: Tab, index: number)}
  <div
    class="tab"
    class:neutral={tab.kind !== 'server'}
    class:active={tab.id === tabs.activeId}
    class:drop-before={drag?.moved && dropIndex === index}
    class:dragging={drag?.moved && drag?.id === tab.id}
  >
    <button
      class="tab-main"
      class:noclose={tab.kind === 'home'}
      role="tab"
      aria-selected={tab.id === tabs.activeId}
      aria-label={tabTitle(tab)}
      onpointerdown={(e) => onPointerDown(e, tab)}
      onpointermove={onPointerMove}
      onpointerup={(e) => void onPointerUp(e, tab)}
      oncontextmenu={(e) => {
        if (tab.kind === 'server') openMenu(e, tab);
      }}
      title={tabTitle(tab)}
    >
      {#if tab.kind === 'server'}
        <span class="glyph"><Icon name={viewMeta(tab.view).icon} size={15} stroke={2} /></span>
        {#if !compact}<span class="name">{t(`view.${tab.view}`)}</span>{/if}
      {:else}
        {@const n = NEUTRAL[tab.kind]}
        <span class="glyph"><Icon name={n.icon} size={15} stroke={2} /></span>
        <!-- Accueil : icône seule (onglet permanent, toujours en tête). -->
        {#if !compact && tab.kind !== 'home'}<span class="name">{t(n.key)}</span>{/if}
      {/if}
    </button>
    {#if tab.kind !== 'home'}
      <button class="tab-close" title={t('tab.close')} aria-label={t('tab.close')} onclick={() => tabs.close(tab.id)}
        >×</button
      >
    {/if}
  </div>
{/snippet}

<div class="tabbar" role="tablist" bind:this={bar}>
  {#each units as unit (unit.type === 'group' ? `g${unit.entries[0].tab.id}` : unit.entry.tab.id)}
    {#if unit.type === 'group'}
      {@const b = serverBiome(unit.serverId)}
      {@const grt = serverRuntime(unit.serverId)}
      {@const first = unit.entries[0].tab}
      <div class="tgroup biome" style={biomeVars(b)}>
        <div class="ghead">
          <button
            class="gname"
            title={unit.serverName}
            onclick={() => tabs.activate(first.id)}
            oncontextmenu={(e) => openMenu(e, first)}
          >
            <span class="gblock">
              <BiomeBlock biome={b} size={14} />
              <span
                class="sbadge"
                style="--sc: {grt ? runtimeMeta(grt).color : 'var(--state-offline)'}"
                title={t(`status.${grt ? runtimeMeta(grt).key : 'offline'}`)}
              ></span>
            </span>
            <span class="gtext">{unit.serverName}</span>
          </button>
          <button
            class="gcaret"
            aria-label={t('tab.menu', { name: unit.serverName })}
            title={t('tab.menu', { name: unit.serverName })}
            onclick={(e) => openMenuFrom(e, first)}>▾</button
          >
        </div>
        {#each unit.entries as e (e.tab.id)}
          {@render tabEl(e.tab, e.index)}
        {/each}
      </div>
    {:else}
      {@render tabEl(unit.entry.tab, unit.entry.index)}
    {/if}
  {/each}
  {#if drag?.moved && dropIndex === tabs.tabs.length}
    <span class="drop-end"></span>
  {/if}
</div>

{#if menu}
  {@const m = menu}
  {@const current = serverBiome(m.tab.serverId)}
  {@const chosen = hasChosenBiome(m.tab.serverId)}
  <button class="backdrop" aria-label={t('common.close')} onclick={closeMenu}></button>
  <div class="ctx" style="left: {m.x}px; top: {m.y}px" role="menu">
    <button class="citem" onclick={() => run(() => tabs.close(m.tab.id))}>{t('tab.close')}</button>
    <button class="citem" onclick={() => run(() => tabs.closeOthers(m.tab.id))}>{t('tab.closeOthers')}</button>
    <button class="citem" onclick={() => run(() => tabs.closeRight(m.tab.id))}>{t('tab.closeRight')}</button>
    <button class="citem" onclick={() => run(() => tabs.closeLeft(m.tab.id))}>{t('tab.closeLeft')}</button>
    <div class="csep"></div>
    <button class="citem" onclick={() => { closeMenu(); void detach(m.tab); }}>{t('tab.detach')}</button>
    <div class="csep"></div>
    <div class="clabel">
      <span>{t('tab.biome')}</span>
      {#if chosen}
        <button class="cauto" title={t('tab.biomeAutoHint')} onclick={() => chooseBiome(m.tab.serverId, null)}>
          {t('tab.biomeAuto')}
        </button>
      {/if}
    </div>
    <div class="biomes" role="group" aria-label={t('tab.biome')}>
      {#each BIOMES as bio (bio.key)}
        <button
          class="bcard"
          class:sel={current.key === bio.key}
          style="--sel: {bio.accent}; --sel-l: {bio.accentLight}"
          aria-pressed={current.key === bio.key}
          onclick={() => chooseBiome(m.tab.serverId, bio.key)}
        >
          <BiomeLandscape biome={bio} seed={m.tab.serverId} height={36} pixel={3} />
          <span class="bname">{t(`biome.${bio.key}`)}</span>
        </button>
      {/each}
    </div>
  </div>
{/if}

<style>
  .tabbar {
    display: flex;
    align-items: flex-end;
    gap: 4px;
    flex: 1;
    min-width: 0;
    overflow-x: auto;
    scrollbar-width: none;
  }
  .tabbar::-webkit-scrollbar {
    display: none;
  }

  /* --- Onglet en intercalaire : bord haut à l'accent du biome, actif = couleur de la page --- */
  .tab {
    --stripe: var(--accent);
    display: flex;
    align-items: center;
    height: 36px;
    border-radius: var(--radius-tab) var(--radius-tab) 0 0;
    max-width: 220px;
    flex: 0 0 auto;
    position: relative;
    background: var(--inset);
    box-shadow: inset 0 3px 0 var(--stripe);
  }
  .tab.neutral {
    --stripe: var(--tab-neutral);
  }
  .tab.active {
    background: var(--bg);
  }
  .tab.drop-before::before {
    content: '';
    position: absolute;
    left: -3px;
    top: 6px;
    bottom: 6px;
    width: 2px;
    border-radius: 2px;
    background: var(--brand-primary);
    z-index: 2;
  }
  .drop-end {
    width: 2px;
    height: 24px;
    margin: 0 2px 6px;
    border-radius: 2px;
    background: var(--brand-primary);
    flex: none;
  }
  .tab-main {
    display: flex;
    align-items: center;
    gap: 7px;
    height: 100%;
    background: none;
    border: none;
    cursor: pointer;
    font: inherit;
    font-weight: 800;
    font-size: 13.5px;
    color: var(--text-muted);
    padding: 0 4px 0 12px;
    min-width: 0;
    user-select: none;
    touch-action: none;
  }
  /* Onglet sans bouton fermer (Accueil) : padding droit symétrique. */
  .tab-main.noclose {
    padding-right: 12px;
  }
  .tab-main:hover,
  .tab.active .tab-main {
    color: var(--text);
  }
  .tab.dragging {
    opacity: 0.55;
  }
  .glyph {
    display: inline-flex;
    align-items: center;
    flex: none;
  }
  .name {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .tab-close {
    background: none;
    border: none;
    color: var(--text-dim);
    cursor: pointer;
    font: inherit;
    font-size: 15px;
    line-height: 1;
    padding: 4px 10px 4px 5px;
    border-radius: 6px;
  }
  .tab-close:hover {
    color: var(--text);
  }

  /* --- Groupe d'onglets d'un même serveur : tête (bloc du biome + nom), puis ses onglets --- */
  .tgroup {
    display: flex;
    align-items: flex-end;
    gap: 3px;
    flex: 0 0 auto;
    margin-left: 6px;
  }
  .ghead {
    display: flex;
    align-items: center;
    align-self: center;
    margin: 0 2px 4px 0;
  }
  .gname {
    display: flex;
    align-items: center;
    gap: 7px;
    max-width: 170px;
    background: none;
    border: none;
    cursor: pointer;
    font: inherit;
    font-weight: 900;
    font-size: 13.5px;
    color: var(--accent);
    padding: 3px 2px 3px 4px;
    border-radius: 6px;
  }
  .gname:hover {
    background: var(--hover);
  }
  .gblock {
    position: relative;
    display: inline-flex;
    flex: none;
  }
  /* Pastille d'état d'exécution du serveur (une seule fois pour tout le groupe). */
  .sbadge {
    position: absolute;
    right: -3px;
    bottom: -3px;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--sc);
    box-shadow: 0 0 0 1.5px var(--chrome);
  }
  .gtext {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .gcaret {
    background: none;
    border: none;
    cursor: pointer;
    font: inherit;
    font-size: 10px;
    color: var(--accent);
    opacity: 0.8;
    padding: 4px 5px;
    border-radius: 6px;
  }
  .gcaret:hover {
    opacity: 1;
    background: var(--hover);
  }

  /* --- Menu contextuel --- */
  .backdrop {
    position: fixed;
    inset: 0;
    background: none;
    border: none;
    z-index: 40;
    cursor: default;
  }
  .ctx {
    position: fixed;
    z-index: 41;
    width: 304px;
    background: var(--elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: var(--shadow);
    padding: 5px;
    display: flex;
    flex-direction: column;
  }
  .citem {
    text-align: left;
    background: none;
    border: none;
    cursor: pointer;
    font: inherit;
    color: var(--text);
    padding: 7px 11px;
    border-radius: 7px;
  }
  .citem:hover {
    background: var(--hover);
  }
  .csep {
    height: 1px;
    background: var(--border);
    margin: 5px 0;
  }
  .clabel {
    display: flex;
    align-items: center;
    justify-content: space-between;
    font-size: 12px;
    font-weight: 800;
    color: var(--text-muted);
    padding: 4px 11px;
  }
  .cauto {
    background: none;
    border: none;
    cursor: pointer;
    font: inherit;
    font-size: 12px;
    color: var(--brand-primary);
    padding: 0;
  }
  .cauto:hover {
    text-decoration: underline;
  }
  .biomes {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 6px;
    padding: 4px 6px 6px;
  }
  /* Choix de biome : mini-paysage + nom ; la sélection prend l'accent du biome. */
  .bcard {
    display: flex;
    flex-direction: column;
    padding: 0;
    border-radius: 10px;
    overflow: hidden;
    border: 2px solid var(--border-strong);
    background: var(--surface);
    cursor: pointer;
    font: inherit;
    text-align: left;
  }
  .bcard:hover {
    border-color: var(--text-dim);
  }
  .bcard.sel {
    border-color: var(--sel);
  }
  :global(:root[data-theme='light']) .bcard.sel {
    border-color: var(--sel-l);
  }
  .bname {
    padding: 4px 7px 5px;
    font-size: 12px;
    font-weight: 800;
    color: var(--text);
  }
</style>
