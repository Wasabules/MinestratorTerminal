<script lang="ts">
  /**
   * Palette de commandes (Ctrl+K, ⌘K sur macOS) : recherche floue sur les serveurs, leurs vues et
   * actions, la navigation et les préférences. Les entrées viennent du registre
   * (`#lib/palette/registry.svelte.ts`) : ce composant n'en connaît aucune.
   *
   * Rendu : bouton de la barre supérieure (raccourci visible) + `<dialog>` natif ouvert en
   * `showModal()` — couche supérieure, reste de l'app inerte, `aria-modal`. Le champ est un
   * combobox ARIA qui pilote une listbox par `aria-activedescendant` : le focus reste dans le champ,
   * piégé dans la palette (Tab ouvre une sous-page au lieu de sortir), puis rendu à l'élément
   * d'origine à la fermeture.
   *
   * Clavier : ↑/↓ (PageUp/PageDown) déplacent la sélection, Entrée exécute, Tab ou → ouvre la
   * sous-page d'une entrée (actions d'un serveur), Maj+Tab ou Retour arrière (champ vide) revient,
   * Échap ferme (ou annule une confirmation). `>` en tête de requête : mode commande console.
   */
  import { onMount, tick, untrack } from 'svelte';
  import { t } from '#lib/i18n/index.ts';
  import { humanizeError } from '#lib/ipc.ts';
  import { fuzzyMatch, highlight } from '#lib/palette/fuzzy.ts';
  import {
    closePalette,
    isPaletteShortcut,
    modeFor,
    openPalette,
    paletteModes,
    paletteShortcutAria,
    paletteShortcutLabel,
    paletteSources,
    paletteUi,
    togglePalette,
    type PaletteEntry,
    type PalettePage,
  } from '#lib/palette/registry.svelte.ts';
  import { initPaletteRecents, pushRecent, recentIds } from '#lib/palette/recents.svelte.ts';
  import { ensurePaletteServers, installBuiltinPaletteSources } from '#lib/palette/sources.svelte.ts';
  import Icon from './Icon.svelte';

  installBuiltinPaletteSources();

  type Row = {
    entry: PaletteEntry;
    titleHits?: number[];
    subtitleHits?: number[];
    /** Section d'origine, affichée en recherche (« Serveurs », « Réglages »…). */
    section?: string;
  };
  type Group = { id: string; label: string | null; rows: Row[] };

  /** Au-delà, la recherche doit être affinée : garde la liste (et le DOM) légère. */
  const MAX_RESULTS = 60;
  const RECENTS_SHOWN = 5;

  const ui = paletteUi();
  const shortcut = paletteShortcutLabel();

  let dialog = $state<HTMLDialogElement>();
  let input = $state<HTMLInputElement>();
  let confirmBtn = $state<HTMLButtonElement>();
  let cancelBtn = $state<HTMLButtonElement>();

  let query = $state('');
  /** Sous-pages ouvertes ; `prevQuery` est la requête à restaurer au retour. */
  let stack = $state.raw<{ page: PalettePage; prevQuery: string }[]>([]);
  let active = $state(0);
  /** Entrée en attente de confirmation (remplace la liste). */
  let confirming = $state.raw<PaletteEntry | null>(null);
  /** Entrée en cours d'exécution (asynchrone) : la palette attend son résultat. */
  let busyId = $state<string | null>(null);
  let status = $state<{ text: string; error: boolean } | null>(null);
  let brokenImages = $state<Record<string, true>>({});

  /** Élément focalisé avant l'ouverture, à qui rendre le focus. */
  let restoreFocus: HTMLElement | null = null;
  /** Le clic a commencé sur le fond : un glisser depuis le champ ne doit pas fermer. */
  let downOnBackdrop = false;
  /** Horodatage du dernier Échap traité au clavier (évite un double traitement par `cancel`). */
  let escAt = -1;

  onMount(() => {
    initPaletteRecents();
    // Phase de capture : le raccourci passe avant les champs qui consomment le clavier (xterm,
    // CodeMirror, saisie console).
    const onKey = (e: KeyboardEvent) => {
      if (!isPaletteShortcut(e)) return;
      e.preventDefault();
      e.stopPropagation();
      togglePalette();
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  });

  // --- Ouverture / fermeture ---

  // Chaque ouverture (y compris une réouverture préremplie, `openPalette({ query: '>' })`) :
  // mémorise le focus d'origine et repart d'un état propre.
  $effect(() => {
    if (!ui.open) return;
    void ui.seq;
    untrack(() => {
      if (!restoreFocus) {
        const el = document.activeElement;
        restoreFocus = el instanceof HTMLElement && !dialog?.contains(el) ? el : null;
      }
      query = ui.query;
      stack = [];
      active = 0;
      confirming = null;
      status = null;
      ensurePaletteServers();
      input?.focus();
    });
  });

  // Le `<dialog>` n'existe que palette ouverte : on le passe en modal dès qu'il est monté.
  $effect(() => {
    if (!dialog) return;
    if (!dialog.open) dialog.showModal();
    input?.focus();
  });

  // Fermeture : rend le focus à l'élément d'origine s'il est encore affiché.
  $effect(() => {
    if (ui.open) return;
    const el = restoreFocus;
    restoreFocus = null;
    if (el?.isConnected) el.focus({ preventScroll: true });
  });

  function close() {
    closePalette();
  }

  // --- Contenu ---

  const page = $derived(stack.length ? stack[stack.length - 1].page : null);
  const mode = $derived(page ? undefined : modeFor(query));
  const modePage = $derived(mode ? mode.page() : null);
  const current = $derived(page ?? modePage);
  /** Requête vue par la page courante : sans le préfixe du mode. */
  const pageQuery = $derived(mode ? query.slice(mode.prefix.length).trimStart() : query);

  function recentBonus(id: string): number {
    const i = recentIds().indexOf(id);
    return i === -1 ? 0 : 3 - i * 0.3;
  }

  function rank(items: Row[], q: string): Row[] {
    const scored: { row: Row; score: number; i: number }[] = [];
    items.forEach((row, i) => {
      const e = row.entry;
      const m = fuzzyMatch(q, [e.title, e.subtitle, e.keywords?.join(' ')]);
      if (!m) return;
      scored.push({
        row: { ...row, titleHits: m.hits[0], subtitleHits: m.hits[1] },
        score: m.score + (e.boost ?? 0) + recentBonus(e.id),
        i,
      });
    });
    scored.sort((a, b) => b.score - a.score || a.i - b.i);
    return scored.slice(0, MAX_RESULTS).map((s) => s.row);
  }

  const groups = $derived.by((): Group[] => {
    if (!ui.open || confirming) return [];
    if (current) {
      const rows = current.entries(pageQuery).map((entry) => ({ entry }));
      const filtered = current.filter === false || !pageQuery.trim() ? rows : rank(rows, pageQuery);
      return [{ id: current.id, label: null, rows: filtered }];
    }
    const sections = paletteSources().map((s) => ({ id: s.id, label: s.label(), entries: s.entries() }));
    if (query.trim()) {
      const all = sections.flatMap((s) => s.entries.map((entry) => ({ entry, section: s.label })));
      return [{ id: 'results', label: null, rows: rank(all, query) }];
    }
    // Requête vide : récents en tête, puis chaque section (sans les entrées « recherche seule »).
    const byId = new Map<string, PaletteEntry>();
    for (const s of sections) for (const e of s.entries) if (!byId.has(e.id)) byId.set(e.id, e);
    const recents = recentIds()
      .map((id) => byId.get(id))
      .filter((e): e is PaletteEntry => e !== undefined)
      .slice(0, RECENTS_SHOWN);
    const shown = new Set(recents.map((e) => e.id));
    const out: Group[] = [];
    if (recents.length) {
      out.push({ id: 'recent', label: t('palette.groups.recent'), rows: recents.map((entry) => ({ entry })) });
    }
    for (const s of sections) {
      const rows = s.entries.filter((e) => !e.searchOnly && !shown.has(e.id)).map((entry) => ({ entry }));
      if (rows.length) out.push({ id: s.id, label: s.label, rows });
    }
    return out;
  });

  const rows = $derived(groups.flatMap((g) => g.rows));
  const activeIndex = $derived(rows.length ? Math.min(active, rows.length - 1) : -1);
  const activeRow = $derived(activeIndex >= 0 ? rows[activeIndex] : null);
  /** Index à plat du premier élément de chaque groupe (ids d'options stables). */
  const offsets = $derived.by(() => {
    let n = 0;
    return groups.map((g) => {
      const at = n;
      n += g.rows.length;
      return at;
    });
  });
  const searching = $derived(!current && query.trim().length > 0);
  const placeholder = $derived(current?.placeholder ?? t('palette.placeholder'));
  const crumbs = $derived([...(mode ? [mode.label()] : []), ...stack.map((s) => s.page.title)]);

  // Garde la sélection visible au clavier.
  $effect(() => {
    const i = activeIndex;
    if (i < 0 || !dialog) return;
    dialog.querySelector(`#pal-opt-${i}`)?.scrollIntoView({ block: 'nearest' });
  });

  // --- Actions ---

  function remember(entry: PaletteEntry) {
    if (entry.noRecent || entry.danger || entry.confirm || entry.disabled) return;
    // Seules les entrées retrouvables à la racine ont un sens dans les récents.
    const known = paletteSources().some((s) => s.entries().some((e) => e.id === entry.id));
    if (known) pushRecent(entry.id);
  }

  function pushPage(entry: PaletteEntry) {
    if (!entry.page || entry.disabled) return;
    remember(entry);
    stack = [...stack, { page: entry.page(), prevQuery: query }];
    query = '';
    active = 0;
    status = null;
  }

  /** Revient à la page précédente, avec la requête qui y était tapée (ex. `> surv`). */
  function popPage() {
    if (!stack.length) return;
    query = stack[stack.length - 1].prevQuery;
    stack = stack.slice(0, -1);
    active = 0;
    status = null;
  }

  async function activate(entry: PaletteEntry, confirmed = false) {
    if (entry.disabled || busyId) return;
    if (!entry.run) {
      pushPage(entry);
      return;
    }
    if (entry.confirm && !confirmed) {
      confirming = entry;
      status = null;
      await tick();
      // Action irréversible : le focus va sur « Annuler », un second Entrée réflexe ne la lance pas.
      (entry.danger ? cancelBtn : confirmBtn)?.focus();
      return;
    }
    confirming = null;
    busyId = entry.id;
    status = null;
    try {
      const outcome = await entry.run();
      remember(entry);
      if (outcome?.keepOpen) {
        if (outcome.query !== undefined) query = outcome.query;
        status = outcome.message ? { text: outcome.message, error: false } : null;
        active = 0;
        await tick();
        input?.focus();
      } else {
        close();
      }
    } catch (e) {
      status = { text: humanizeError(e), error: true };
      await tick();
      input?.focus();
    } finally {
      busyId = null;
    }
  }

  async function cancelConfirm() {
    confirming = null;
    await tick();
    input?.focus();
  }

  // --- Clavier ---

  function onInputKeydown(e: KeyboardEvent) {
    if (e.isComposing) return;
    const n = rows.length;
    const entry = activeRow?.entry;
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        if (n) active = (activeIndex + 1) % n;
        break;
      case 'ArrowUp':
        e.preventDefault();
        if (n) active = (activeIndex - 1 + n) % n;
        break;
      case 'PageDown':
        e.preventDefault();
        if (n) active = Math.min(activeIndex + 8, n - 1);
        break;
      case 'PageUp':
        e.preventDefault();
        if (n) active = Math.max(activeIndex - 8, 0);
        break;
      case 'Enter':
        e.preventDefault();
        if (entry) void activate(entry);
        break;
      case 'Tab':
        // Focus piégé : Tab ne quitte jamais la palette, il descend dans une sous-page.
        e.preventDefault();
        if (e.shiftKey) popPage();
        else if (entry?.page) pushPage(entry);
        break;
      case 'ArrowRight':
        if (entry?.page && input?.selectionStart === query.length && input.selectionEnd === query.length) {
          e.preventDefault();
          pushPage(entry);
        }
        break;
      case 'Backspace':
        if (query === '' && stack.length) {
          e.preventDefault();
          popPage();
        }
        break;
    }
  }

  function onDialogKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      escAt = e.timeStamp;
      if (confirming) void cancelConfirm();
      else close();
      return;
    }
    // Confirmation : Tab circule entre « Annuler » et « Confirmer » sans quitter la palette.
    if (e.key === 'Tab' && confirming && dialog) {
      const focusables = [...dialog.querySelectorAll<HTMLElement>('.confirm button:not(:disabled)')];
      if (!focusables.length) return;
      e.preventDefault();
      const i = focusables.indexOf(document.activeElement as HTMLElement);
      const next = e.shiftKey ? (i <= 0 ? focusables.length - 1 : i - 1) : (i + 1) % focusables.length;
      focusables[next].focus();
    }
  }

  /** Échap natif du `<dialog>` : déjà traité au clavier, sinon ferme. */
  function onCancel(e: Event) {
    e.preventDefault();
    if (e.timeStamp - escAt < 200) return;
    close();
  }

  /** Clic hors de la palette (sur le fond) : ferme. */
  function onBackdropClick(e: MouseEvent) {
    if (e.target === dialog && downOnBackdrop) close();
    downOnBackdrop = false;
  }

  /** Un clic dans la palette ne doit pas voler le focus du champ (sauf boutons). */
  function keepFocus(e: MouseEvent) {
    const target = e.target as HTMLElement;
    if (!target.closest('input, button')) e.preventDefault();
  }

  function onInput() {
    active = 0;
    status = null;
  }
</script>

<button
  class="trigger"
  onclick={() => openPalette()}
  title={t('palette.openHint', { keys: shortcut })}
  aria-label={t('palette.openHint', { keys: shortcut })}
  aria-haspopup="dialog"
  aria-keyshortcuts={paletteShortcutAria()}
>
  <Icon name="search" size={15} />
  <span class="tlabel">{t('palette.search')}</span>
  <kbd>{shortcut}</kbd>
</button>

{#snippet text(value: string, hits: number[] | undefined)}
  {#each highlight(value, hits) as seg, i (i)}{#if seg.hit}<mark>{seg.text}</mark>{:else}{seg.text}{/if}{/each}
{/snippet}

{#if ui.open}
  <dialog
    bind:this={dialog}
    class="palette"
    aria-label={t('palette.title')}
    onkeydown={onDialogKeydown}
    oncancel={onCancel}
    onclose={() => ui.open && close()}
    onpointerdown={(e) => (downOnBackdrop = e.target === dialog)}
    onclick={onBackdropClick}
  >
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="pal" onmousedown={keepFocus}>
      <div class="search">
        {#if crumbs.length}
          <button
            class="back"
            tabindex="-1"
            aria-label={t('palette.back')}
            title={t('palette.back')}
            onclick={() => {
              if (stack.length) popPage();
              else query = '';
              input?.focus();
            }}
          >
            <Icon name="arrow-up" size={14} class="rot" />
          </button>
          {#each crumbs as c, i (i)}
            <span class="crumb">{c}</span>
          {/each}
        {:else}
          <span class="sicon"><Icon name="search" size={17} /></span>
        {/if}
        <input
          bind:this={input}
          bind:value={query}
          class="field"
          type="text"
          role="combobox"
          aria-expanded={!confirming}
          aria-controls="pal-list"
          aria-autocomplete="list"
          aria-activedescendant={!confirming && activeIndex >= 0 ? `pal-opt-${activeIndex}` : undefined}
          aria-label={placeholder}
          {placeholder}
          spellcheck="false"
          autocomplete="off"
          disabled={!!confirming}
          oninput={onInput}
          onkeydown={onInputKeydown}
        />
        <kbd class="esc">Esc</kbd>
      </div>

      {#if confirming}
        {@const c = confirming.confirm!}
        <div class="confirm" role="alertdialog" aria-labelledby="pal-confirm-title" aria-describedby="pal-confirm-desc">
          <h2 id="pal-confirm-title" class:danger={confirming.danger}>{c.title}</h2>
          <div id="pal-confirm-desc">
            {#if c.warning}
              <p class="warn">
                <span class="wi"><Icon name="triangle-alert" size={15} /></span>
                <span>{c.warning}</span>
              </p>
            {/if}
            {#if c.detail}<p class="detail">{c.detail}</p>{/if}
          </div>
          <div class="cactions">
            <button bind:this={cancelBtn} class="btn btn--ghost" onclick={() => void cancelConfirm()}>
              {t('common.cancel')}
            </button>
            <button
              bind:this={confirmBtn}
              class="btn"
              class:btn-danger={confirming.danger}
              onclick={() => confirming && void activate(confirming, true)}
            >
              {c.confirmLabel}
            </button>
          </div>
        </div>
      {:else}
        <div id="pal-list" class="list" role="listbox" aria-label={t('palette.title')}>
          {#each groups as g, gi (g.id)}
            <div class="group" role="group" aria-labelledby={g.label ? `pal-g-${g.id}` : undefined}>
              {#if g.label}
                <div class="glabel" id="pal-g-{g.id}" role="presentation">{g.label}</div>
              {/if}
              {#each g.rows as row, ri (row.entry.id)}
                {@const i = offsets[gi] + ri}
                {@const e = row.entry}
                <!-- Le clavier passe par le combobox (aria-activedescendant) : l'option ne reçoit
                     jamais le focus, le clic n'est qu'un raccourci souris. -->
                <!-- svelte-ignore a11y_click_events_have_key_events -->
                <div
                  id="pal-opt-{i}"
                  class="opt"
                  class:active={i === activeIndex}
                  class:danger={e.danger}
                  class:disabled={e.disabled}
                  role="option"
                  tabindex="-1"
                  aria-selected={i === activeIndex}
                  aria-disabled={e.disabled || undefined}
                  onmousemove={() => (active = i)}
                  onclick={() => void activate(e)}
                >
                  <span class="oicon">
                    {#if e.image && !brokenImages[e.image]}
                      <img
                        src={e.image}
                        alt=""
                        loading="lazy"
                        onerror={() => (brokenImages = { ...brokenImages, [e.image!]: true })}
                      />
                    {:else}
                      <Icon name={e.icon ?? 'zap'} size={16} />
                    {/if}
                    {#if e.dot}<span class="odot" style="background: {e.dot}"></span>{/if}
                  </span>
                  <span class="otext">
                    <span class="otitle">{@render text(e.title, row.titleHits)}</span>
                    {#if e.subtitle}
                      <span class="osub">{@render text(e.subtitle, row.subtitleHits)}</span>
                    {/if}
                  </span>
                  {#if busyId === e.id}
                    <span class="spinner" aria-label={t('palette.running')}></span>
                  {:else if e.hint}
                    <span class="ohint">{e.hint}</span>
                  {/if}
                  {#if searching && row.section}<span class="osec">{row.section}</span>{/if}
                  {#if e.page && i === activeIndex && !e.disabled}
                    <kbd class="otab" title={t('palette.keys.open')}>Tab</kbd>
                  {/if}
                </div>
              {/each}
            </div>
          {/each}
          {#if rows.length === 0}
            <div class="empty">{t('palette.empty')}</div>
          {/if}
        </div>
      {/if}

      {#if status}
        <div class="status" class:error={status.error}>{status.text}</div>
      {/if}
      <!-- Région vivante toujours présente (sinon le premier message n'est pas annoncé). -->
      <div class="sr" role="status" aria-live="polite">
        {status ? status.text : searching ? t('palette.results', { n: rows.length }) : ''}
      </div>

      <footer class="keys" aria-hidden="true">
        {#if confirming}
          <span><kbd>Tab</kbd> {t('palette.keys.switch')}</span>
          <span><kbd>↵</kbd> {t('palette.keys.run')}</span>
          <span class="spacer"></span>
          <span><kbd>Esc</kbd> {t('palette.keys.cancel')}</span>
        {:else}
          <span><kbd>↑</kbd><kbd>↓</kbd> {t('palette.keys.navigate')}</span>
          <span><kbd>↵</kbd> {t('palette.keys.run')}</span>
          <span><kbd>Tab</kbd> {t('palette.keys.open')}</span>
          {#if stack.length}<span><kbd>⌫</kbd> {t('palette.keys.back')}</span>{/if}
          {#if !current}
            {#each paletteModes() as m (m.prefix)}
              <span><kbd>{m.prefix}</kbd> {m.label()}</span>
            {/each}
          {/if}
          <span class="spacer"></span>
          <span><kbd>Esc</kbd> {t('palette.keys.close')}</span>
        {/if}
      </footer>
    </div>
  </dialog>
{/if}

<style>
  /* --- Bouton d'ouverture (barre supérieure) --- */
  .trigger {
    display: flex;
    align-items: center;
    gap: 7px;
    background: color-mix(in srgb, var(--text) 4%, transparent);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    cursor: pointer;
    font: inherit;
    font-size: 12.5px;
    color: var(--text-dim);
    padding: 4px 6px 4px 9px;
    flex: none;
  }
  .trigger:hover {
    color: var(--text);
    border-color: color-mix(in srgb, var(--text) 18%, var(--border));
  }
  .tlabel {
    padding-right: 6px;
  }
  @media (max-width: 960px) {
    .tlabel {
      display: none;
    }
  }
  kbd {
    font-family: var(--font-sans);
    font-size: 10.5px;
    font-weight: 600;
    line-height: 1;
    color: var(--text-dim);
    background: var(--surface);
    border: 1px solid var(--border);
    border-bottom-width: 2px;
    border-radius: 5px;
    padding: 3px 5px;
    white-space: nowrap;
  }

  /* --- Fenêtre --- */
  .palette {
    --pal-hit: var(--brand-primary);
    --pal-active: color-mix(in srgb, var(--brand-primary) 15%, transparent);
    width: min(640px, calc(100vw - 32px));
    max-width: none;
    margin: 12vh auto auto;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--elevated);
    color: var(--text);
    box-shadow: 0 24px 60px rgba(0, 0, 0, 0.45);
    overflow: hidden;
  }
  :global(:root[data-theme='light']) .palette {
    --pal-hit: var(--brand-primary-700);
    --pal-active: color-mix(in srgb, var(--brand-primary) 10%, transparent);
    box-shadow: 0 24px 60px rgba(15, 23, 42, 0.18);
  }
  /* `::backdrop` n'hérite pas toujours des variables de :root (WebView anciens) : valeurs en dur. */
  .palette::backdrop {
    background: rgba(0, 0, 0, 0.45);
    backdrop-filter: blur(2px);
  }
  :global(:root[data-theme='light']) .palette::backdrop {
    background: rgba(15, 23, 42, 0.22);
  }
  .pal {
    display: flex;
    flex-direction: column;
    max-height: min(76vh, 620px);
  }

  .search {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 14px;
    border-bottom: 1px solid var(--border);
    flex: none;
  }
  .sicon {
    display: inline-flex;
    color: var(--text-dim);
  }
  .back {
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    border-radius: 6px;
    border: none;
    background: none;
    color: var(--text-dim);
    cursor: pointer;
    flex: none;
  }
  .back:hover {
    color: var(--text);
    background: color-mix(in srgb, var(--text) 8%, transparent);
  }
  .back :global(.rot) {
    transform: rotate(-90deg);
  }
  .crumb {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-muted);
    background: color-mix(in srgb, var(--text) 7%, transparent);
    border-radius: 6px;
    padding: 3px 8px;
    white-space: nowrap;
    max-width: 180px;
    overflow: hidden;
    text-overflow: ellipsis;
    flex: none;
  }
  .field {
    flex: 1;
    min-width: 0;
    font: inherit;
    font-size: 15px;
    color: var(--text);
    background: none;
    border: none;
    outline: none;
    padding: 15px 0;
  }
  .field::placeholder {
    color: var(--text-dim);
  }
  .esc {
    flex: none;
  }

  /* --- Liste --- */
  .list {
    overflow-y: auto;
    padding: 6px;
    flex: 1;
    min-height: 0;
    overscroll-behavior: contain;
  }
  .glabel {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.07em;
    color: var(--text-dim);
    padding: 9px 10px 4px;
  }
  .opt {
    display: flex;
    align-items: center;
    gap: 11px;
    padding: 8px 10px;
    border-radius: 8px;
    cursor: pointer;
    min-height: 40px;
    outline: none;
  }
  .opt.active {
    background: var(--pal-active);
    box-shadow: inset 2px 0 0 var(--brand-primary);
  }
  .opt.disabled {
    cursor: not-allowed;
  }
  .opt.disabled .otext,
  .opt.disabled .oicon {
    opacity: 0.5;
  }
  .opt.danger .otitle,
  .opt.danger .oicon {
    color: var(--state-danger);
  }
  .opt.danger.active {
    background: color-mix(in srgb, var(--state-danger) 13%, transparent);
    box-shadow: inset 2px 0 0 var(--state-danger);
  }
  .oicon {
    position: relative;
    display: grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border-radius: 7px;
    background: color-mix(in srgb, var(--text) 6%, transparent);
    color: var(--text-muted);
    flex: none;
  }
  .oicon img {
    width: 20px;
    height: 20px;
    object-fit: contain;
    border-radius: 4px;
  }
  .odot {
    position: absolute;
    right: -2px;
    bottom: -2px;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    box-shadow: 0 0 0 2px var(--elevated);
  }
  .otext {
    display: flex;
    align-items: baseline;
    gap: 8px;
    min-width: 0;
    flex: 1;
  }
  .otitle {
    font-size: 13.5px;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .osub {
    font-size: 12px;
    color: var(--text-dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }
  mark {
    background: none;
    color: var(--pal-hit);
    font-weight: 700;
  }
  .ohint {
    font-size: 11.5px;
    color: var(--text-dim);
    white-space: nowrap;
    flex: none;
  }
  .osec {
    font-size: 10.5px;
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 1px 7px;
    white-space: nowrap;
    flex: none;
  }
  .otab {
    flex: none;
  }
  .empty {
    padding: 26px 10px;
    text-align: center;
    color: var(--text-dim);
    font-size: 13px;
  }

  /* --- Confirmation --- */
  .confirm {
    padding: 18px 18px 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .confirm h2 {
    margin: 0;
    font-size: 15.5px;
    letter-spacing: -0.01em;
  }
  .confirm h2.danger {
    color: var(--state-danger);
  }
  .warn {
    display: flex;
    gap: 9px;
    margin: 0 0 8px;
    padding: 10px 12px;
    font-size: 13px;
    line-height: 1.45;
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--state-pending) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--state-pending) 40%, var(--border));
    color: var(--text);
  }
  .wi {
    color: var(--state-pending);
    flex: none;
    padding-top: 1px;
  }
  .detail {
    margin: 0;
    font-size: 13px;
    line-height: 1.45;
    color: var(--text-muted);
  }
  .cactions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .cactions .btn {
    padding: 8px 14px;
    font-size: 13px;
  }
  .btn-danger {
    background: var(--state-danger);
  }

  /* --- Statut + aide clavier --- */
  .status {
    font-size: 12.5px;
    color: var(--state-running);
    padding: 8px 16px;
    border-top: 1px solid var(--border);
    flex: none;
  }
  .status.error {
    color: var(--state-danger);
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }
  .keys {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px 14px;
    padding: 8px 14px;
    border-top: 1px solid var(--border);
    font-size: 11.5px;
    color: var(--text-dim);
    background: color-mix(in srgb, var(--text) 2.5%, transparent);
    flex: none;
  }
  .keys span {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .keys .spacer {
    flex: 1;
  }
</style>
