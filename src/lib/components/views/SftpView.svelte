<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import type { UnlistenFn } from '@tauri-apps/api/event';
  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
  import { api, humanizeError } from '$lib/ipc';
  import { t, getLocale } from '$lib/i18n';
  import { fmtBytes as fmtSize } from '$lib/copilot/format';
  import { tabs, type ServerTab } from '$lib/tabs/tabs.svelte';
  import Icon from '../Icon.svelte';
  import { sftpEvents } from '$lib/events';
  import { uid } from '$lib/util/id';
  import {
    startTransfer,
    transferItems,
    activeCount,
    clearFinished,
  } from '$lib/transfers/transfers.svelte';
  import type { ArchiveEntry, SftpEntry } from '$lib/types';
  import {
    isColVisible,
    toggleCol,
    setSort,
    sortKey,
    sortDir,
    sortEntries,
    OPTIONAL_COLUMNS,
    COLUMN_LABEL,
    type SftpSortKey,
  } from '$lib/sftp/columns.svelte';
  import FileEditor from './FileEditor.svelte';

  let { tab }: { tab: ServerTab } = $props();
  const serverId = $derived(tab.serverId);

  let cwd = $state('/');
  let entries = $state<SftpEntry[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let dragOver = $state(false);
  let colsMenu = $state(false);

  let editor = $state<{ path: string; name: string; content: string; readonly?: boolean } | null>(null);
  let editorSaved = false;
  let prompt = $state<{ kind: 'mkdir' | 'newfile' | 'rename'; value: string; target?: SftpEntry } | null>(null);
  let confirmDel = $state<SftpEntry | null>(null);
  let uploadConfirm = $state<{ paths: string[]; conflicts: string[]; remember: boolean } | null>(null);
  // Sélection multiple, navigateur d'archive (lecture seule), popover des transferts.
  let selected = $state<Set<string>>(new Set());
  let archive = $state<{ path: string; name: string; entries: ArchiveEntry[] } | null>(null);
  let xfersOpen = $state(false);

  let dropUnlisten: UnlistenFn | undefined;
  let progressUnlisten: UnlistenFn | undefined;

  const crumbs = $derived(buildCrumbs(cwd));
  const sorted = $derived(sortEntries(entries, sortKey(), sortDir()));
  const gridTemplate = $derived(
    [
      'minmax(0, 1fr)',
      isColVisible('size') ? '110px' : null,
      isColVisible('type') ? '120px' : null,
      isColVisible('modified') ? '170px' : null,
    ]
      .filter(Boolean)
      .join(' ')
  );

  let destroyed = false;
  onMount(async () => {
    await load('/');
    if (destroyed) return; // démonté pendant le listing → ne pas enregistrer de handler global
    dropUnlisten = await getCurrentWebview().onDragDropEvent((event) => {
      if (tabs.activeId !== tab.id) return;
      const p = event.payload;
      if (p.type === 'drop') {
        dragOver = false;
        void uploadPaths(p.paths);
      } else if (p.type === 'leave') {
        dragOver = false;
      } else {
        dragOver = true;
      }
    });
    if (destroyed) dropUnlisten?.(); // démonté pendant l'enregistrement → détacher aussitôt
    // Rafraîchit la liste quand un TÉLÉVERSEMENT se termine (le store global, lui, suit la progression).
    progressUnlisten = await sftpEvents.progress((p) => {
      if (!destroyed && p.status === 'done' && p.direction === 'up') void refreshSilently();
    });
    if (destroyed) progressUnlisten?.();
  });
  onDestroy(() => {
    destroyed = true;
    dropUnlisten?.();
    progressUnlisten?.();
  });

  function join(dir: string, name: string): string {
    return dir === '/' ? `/${name}` : `${dir}/${name}`;
  }
  function parent(path: string): string {
    if (path === '/' || !path.includes('/')) return '/';
    const p = path.replace(/\/[^/]+$/, '');
    return p === '' ? '/' : p;
  }
  function basename(p: string): string {
    return p.split(/[\\/]/).pop() ?? p;
  }
  function buildCrumbs(path: string): { label: string; path: string }[] {
    const out = [{ label: '/', path: '/' }];
    let acc = '';
    for (const seg of path.split('/').filter(Boolean)) {
      acc += `/${seg}`;
      out.push({ label: seg, path: acc });
    }
    return out;
  }

  async function load(path: string) {
    loading = true;
    error = null;
    try {
      entries = await api.sftpList(serverId, path);
      cwd = path;
    } catch (e) {
      error = humanizeError(e);
    } finally {
      loading = false;
    }
  }

  function isArchive(name: string): boolean {
    const n = name.toLowerCase();
    return n.endsWith('.zip') || n.endsWith('.tar') || n.endsWith('.tar.gz') || n.endsWith('.tgz');
  }
  function isGz(name: string): boolean {
    const n = name.toLowerCase();
    return n.endsWith('.gz') && !n.endsWith('.tar.gz');
  }

  function onRowClick(entry: SftpEntry) {
    if (entry.is_dir) void load(entry.path);
    else if (isArchive(entry.name)) void openArchive(entry);
    else if (isGz(entry.name)) void openGz(entry);
    else void openFile(entry);
  }

  async function openFile(entry: SftpEntry) {
    try {
      const content = await api.sftpReadText(serverId, entry.path);
      editor = { path: entry.path, name: entry.name, content };
    } catch (e) {
      error = humanizeError(e);
    }
  }

  // --- Archives (lecture seule) : parcours + ouverture/extraction d'une entrée ---
  async function openArchive(entry: SftpEntry) {
    try {
      const list = await api.sftpArchiveList(serverId, entry.path);
      archive = { path: entry.path, name: entry.name, entries: list };
    } catch (e) {
      error = humanizeError(e);
    }
  }
  function closeArchive() {
    archive = null;
  }
  async function openArchiveEntry(ar: { path: string; name: string }, e: ArchiveEntry) {
    if (e.is_dir) return;
    try {
      const content = await api.sftpArchiveReadText(serverId, ar.path, e.name);
      editor = { path: ar.path, name: basename(e.name), content, readonly: true };
    } catch (err) {
      error = humanizeError(err);
    }
  }
  async function extractEntry(ar: { path: string }, e: ArchiveEntry) {
    const dest = await saveDialog({ defaultPath: basename(e.name) });
    if (!dest) return;
    try {
      await api.sftpExtractEntry(serverId, ar.path, e.name, dest);
    } catch (err) {
      error = humanizeError(err);
    }
  }
  async function openGz(entry: SftpEntry) {
    try {
      const content = await api.sftpGzText(serverId, entry.path);
      editor = { path: entry.path, name: entry.name.replace(/\.gz$/i, ''), content, readonly: true };
    } catch (e) {
      error = humanizeError(e);
    }
  }

  // --- Sélection multiple ---
  function toggleSel(path: string) {
    const s = new Set(selected);
    s.has(path) ? s.delete(path) : s.add(path);
    selected = s;
  }
  function clearSel() {
    selected = new Set();
  }

  function closeEditor() {
    const changed = editorSaved;
    editorSaved = false;
    editor = null;
    // Rafraîchit taille/date uniquement si le fichier a été enregistré — sans spinner.
    if (changed) void refreshSilently();
  }

  async function refreshSilently() {
    try {
      entries = await api.sftpList(serverId, cwd);
    } catch {
      /* on garde la liste actuelle */
    }
  }

  // --- Upload (avec confirmation d'écrasement) ---
  function overwriteAlways(): boolean {
    return localStorage.getItem('mnstr-sftp-overwrite') === '1';
  }

  async function doUploadDialog() {
    const selected = await openDialog({ multiple: true, title: t('sftp.upload') });
    if (!selected) return;
    await uploadPaths(Array.isArray(selected) ? selected : [selected]);
  }

  async function uploadPaths(paths: string[]) {
    const existing = new Set(entries.map((e) => e.name));
    const conflicts = paths.map(basename).filter((n) => existing.has(n));
    if (conflicts.length > 0 && !overwriteAlways()) {
      uploadConfirm = { paths, conflicts, remember: false };
      return;
    }
    await performUpload(paths);
  }

  function confirmUpload() {
    if (!uploadConfirm) return;
    if (uploadConfirm.remember) localStorage.setItem('mnstr-sftp-overwrite', '1');
    const paths = uploadConfirm.paths;
    uploadConfirm = null;
    void performUpload(paths);
  }

  function performUpload(paths: string[]) {
    error = null;
    // Transferts en tâche de fond (suivis dans le gestionnaire) ; la liste se rafraîchit à la fin.
    for (const p of paths) {
      const id = uid();
      startTransfer(id, basename(p), 'up');
      void api.sftpUpload(serverId, p, cwd, id);
    }
  }

  // Fichier → téléchargement direct ; dossier → archive .zip (récursive côté client).
  async function doDownload(entry: SftpEntry) {
    if (entry.is_dir) {
      const dest = await saveDialog({ defaultPath: `${entry.name}.zip` });
      if (!dest) return;
      const id = uid();
      startTransfer(id, basename(dest), 'down');
      void api.sftpDownloadZip(serverId, [entry.path], dest, id);
    } else {
      const dest = await saveDialog({ defaultPath: entry.name });
      if (!dest) return;
      const id = uid();
      startTransfer(id, entry.name, 'down');
      void api.sftpDownload(serverId, entry.path, dest, id);
    }
  }

  /// Télécharge la SÉLECTION (fichiers/dossiers) en un seul .zip.
  async function downloadSelected() {
    const paths = [...selected];
    if (paths.length === 0) return;
    const dest = await saveDialog({ defaultPath: 'download.zip' });
    if (!dest) return;
    const id = uid();
    startTransfer(id, basename(dest), 'down');
    void api.sftpDownloadZip(serverId, paths, dest, id);
    clearSel();
  }

  function openPrompt(kind: 'mkdir' | 'newfile' | 'rename', target?: SftpEntry) {
    prompt = { kind, value: target?.name ?? '', target };
  }
  async function submitPrompt() {
    if (!prompt) return;
    const name = prompt.value.trim();
    if (!name) return;
    const { kind, target } = prompt;
    prompt = null;
    try {
      if (kind === 'mkdir') await api.sftpMkdir(serverId, join(cwd, name));
      else if (kind === 'newfile') await api.sftpWriteText(serverId, join(cwd, name), '');
      else if (kind === 'rename' && target) await api.sftpRename(serverId, target.path, join(cwd, name));
      await load(cwd);
    } catch (e) {
      error = humanizeError(e);
    }
  }

  async function doDelete() {
    if (!confirmDel) return;
    const entry = confirmDel;
    confirmDel = null;
    try {
      await api.sftpDelete(serverId, entry.path, entry.is_dir);
      await load(cwd);
    } catch (e) {
      error = humanizeError(e);
    }
  }

  // --- Rendu ---
  function ind(key: SftpSortKey): string {
    return sortKey() === key ? (sortDir() === 'asc' ? '▲' : '▼') : '';
  }
  function typeLabel(e: SftpEntry): string {
    if (e.is_dir) return t('sftp.folder');
    const i = e.name.lastIndexOf('.');
    if (i > 0 && i < e.name.length - 1) return e.name.slice(i + 1).toUpperCase();
    return t('sftp.file');
  }
  function fmtDate(m: number | null): string {
    if (!m) return '';
    try {
      return new Date(m * 1000).toLocaleString(getLocale() === 'fr' ? 'fr-FR' : 'en-US', {
        dateStyle: 'short',
        timeStyle: 'short',
      });
    } catch {
      return '';
    }
  }
</script>

<div class="sftp" class:drag={dragOver}>
  <div class="toolbar">
    <button class="tb ico-btn" title={t('sftp.up')} disabled={cwd === '/'} onclick={() => load(parent(cwd))}><Icon name="arrow-up" size={16} /></button>
    <button class="tb ico-btn" title={t('common.refresh')} onclick={() => load(cwd)}><Icon name="refresh-cw" size={15} /></button>
    <nav class="crumbs">
      {#each crumbs as c, i (c.path)}
        {#if i > 0}<span class="sl">/</span>{/if}
        <button class="crumb" onclick={() => load(c.path)}>{c.label}</button>
      {/each}
    </nav>
    <div class="grow"></div>
    <div class="xfers">
      <button
        class="tb ghost ico-btn"
        class:busy={activeCount() > 0}
        title={t('sftp.transfers')}
        aria-label={t('sftp.transfers')}
        onclick={() => (xfersOpen = !xfersOpen)}
      >
        {#if activeCount() > 0}<span class="spinner"></span>{:else}<Icon name="download" size={15} />{/if}
        {#if transferItems().length > 0}<span class="xbadge">{transferItems().length}</span>{/if}
      </button>
      {#if xfersOpen}
        <button class="backdrop" onclick={() => (xfersOpen = false)} aria-label={t('common.close')}></button>
        <div class="xmenu">
          <div class="xhead">
            <span>{t('sftp.transfers')}</span>
            {#if transferItems().length > 0}
              <button class="xclear" onclick={clearFinished}>{t('sftp.clearDone')}</button>
            {/if}
          </div>
          {#if transferItems().length === 0}
            <div class="xempty dim">{t('sftp.noTransfers')}</div>
          {:else}
            {#each transferItems() as x (x.id)}
              <div class="xrow">
                <span class="xdir"><Icon name={x.direction === 'up' ? 'upload' : 'download'} size={14} /></span>
                <div class="xinfo">
                  <div class="xname" title={x.name}>{x.name}</div>
                  <div class="xbar">
                    <div
                      class="xfill"
                      class:err={x.status === 'error'}
                      class:done={x.status === 'done'}
                      style="width: {x.total > 0
                        ? Math.round((x.done / x.total) * 100)
                        : x.status === 'active'
                          ? 8
                          : 100}%"
                    ></div>
                  </div>
                </div>
                <span class="xstate" class:err={x.status === 'error'}>
                  {x.status === 'error'
                    ? '⚠'
                    : x.status === 'done'
                      ? '✓'
                      : x.total > 0
                        ? Math.round((x.done / x.total) * 100) + '%'
                        : '…'}
                </span>
              </div>
            {/each}
          {/if}
        </div>
      {/if}
    </div>
    <button class="tb ghost ico-btn" onclick={() => openPrompt('mkdir')}><Icon name="plus" size={14} /> {t('sftp.newFolder')}</button>
    <button class="tb ghost ico-btn" onclick={() => openPrompt('newfile')}><Icon name="plus" size={14} /> {t('sftp.newFile')}</button>
    <button class="tb primary ico-btn" onclick={doUploadDialog}><Icon name="upload" size={15} /> {t('sftp.upload')}</button>
  </div>

  {#if error}<div class="bar err">{error}</div>{/if}

  {#if selected.size > 0}
    <div class="selbar">
      <span class="selcount">{t('sftp.selectedCount', { n: selected.size })}</span>
      <button class="btn small" onclick={downloadSelected}>
        <Icon name="download" size={14} /> {t('sftp.downloadZip')}
      </button>
      <button class="btn btn--ghost small" onclick={clearSel}>{t('sftp.clearSel')}</button>
    </div>
  {/if}

  <div class="thead" style="grid-template-columns: {gridTemplate}">
    <button class="th" onclick={() => setSort('name')}>{t('sftp.colName')}<span class="ind">{ind('name')}</span></button>
    {#if isColVisible('size')}
      <button class="th num" onclick={() => setSort('size')}>{t('sftp.colSize')}<span class="ind">{ind('size')}</span></button>
    {/if}
    {#if isColVisible('type')}
      <button class="th" onclick={() => setSort('type')}>{t('sftp.colType')}<span class="ind">{ind('type')}</span></button>
    {/if}
    {#if isColVisible('modified')}
      <button class="th" onclick={() => setSort('modified')}>{t('sftp.colModified')}<span class="ind">{ind('modified')}</span></button>
    {/if}
    <button class="cols-toggle" title={t('sftp.columns')} onclick={() => (colsMenu = !colsMenu)}>⋮</button>
    {#if colsMenu}
      <button class="backdrop" onclick={() => (colsMenu = false)} aria-label={t('common.close')}></button>
      <div class="colsmenu">
        {#each OPTIONAL_COLUMNS as c (c)}
          <label class="colitem">
            <input type="checkbox" checked={isColVisible(c)} onchange={() => toggleCol(c)} />
            {t(COLUMN_LABEL[c])}
          </label>
        {/each}
      </div>
    {/if}
  </div>

  <div class="list">
    {#if loading}
      <div class="center"><span class="spinner"></span></div>
    {:else if sorted.length === 0}
      <div class="center dim">{t('sftp.empty')}</div>
    {:else}
      {#each sorted as entry (entry.path)}
        <div class="row" class:sel={selected.has(entry.path)}>
          <button
            class="rowcheck"
            title={t('sftp.select')}
            aria-label={t('sftp.select')}
            onclick={() => toggleSel(entry.path)}
          >
            <Icon name={selected.has(entry.path) ? 'check-square' : 'square'} size={16} />
          </button>
          <button class="rowmain" style="grid-template-columns: {gridTemplate}" onclick={() => onRowClick(entry)}>
            <span class="cell name">
              <span class="ico" class:dir={entry.is_dir}>
                <Icon
                  name={entry.is_dir
                    ? 'folder'
                    : isArchive(entry.name) || isGz(entry.name)
                      ? 'archive'
                      : 'file'}
                  size={16}
                />
              </span>
              <span class="nm">{entry.name}</span>
            </span>
            {#if isColVisible('size')}<span class="cell num dim">{entry.is_dir ? '' : fmtSize(entry.size)}</span>{/if}
            {#if isColVisible('type')}<span class="cell dim">{typeLabel(entry)}</span>{/if}
            {#if isColVisible('modified')}<span class="cell dim">{fmtDate(entry.modified)}</span>{/if}
          </button>
          <div class="rowacts">
            <button class="ra ico-btn" title={t('sftp.download')} onclick={() => doDownload(entry)}><Icon name="download" size={15} /></button>
            <button class="ra ico-btn" title={t('sftp.rename')} onclick={() => openPrompt('rename', entry)}><Icon name="pencil" size={15} /></button>
            <button class="ra danger ico-btn" title={t('sftp.delete')} onclick={() => (confirmDel = entry)}><Icon name="trash" size={15} /></button>
          </div>
        </div>
      {/each}
    {/if}
  </div>

  {#if dragOver}<div class="dropzone">{t('sftp.dropHere')}</div>{/if}
</div>

{#if editor}
  {@const ed = editor}
  <div class="overlay top">
    <FileEditor
      serverId={serverId}
      path={ed.path}
      name={ed.name}
      content={ed.content}
      readonly={ed.readonly ?? false}
      onClose={closeEditor}
      onSaved={() => (editorSaved = true)}
    />
  </div>
{/if}

{#if archive}
  {@const ar = archive}
  <div class="overlay">
    <div class="archview">
      <div class="abar">
        <span class="fname"><Icon name="archive" size={15} /> {ar.name}</span>
        <span class="robadge">{t('sftp.readonly')}</span>
        <div class="grow"></div>
        <button class="close" onclick={closeArchive} title={t('common.close')}><Icon name="x" size={16} /></button>
      </div>
      <div class="alist">
        {#if ar.entries.length === 0}
          <div class="center dim">{t('sftp.empty')}</div>
        {:else}
          {#each ar.entries as e (e.name)}
            <div class="row">
              <button class="rowmain arow" disabled={e.is_dir} onclick={() => openArchiveEntry(ar, e)}>
                <span class="cell name">
                  <span class="ico" class:dir={e.is_dir}><Icon name={e.is_dir ? 'folder' : 'file'} size={15} /></span>
                  <span class="nm">{e.name}</span>
                </span>
                <span class="cell num dim">{e.is_dir ? '' : fmtSize(e.size)}</span>
              </button>
              {#if !e.is_dir}
                <div class="rowacts">
                  <button class="ra ico-btn" title={t('sftp.download')} onclick={() => extractEntry(ar, e)}>
                    <Icon name="download" size={15} />
                  </button>
                </div>
              {/if}
            </div>
          {/each}
        {/if}
      </div>
    </div>
  </div>
{/if}

{#if prompt}
  {@const pr = prompt}
  <div class="overlay center">
    <form class="modal card" onsubmit={(e) => { e.preventDefault(); submitPrompt(); }}>
      <h3>{pr.kind === 'mkdir' ? t('sftp.newFolder') : pr.kind === 'newfile' ? t('sftp.newFile') : t('sftp.rename')}</h3>
      <!-- svelte-ignore a11y_autofocus -->
      <input class="input" bind:value={pr.value} placeholder={t('sftp.namePlaceholder')} autofocus />
      <div class="modal-actions">
        <button type="button" class="btn btn--ghost" onclick={() => (prompt = null)}>{t('common.cancel')}</button>
        <button type="submit" class="btn">{pr.kind === 'rename' ? t('sftp.rename') : t('sftp.create')}</button>
      </div>
    </form>
  </div>
{/if}

{#if confirmDel}
  {@const cd = confirmDel}
  <div class="overlay center">
    <div class="modal card">
      <h3>{t('sftp.confirmDelete', { name: cd.name })}</h3>
      <div class="modal-actions">
        <button class="btn btn--ghost" onclick={() => (confirmDel = null)}>{t('common.cancel')}</button>
        <button class="btn danger-btn" onclick={doDelete}>{t('sftp.delete')}</button>
      </div>
    </div>
  </div>
{/if}

{#if uploadConfirm}
  {@const uc = uploadConfirm}
  <div class="overlay center">
    <div class="modal card">
      <h3>{t('sftp.overwriteTitle')}</h3>
      <p class="ow-body">{t('sftp.overwriteBody', { names: uc.conflicts.join(', ') })}</p>
      <label class="ow-remember">
        <input type="checkbox" bind:checked={uc.remember} />
        {t('sftp.dontAskAgain')}
      </label>
      <div class="modal-actions">
        <button class="btn btn--ghost" onclick={() => (uploadConfirm = null)}>{t('common.cancel')}</button>
        <button class="btn" onclick={confirmUpload}>{t('sftp.overwrite')}</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .sftp {
    height: 100%;
    display: flex;
    flex-direction: column;
    position: relative;
  }
  .toolbar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 10px 14px;
    border-bottom: 1px solid var(--border);
    flex: none;
  }
  .tb {
    background: none;
    border: 1px solid transparent;
    border-radius: 8px;
    cursor: pointer;
    font: inherit;
    color: var(--text-muted);
    padding: 7px 10px;
  }
  /* Boutons contenant une icône (seule ou avec libellé) : centrage + espacement. */
  .ico-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .tb:hover:not(:disabled) {
    color: var(--text);
    background: color-mix(in srgb, var(--text) 6%, transparent);
  }
  .tb:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
  .tb.ghost {
    border-color: var(--border);
    font-size: 12.5px;
  }
  .tb.primary {
    background: var(--brand-primary);
    color: #fff;
    font-size: 12.5px;
    font-weight: 600;
  }
  .crumbs {
    display: flex;
    align-items: center;
    gap: 2px;
    overflow-x: auto;
    white-space: nowrap;
    scrollbar-width: none;
    margin-left: 4px;
  }
  .crumb {
    background: none;
    border: none;
    cursor: pointer;
    font: inherit;
    font-family: var(--font-mono);
    font-size: 12.5px;
    color: var(--text-muted);
    padding: 3px 5px;
    border-radius: 5px;
  }
  .crumb:hover {
    color: var(--brand-primary);
  }
  .sl {
    color: var(--text-dim);
  }
  .grow {
    flex: 1;
  }
  .bar.err {
    padding: 8px 14px;
    background: color-mix(in srgb, var(--state-danger) 12%, transparent);
    color: color-mix(in srgb, var(--state-danger) 70%, var(--text));
    font-size: 13px;
  }

  .thead {
    display: grid;
    align-items: center;
    position: relative;
    padding: 0 90px 0 14px;
    border-bottom: 1px solid var(--border);
    background: var(--bg);
    flex: none;
  }
  .th {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    background: none;
    border: none;
    cursor: pointer;
    font: inherit;
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--text-dim);
    padding: 9px 4px;
    text-align: left;
  }
  .th:hover {
    color: var(--text);
  }
  .th.num {
    justify-content: flex-start;
  }
  .ind {
    font-size: 9px;
    color: var(--brand-primary);
  }
  .cols-toggle {
    position: absolute;
    right: 12px;
    top: 50%;
    transform: translateY(-50%);
    background: none;
    border: none;
    cursor: pointer;
    color: var(--text-dim);
    font-size: 16px;
    padding: 4px 8px;
    border-radius: 6px;
  }
  .cols-toggle:hover {
    color: var(--text);
    background: color-mix(in srgb, var(--text) 8%, transparent);
  }
  .colsmenu {
    position: absolute;
    right: 8px;
    top: 100%;
    z-index: 15;
    background: var(--elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: var(--shadow);
    padding: 6px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .colitem {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    padding: 6px 10px;
    border-radius: 6px;
    cursor: pointer;
    white-space: nowrap;
  }
  .colitem:hover {
    background: color-mix(in srgb, var(--text) 6%, transparent);
  }

  .list {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }
  .center {
    display: grid;
    place-items: center;
    height: 100%;
    padding: 40px;
  }
  .row {
    position: relative;
    display: flex;
    align-items: center;
    border-bottom: 1px solid color-mix(in srgb, var(--border) 55%, transparent);
  }
  .row:hover {
    background: color-mix(in srgb, var(--text) 4%, transparent);
  }
  .row.sel {
    background: color-mix(in srgb, var(--brand-primary) 12%, transparent);
  }
  .rowcheck {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
    width: 34px;
    align-self: stretch;
    background: none;
    border: none;
    cursor: pointer;
    color: var(--text-dim);
    opacity: 0;
  }
  .row:hover .rowcheck,
  .row.sel .rowcheck {
    opacity: 1;
  }
  .row.sel .rowcheck {
    color: var(--brand-primary);
  }
  .rowmain {
    display: grid;
    align-items: center;
    flex: 1;
    min-width: 0;
    background: none;
    border: none;
    cursor: pointer;
    font: inherit;
    color: var(--text);
    text-align: left;
    padding: 0 90px 0 14px;
  }
  .cell {
    padding: 9px 4px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13.5px;
  }
  .cell.name {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
  }
  .ico {
    display: inline-flex;
    align-items: center;
    color: var(--text-dim);
    flex: none;
  }
  .ico.dir {
    color: var(--brand-primary);
  }
  .nm {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cell.num {
    font-family: var(--font-mono);
    font-size: 12px;
  }
  .rowacts {
    position: absolute;
    right: 8px;
    top: 0;
    bottom: 0;
    display: flex;
    align-items: center;
    gap: 2px;
    opacity: 0;
  }
  .row:hover .rowacts {
    opacity: 1;
  }
  .ra {
    background: none;
    border: none;
    cursor: pointer;
    color: var(--text-dim);
    padding: 6px 8px;
    border-radius: 6px;
    font-size: 13px;
  }
  .ra:hover {
    color: var(--text);
    background: color-mix(in srgb, var(--text) 8%, transparent);
  }
  .ra.danger:hover {
    color: var(--state-danger);
  }

  .dropzone {
    position: absolute;
    inset: 8px;
    border: 2px dashed var(--brand-primary);
    border-radius: var(--radius-lg);
    background: color-mix(in srgb, var(--brand-primary) 10%, transparent);
    display: grid;
    place-items: center;
    font-weight: 600;
    color: var(--brand-primary);
    pointer-events: none;
    z-index: 5;
  }

  .overlay {
    position: absolute;
    inset: 0;
    background: color-mix(in srgb, #000 45%, transparent);
    z-index: 10;
    padding: 20px;
  }
  .overlay.center {
    display: grid;
    place-items: center;
  }
  /* L'éditeur (ouvert depuis une entrée d'archive) doit passer AU-DESSUS de l'overlay d'archive. */
  .overlay.top {
    z-index: 12;
  }
  .backdrop {
    position: fixed;
    inset: 0;
    background: none;
    border: none;
    z-index: 14;
    cursor: default;
  }
  .modal {
    padding: 22px;
    width: 100%;
    max-width: 400px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .modal h3 {
    margin: 0;
    font-size: 16px;
  }
  .ow-body {
    margin: 0;
    font-size: 13px;
    color: var(--text-muted);
    word-break: break-word;
  }
  .ow-remember {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    color: var(--text-muted);
  }
  .modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .danger-btn {
    background: var(--state-danger);
  }

  /* --- Sélection multiple --- */
  .selbar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 14px;
    background: color-mix(in srgb, var(--brand-primary) 8%, var(--surface));
    border-bottom: 1px solid var(--border);
    font-size: 13px;
  }
  .selcount {
    font-weight: 600;
    margin-right: auto;
  }
  .btn.small {
    padding: 5px 11px;
    font-size: 12.5px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }

  /* --- Gestionnaire de transferts --- */
  .xfers {
    position: relative;
    display: inline-flex;
  }
  .tb.busy {
    color: var(--brand-primary);
  }
  .xbadge {
    margin-left: 5px;
    font-size: 11px;
    font-family: var(--font-mono);
    background: var(--brand-primary);
    color: #fff;
    border-radius: 999px;
    padding: 0 6px;
    line-height: 16px;
  }
  .xmenu {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 21;
    width: 320px;
    max-height: 380px;
    overflow-y: auto;
    background: var(--elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: var(--shadow);
    padding: 6px;
  }
  .xhead {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 4px 8px 8px;
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--text-muted);
    font-family: var(--font-mono);
  }
  .xclear {
    background: none;
    border: none;
    color: var(--text-dim);
    cursor: pointer;
    font: inherit;
    font-size: 11px;
    text-transform: none;
    letter-spacing: 0;
  }
  .xclear:hover {
    color: var(--text);
  }
  .xempty {
    padding: 14px;
    text-align: center;
    font-size: 12.5px;
  }
  .xrow {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 7px 8px;
  }
  .xdir {
    color: var(--text-dim);
    flex: none;
    display: inline-flex;
  }
  .xinfo {
    flex: 1;
    min-width: 0;
  }
  .xname {
    font-size: 12.5px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .xbar {
    height: 4px;
    margin-top: 5px;
    background: color-mix(in srgb, var(--text) 12%, transparent);
    border-radius: 3px;
    overflow: hidden;
  }
  .xfill {
    height: 100%;
    background: var(--brand-primary);
    transition: width 0.15s ease;
  }
  .xfill.done {
    background: var(--state-running);
  }
  .xfill.err {
    background: var(--state-danger);
  }
  .xstate {
    flex: none;
    font-size: 11px;
    font-family: var(--font-mono);
    color: var(--text-dim);
    min-width: 30px;
    text-align: right;
  }
  .xstate.err {
    color: var(--state-danger);
  }

  /* --- Navigateur d'archive (lecture seule) --- */
  .archview {
    position: absolute;
    inset: 20px;
    display: flex;
    flex-direction: column;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
  }
  .abar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 9px 12px;
    border-bottom: 1px solid var(--border);
    background: var(--elevated);
  }
  .abar .fname {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    font-weight: 600;
    font-size: 13.5px;
  }
  .abar .close {
    background: none;
    border: none;
    color: var(--text-dim);
    cursor: pointer;
    padding: 4px;
    border-radius: 6px;
  }
  .abar .close:hover {
    color: var(--text);
    background: color-mix(in srgb, var(--text) 8%, transparent);
  }
  .robadge {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-muted);
    background: color-mix(in srgb, var(--text) 10%, transparent);
    border-radius: 6px;
    padding: 3px 9px;
  }
  .alist {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }
  .arow {
    grid-template-columns: minmax(0, 1fr) 110px;
  }
  .arow:disabled {
    cursor: default;
  }
</style>
