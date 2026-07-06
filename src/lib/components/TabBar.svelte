<script lang="ts">
  import { tabs, viewMeta, type ServerTab, type Tab } from '$lib/tabs/tabs.svelte';
  import { serverColor, setServerColor, SERVER_COLORS } from '$lib/servers/colors.svelte';
  import { serverRuntime } from '$lib/servers/runtime.svelte';
  import { runtimeMeta } from '$lib/status';
  import { detachTab, isPointerOutsideWindow } from '$lib/windows';
  import { t } from '$lib/i18n';
  import Icon from './Icon.svelte';

  type Menu = { x: number; y: number; tab: ServerTab };
  let menu = $state<Menu | null>(null);

  function openMenu(event: MouseEvent, tab: ServerTab) {
    event.preventDefault();
    menu = { x: event.clientX, y: event.clientY, tab };
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

  // Regroupe les onglets ADJACENTS du même serveur (visuel seul ; l'ordre à plat est conservé, donc
  // drag/fermeture/détachement inchangés). Chaque entrée garde son index à plat (pour `drop-before`).
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
          return;
        }
        if (last?.type === 'solo' && last.entry.tab.kind === 'server' && last.entry.tab.serverId === tab.serverId) {
          out[out.length - 1] = {
            type: 'group',
            serverId: tab.serverId,
            serverName: tab.serverName,
            entries: [{ tab: last.entry.tab, index: last.entry.index }, { tab, index }],
          };
          return;
        }
      }
      out.push({ type: 'solo', entry: { tab, index } });
    });
    return out;
  });
</script>

{#snippet tabEl(tab: Tab, index: number, grouped: boolean)}
  <div
    class="tab"
    class:active={tab.id === tabs.activeId}
    class:drop-before={drag?.moved && dropIndex === index}
    class:dragging={drag?.moved && drag?.id === tab.id}
    class:grouped
  >
    {#if !grouped && tab.kind === 'server' && serverColor(tab.serverId)}
      <span class="cstripe" style="background: {serverColor(tab.serverId)}"></span>
    {/if}
    <button
      class="tab-main"
      role="tab"
      aria-selected={tab.id === tabs.activeId}
      onpointerdown={(e) => onPointerDown(e, tab)}
      onpointermove={onPointerMove}
      onpointerup={(e) => void onPointerUp(e, tab)}
      oncontextmenu={(e) => {
        if (tab.kind === 'server') openMenu(e, tab);
      }}
      title={tab.kind === 'server'
        ? `${tab.serverName} · ${t(`view.${tab.view}`)}`
        : tab.kind === 'settings'
          ? t('settings.title')
          : tab.kind === 'copilot'
            ? t('copilot.title')
            : t('common.home')}
    >
      {#if tab.kind === 'home'}
        <span class="glyph"><Icon name="home" size={15} /></span>
        <span class="name">{t('common.home')}</span>
      {:else if tab.kind === 'settings'}
        <span class="glyph"><Icon name="settings" size={15} /></span>
        <span class="name">{t('settings.title')}</span>
      {:else if tab.kind === 'copilot'}
        <span class="glyph"><Icon name="activity" size={15} /></span>
        <span class="name">{t('copilot.title')}</span>
      {:else}
        {@const rt = serverRuntime(tab.serverId)}
        <span class="glyph">
          <Icon name={viewMeta(tab.view).icon} size={15} />
          <span
            class="sbadge"
            style="--sc: {rt ? runtimeMeta(rt).color : 'var(--text-dim)'}"
            title={t(`status.${rt ? runtimeMeta(rt).key : 'offline'}`)}
          ></span>
        </span>
        {#if grouped}
          <span class="name">{t(`view.${tab.view}`)}</span>
        {:else}
          <span class="name">{tab.serverName}</span>
          <span class="view">{t(`view.${tab.view}`)}</span>
        {/if}
      {/if}
    </button>
    {#if tab.kind !== 'home'}
      <button class="tab-close" title={t('tab.close')} onclick={() => tabs.close(tab.id)}>×</button>
    {/if}
  </div>
{/snippet}

<div class="tabbar" role="tablist" bind:this={bar}>
  {#each units as unit (unit.type === 'group' ? `g${unit.entries[0].tab.id}` : unit.entry.tab.id)}
    {#if unit.type === 'group'}
      {@const gcolor = serverColor(unit.serverId)}
      <div class="tgroup" class:colored={!!gcolor}>
        <span class="cstripe gstripe" style="background: {gcolor || 'color-mix(in srgb, var(--text) 28%, transparent)'}"></span>
        <button
          class="tchip"
          style={gcolor ? `background: color-mix(in srgb, ${gcolor} 22%, var(--surface))` : ''}
          title={unit.serverName}
          onclick={() => tabs.activate(unit.entries[0].tab.id)}
          oncontextmenu={(e) => openMenu(e, unit.entries[0].tab)}
        >{unit.serverName}</button>
        {#each unit.entries as e (e.tab.id)}
          {@render tabEl(e.tab, e.index, true)}
        {/each}
      </div>
    {:else}
      {@render tabEl(unit.entry.tab, unit.entry.index, false)}
    {/if}
  {/each}
  {#if drag?.moved && dropIndex === tabs.tabs.length}
    <span class="drop-end"></span>
  {/if}
</div>

{#if menu}
  {@const m = menu}
  <button class="backdrop" aria-label={t('common.close')} onclick={closeMenu}></button>
  <div class="ctx" style="left: {m.x}px; top: {m.y}px" role="menu">
    <button class="citem" onclick={() => run(() => tabs.close(m.tab.id))}>{t('tab.close')}</button>
    <button class="citem" onclick={() => run(() => tabs.closeOthers(m.tab.id))}>{t('tab.closeOthers')}</button>
    <button class="citem" onclick={() => run(() => tabs.closeRight(m.tab.id))}>{t('tab.closeRight')}</button>
    <button class="citem" onclick={() => run(() => tabs.closeLeft(m.tab.id))}>{t('tab.closeLeft')}</button>
    <div class="csep"></div>
    <button class="citem" onclick={() => { closeMenu(); void detach(m.tab); }}>{t('tab.detach')}</button>
    <div class="csep"></div>
    <div class="clabel">{t('tab.color')}</div>
    <div class="swatches">
      <button
        class="swatch none"
        title={t('tab.noColor')}
        onclick={() => run(() => setServerColor(m.tab.serverId, null))}
      >×</button>
      {#each SERVER_COLORS as c (c.key)}
        <button
          class="swatch"
          style="background: {c.hex}"
          class:sel={serverColor(m.tab.serverId) === c.hex}
          onclick={() => run(() => setServerColor(m.tab.serverId, c.hex))}
          aria-label={c.key}
        ></button>
      {/each}
    </div>
  </div>
{/if}

<style>
  .tabbar {
    display: flex;
    align-items: stretch;
    gap: 2px;
    flex: 1;
    min-width: 0;
    overflow-x: auto;
    scrollbar-width: none;
  }
  .tabbar::-webkit-scrollbar {
    display: none;
  }
  .tab {
    display: flex;
    align-items: center;
    border-radius: var(--radius) var(--radius) 0 0;
    max-width: 230px;
    flex: 0 0 auto;
    position: relative;
  }
  .tab.active {
    background: var(--surface);
  }
  .tab.active::after {
    content: '';
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 2px;
    background: var(--brand-gradient);
  }
  .tab.drop-before::before {
    content: '';
    position: absolute;
    left: -1px;
    top: 5px;
    bottom: 5px;
    width: 2px;
    border-radius: 2px;
    background: var(--brand-primary);
    z-index: 2;
  }
  .drop-end {
    width: 2px;
    align-self: stretch;
    margin: 6px 2px;
    border-radius: 2px;
    background: var(--brand-primary);
    flex: none;
  }
  .tab-main {
    display: flex;
    align-items: center;
    gap: 8px;
    background: none;
    border: none;
    cursor: pointer;
    font: inherit;
    color: var(--text-muted);
    padding: 10px 6px 10px 12px;
    min-width: 0;
    user-select: none;
    touch-action: none;
  }
  .tab.dragging {
    opacity: 0.55;
  }
  .tab.active .tab-main {
    color: var(--text);
  }
  .cstripe {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 3px;
    border-radius: var(--radius) var(--radius) 0 0;
    pointer-events: none;
    z-index: 1;
  }

  /* --- Groupe d'onglets d'un même serveur (adjacents) --- */
  .tgroup {
    display: flex;
    align-items: stretch;
    gap: 2px; /* onglets distincts (même écart que la barre) — la « liaison » = le trait du dessus */
    position: relative;
    flex: 0 0 auto;
  }
  .tchip {
    display: flex;
    align-items: center;
    max-width: 150px;
    background: var(--elevated);
    border: none;
    border-radius: var(--radius) var(--radius) 0 0;
    cursor: pointer;
    font: inherit;
    font-size: 12.5px;
    font-weight: 600;
    color: var(--text-muted);
    padding: 0 11px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    flex: none;
  }
  .tchip:hover {
    color: var(--text);
  }
  /* Trait continu au-dessus du groupe (par-dessus onglets ET espaces) = la « liaison » du groupe. */
  .gstripe {
    z-index: 2;
  }
  .glyph {
    display: inline-flex;
    align-items: center;
    opacity: 0.85;
    flex: none;
    position: relative;
  }
  /* Pastille d'état d'exécution (badge sur l'icône de vue) — anneau assorti au fond de l'onglet. */
  .sbadge {
    position: absolute;
    right: -3px;
    bottom: -3px;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--sc);
    box-shadow: 0 0 0 1.5px var(--bg);
  }
  .tab.active .sbadge {
    box-shadow: 0 0 0 1.5px var(--surface);
  }
  .name {
    font-weight: 600;
    font-size: 13px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .view {
    font-size: 11px;
    color: var(--text-dim);
    font-family: var(--font-mono);
    flex: none;
  }
  .tab-close {
    background: none;
    border: none;
    color: var(--text-dim);
    cursor: pointer;
    font-size: 16px;
    line-height: 1;
    padding: 4px 9px 4px 4px;
    border-radius: 4px;
  }
  .tab-close:hover {
    color: var(--text);
  }

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
    min-width: 210px;
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
    padding: 8px 11px;
    border-radius: 7px;
  }
  .citem:hover {
    background: color-mix(in srgb, var(--text) 8%, transparent);
  }
  .csep {
    height: 1px;
    background: var(--border);
    margin: 5px 0;
  }
  .clabel {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.07em;
    color: var(--text-dim);
    padding: 4px 11px;
  }
  .swatches {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 4px 8px 6px;
  }
  .swatch {
    width: 20px;
    height: 20px;
    border-radius: 6px;
    border: 2px solid transparent;
    cursor: pointer;
    padding: 0;
  }
  .swatch.sel {
    border-color: var(--text);
  }
  .swatch.none {
    background: var(--surface);
    border: 1px solid var(--border);
    color: var(--text-dim);
    display: grid;
    place-items: center;
    font-size: 13px;
  }
</style>
