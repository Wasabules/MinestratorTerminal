<script lang="ts">
  import { api, humanizeError } from "../../ipc";
  import { activeCount, uploadTo, downloadFrom, transferItems } from "../../stores/transfers.svelte";
  import TransferCenter from "../TransferCenter.svelte";
  import { t } from "../../i18n";
  import { portal } from "../../portal";
  import Icon from "../Icon.svelte";
  import CodeEditor from "../CodeEditor.svelte";
  import type { ArchiveEntry, SftpEntry } from "../../types";

  let { serverId, active }: { serverId: number; active: boolean } = $props();

  /** Un transfert est en cours : évite d'en lancer deux à la fois depuis cette vue. */
  const busy = $derived(activeCount() > 0);

  /** Envoi : le store gère sélecteur, nom réel du fichier, lecture en flux et appels IPC. */
  async function uploadFile() {
    const ok = await uploadTo(serverId, cwd, t("files.upload"));
    if (ok) {
      await load();
    } else {
      // Échec : l'erreur détaillée vit dans le centre de transferts, qu'on ouvre d'emblée
      // plutôt que de laisser l'utilisateur deviner ce qui s'est passé.
      const last = transferItems()[0];
      if (last?.status === "error") error = last.error;
    }
  }

  /** Téléchargement d'une entrée distante vers l'emplacement choisi. */
  async function downloadEntry(entry: SftpEntry) {
    actionFor = null;
    await downloadFrom(serverId, entry.path, entry.name, t("files.download"));
  }

  let loaded = $state(false);

  let cwd = $state("/");
  let entries = $state<SftpEntry[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);

  // Éditeur de fichier (overlay).
  let opened = $state<{
    path: string;
    name: string;
    content: string;
    dirty: boolean;
    saving: boolean;
    readonly: boolean;
  } | null>(null);
  let editor: CodeEditor | undefined = $state();
  /** Aperçu d'image (data URI fourni par le core, aucune écriture sur le téléphone). */
  let preview = $state<{ name: string; src: string } | null>(null);
  /** Archive ouverte en lecture : liste des entrées, plus l'entrée texte affichée. */
  let archive = $state<{ path: string; name: string; entries: ArchiveEntry[] } | null>(null);
  /** Recherche récursive sous le dossier courant, dépliée à la demande depuis la loupe. */
  let searchOpen = $state(false);
  let query = $state("");
  let results = $state<{ entries: SftpEntry[]; truncated: boolean } | null>(null);
  let searching = $state(false);

  const IMAGE_EXT = /\.(png|jpe?g|gif|webp|bmp|ico|svg)$/i;
  const ARCHIVE_EXT = /\.(zip|tar|tar\.gz|tgz|jar)$/i;

  /** Recherche par nom sous le dossier courant ; vide = on rend la main à la navigation. */
  async function runSearch() {
    const q = query.trim();
    if (q.length < 2) {
      results = null;
      return;
    }
    searching = true;
    error = null;
    try {
      results = await api.sftpSearch(serverId, cwd, q);
    } catch (err) {
      error = humanizeError(err);
      results = null;
    } finally {
      searching = false;
    }
  }

  function clearSearch() {
    query = "";
    results = null;
  }

  /** Ouvre/ferme le champ ; la fermeture rend la main à la navigation du dossier. */
  function toggleSearch() {
    searchOpen = !searchOpen;
    if (!searchOpen) clearSearch();
  }

  /** Focus + clavier dès l'apparition du champ : sinon il faut un second appui pour saisir. */
  function autofocus(node: HTMLInputElement) {
    node.focus();
  }

  // Feuille d'actions pour une entrée.
  let actionFor = $state<SftpEntry | null>(null);
  // Modale de saisie / confirmation.
  let modal = $state<{ kind: "mkdir" | "rename" | "delete"; value: string; target?: SftpEntry } | null>(null);

  function parent(p: string): string {
    if (p === "/" || p === "") return "/";
    const trimmed = p.replace(/\/+$/, "");
    const idx = trimmed.lastIndexOf("/");
    return idx <= 0 ? "/" : trimmed.slice(0, idx);
  }
  function join(dir: string, name: string): string {
    return dir === "/" ? `/${name}` : `${dir}/${name}`;
  }
  function fmtSize(n: number): string {
    if (n >= 1024 ** 2) return `${(n / 1024 ** 2).toFixed(1)} Mo`;
    if (n >= 1024) return `${Math.round(n / 1024)} Ko`;
    return `${n} o`;
  }

  async function load() {
    loading = true;
    error = null;
    try {
      const list = await api.sftpList(serverId, cwd);
      list.sort((a, b) =>
        a.is_dir === b.is_dir ? a.name.localeCompare(b.name) : a.is_dir ? -1 : 1,
      );
      entries = list;
    } catch (err) {
      error = humanizeError(err);
    } finally {
      loading = false;
    }
  }

  function cd(path: string) {
    cwd = path;
    load();
  }

  async function openEntry(entry: SftpEntry) {
    if (entry.is_dir) {
      cd(entry.path);
      return;
    }
    // Image : aperçu direct plutôt qu'un « fichier binaire » sans recours.
    if (IMAGE_EXT.test(entry.name)) {
      try {
        preview = { name: entry.name, src: await api.sftpReadDataUri(serverId, entry.path) };
      } catch (err) {
        error = humanizeError(err);
      }
      return;
    }
    // Archive : on liste son contenu sans rien extraire ni télécharger.
    if (ARCHIVE_EXT.test(entry.name)) {
      try {
        archive = {
          path: entry.path,
          name: entry.name,
          entries: await api.sftpArchiveList(serverId, entry.path),
        };
      } catch (err) {
        error = humanizeError(err);
      }
      return;
    }
    const isGz = entry.name.toLowerCase().endsWith(".gz");
    try {
      const content = isGz
        ? await api.sftpGzText(serverId, entry.path)
        : await api.sftpReadText(serverId, entry.path);
      opened = { path: entry.path, name: entry.name, content, dirty: false, saving: false, readonly: isGz };
    } catch {
      error = t("files.binary");
    }
  }

  /** Ouvre une entrée d'archive dans l'éditeur, en lecture seule (rien n'est réécrit). */
  async function openArchiveEntry(entryPath: string) {
    if (!archive) return;
    try {
      const content = await api.sftpArchiveReadText(serverId, archive.path, entryPath);
      opened = {
        path: `${archive.path}!${entryPath}`,
        name: entryPath.split("/").pop() ?? entryPath,
        content,
        dirty: false,
        saving: false,
        readonly: true,
      };
    } catch {
      error = t("files.binary");
    }
  }

  async function save() {
    if (!opened) return;
    opened.saving = true;
    try {
      await api.sftpWriteText(serverId, opened.path, opened.content);
      opened.dirty = false;
      notice = t("files.saved");
      setTimeout(() => (notice = null), 1500);
    } catch (err) {
      error = humanizeError(err);
    } finally {
      opened.saving = false;
    }
  }

  async function confirmModal() {
    if (!modal) return;
    const m = modal;
    modal = null;
    try {
      if (m.kind === "mkdir" && m.value.trim()) {
        await api.sftpMkdir(serverId, join(cwd, m.value.trim()));
      } else if (m.kind === "rename" && m.target && m.value.trim()) {
        await api.sftpRename(serverId, m.target.path, join(parent(m.target.path), m.value.trim()));
      } else if (m.kind === "delete" && m.target) {
        await api.sftpDelete(serverId, m.target.path, m.target.is_dir);
      }
      await load();
    } catch (err) {
      error = humanizeError(err);
    }
  }

  $effect(() => {
    if (active && !loaded) {
      loaded = true;
      load();
    }
  });
</script>

<div class="sftp">
  <!-- Barre de chemin -->
  <div class="pathbar">
    <button class="ic" disabled={cwd === "/"} onclick={() => cd(parent(cwd))} aria-label="Remonter">
      <Icon name="up" size={18} />
    </button>
    <span class="path selectable">{cwd}</span>
    <button class="ic" onclick={() => (modal = { kind: "mkdir", value: "" })} aria-label={t("files.newFolder")}>
      <Icon name="folderPlus" size={18} />
    </button>
    <button class="ic" onclick={uploadFile} disabled={busy} aria-label={t("files.upload")}>
      <Icon name="up" size={18} />
    </button>
    <TransferCenter />
    <button
      class="ic"
      class:on={searchOpen}
      onclick={toggleSearch}
      aria-label={t("files.search")}
      aria-expanded={searchOpen}
    >
      <Icon name="search" size={18} />
    </button>
    <button class="ic" onclick={load} aria-label={t("servers.refresh")}>
      <Icon name="refresh" size={18} />
    </button>
  </div>

  <!-- Recherche dépliée depuis la loupe : sur mobile, un champ permanent occupe une ligne
       entière pour une action ponctuelle. -->
  {#if searchOpen}
    <div class="searchbar">
      <Icon name="search" size={16} />
      <input
        type="search"
        bind:value={query}
        use:autofocus
        placeholder={t("files.search")}
        onkeydown={(e) => e.key === "Enter" && runSearch()}
      />
      {#if results}
        <button class="ic" onclick={clearSearch} aria-label={t("files.searchClear")}>
          <Icon name="close" size={16} />
        </button>
      {:else}
        <button class="ic" onclick={runSearch} disabled={searching || query.trim().length < 2}>
          <Icon name="send" size={16} />
        </button>
      {/if}
    </div>
  {/if}

  {#if notice}<p class="ok">{notice}</p>{/if}
  {#if error}<p class="err selectable">{error}</p>{/if}

  {#if searching}
    <p class="dim pad">…</p>
  {:else if results}
    {#if results.entries.length === 0}
      <p class="dim pad">{t("files.searchNone")}</p>
    {:else}
      {#if results.truncated}<p class="dim pad">{t("files.searchTruncated")}</p>{/if}
      <ul>
        {#each results.entries as e (e.path)}
          <li>
            <button class="row" onclick={() => openEntry(e)}>
              <Icon name={e.is_dir ? "folder" : "file"} size={20} />
              <span class="meta">
                <span class="name">{e.name}</span>
                <!-- Le chemin compte plus que la taille ici : il situe le résultat. -->
                <small class="dim selectable">{e.path}</small>
              </span>
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  {:else if loading}
    <p class="dim pad">…</p>
  {:else if entries.length === 0}
    <p class="dim pad">{t("files.empty")}</p>
  {:else}
    <ul>
      {#each entries as e (e.path)}
        <li>
          <button class="row" onclick={() => openEntry(e)}>
            <Icon name={e.is_dir ? "folder" : "file"} size={20} />
            <span class="meta">
              <span class="name">{e.name}</span>
              {#if !e.is_dir}<small class="dim">{fmtSize(e.size)}</small>{/if}
            </span>
            {#if e.is_dir}<Icon name="chevronRight" size={18} />{/if}
          </button>
          <button class="kebab" onclick={() => (actionFor = e)} aria-label="Actions">⋯</button>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<!-- Aperçu d'image : le core renvoie un data URI, rien n'est écrit sur le téléphone. -->
{#if preview}
  <div class="scrim" use:portal role="button" tabindex="-1" onclick={() => (preview = null)} onkeydown={() => {}}>
    <div class="imgbox" role="dialog" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
      <header class="obar">
        <span class="oname">{preview.name}</span>
        <button class="ic" onclick={() => (preview = null)} aria-label={t("files.close")}>
          <Icon name="close" size={18} />
        </button>
      </header>
      <img src={preview.src} alt={preview.name} />
    </div>
  </div>
{/if}

<!-- Contenu d'archive, en lecture seule. -->
{#if archive}
  <div class="scrim" use:portal role="button" tabindex="-1" onclick={() => (archive = null)} onkeydown={() => {}}>
    <div class="sheet tall" role="dialog" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={() => {}}>
      <header class="obar">
        <span class="oname">{archive.name}</span>
        <button class="ic" onclick={() => (archive = null)} aria-label={t("files.close")}>
          <Icon name="close" size={18} />
        </button>
      </header>
      {#if archive.entries.length === 0}
        <p class="dim pad">{t("files.empty")}</p>
      {:else}
        <ul class="arc">
          {#each archive.entries.slice(0, 300) as a (a.name)}
            <li>
              <button class="row" onclick={() => openArchiveEntry(a.name)} disabled={a.is_dir}>
                <Icon name={a.is_dir ? "folder" : "file"} size={18} />
                <span class="meta">
                  <span class="name">{a.name}</span>
                  {#if !a.is_dir}<small class="dim">{fmtSize(a.size)}</small>{/if}
                </span>
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  </div>
{/if}

<!-- Feuille d'actions par entrée -->
{#if actionFor}
  <div
    class="scrim"
    use:portal
    role="button"
    tabindex="-1"
    onclick={() => (actionFor = null)}
    onkeydown={(e) => e.key === "Escape" && (actionFor = null)}
  ></div>
  <div class="sheet" use:portal>
    <div class="sheet-title selectable">{actionFor.name}</div>
    {#if !actionFor.is_dir}
      <!-- Dossiers exclus : le téléchargement récursif (zip) est une capacité desktop. -->
      <button onclick={() => downloadEntry(actionFor!)} disabled={busy}>
        <Icon name="save" size={18} /> {t("files.download")}
      </button>
    {/if}
    <button
      onclick={() => {
        modal = { kind: "rename", value: actionFor!.name, target: actionFor! };
        actionFor = null;
      }}
    >
      <Icon name="edit" size={18} /> {t("files.rename")}
    </button>
    <button
      class="danger"
      onclick={() => {
        modal = { kind: "delete", value: "", target: actionFor! };
        actionFor = null;
      }}
    >
      <Icon name="trash" size={18} /> {t("files.delete")}
    </button>
  </div>
{/if}

<!-- Modale saisie / confirmation -->
{#if modal}
  <div class="scrim" use:portal role="button" tabindex="-1" onclick={() => (modal = null)} onkeydown={() => {}}></div>
  <div class="dialog" use:portal>
    {#if modal.kind === "delete"}
      <p class="dlg-title">{t("files.confirmDelete")} « {modal.target?.name} » ?</p>
      <div class="dlg-actions">
        <button onclick={() => (modal = null)}>{t("files.cancel")}</button>
        <button class="primary danger" onclick={confirmModal}>{t("files.delete")}</button>
      </div>
    {:else}
      <p class="dlg-title">{modal.kind === "mkdir" ? t("files.newFolder") : t("files.rename")}</p>
      <input
        class="selectable"
        autocapitalize="off"
        autocomplete="off"
        spellcheck="false"
        placeholder={modal.kind === "mkdir" ? t("files.folderName") : t("files.newName")}
        bind:value={modal.value}
      />
      <div class="dlg-actions">
        <button onclick={() => (modal = null)}>{t("files.cancel")}</button>
        <button class="primary" onclick={confirmModal}>
          {modal.kind === "mkdir" ? t("files.create") : t("files.rename")}
        </button>
      </div>
    {/if}
  </div>
{/if}

<!-- Éditeur de fichier -->
{#if opened}
  <div class="editor" use:portal>
    <header>
      <button class="ic" onclick={() => (opened = null)} aria-label="Fermer"><Icon name="back" size={20} /></button>
      <span class="ename selectable">{opened.name}{opened.dirty ? " •" : ""}</span>
      {#if opened.readonly}<span class="ro">lecture seule</span>{/if}
      <button class="ic" onclick={() => editor?.openSearch()} aria-label="Rechercher">
        <Icon name="search" size={18} />
      </button>
      {#if opened.dirty && !opened.readonly}
        <button class="ic save" disabled={opened.saving} onclick={save} aria-label={t("files.save")}>
          <Icon name="save" size={18} />
        </button>
      {/if}
    </header>
    <div class="edwrap">
      <CodeEditor
        bind:this={editor}
        value={opened.content}
        filename={opened.name}
        readOnly={opened.readonly}
        onChange={(v) => {
          if (opened) {
            opened.content = v;
            opened.dirty = true;
          }
        }}
      />
    </div>
  </div>
{/if}

<style>
  .ic.on {
    color: var(--brand-primary);
  }
  .searchbar {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0 12px 8px;
    padding: 6px 10px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--surface);
    color: var(--text-dim);
  }
  .searchbar input {
    flex: 1;
    min-width: 0;
    border: 0;
    background: none;
    color: var(--text);
    font: inherit;
    font-size: 15px;
    outline: none;
  }
  .searchbar .ic:disabled {
    opacity: 0.4;
  }
  .imgbox {
    width: min(92vw, 560px);
    max-height: 86vh;
    display: flex;
    flex-direction: column;
    border-radius: 14px;
    background: var(--surface);
    overflow: hidden;
  }
  .imgbox img {
    max-width: 100%;
    max-height: 74vh;
    object-fit: contain;
    /* Damier discret : sans lui, un PNG transparent est illisible en thème sombre. */
    background:
      repeating-conic-gradient(color-mix(in srgb, var(--text) 8%, transparent) 0% 25%, transparent 0% 50%)
      50% / 16px 16px;
  }
  .obar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border);
  }
  .oname {
    font-size: 14px;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sheet.tall {
    max-height: 80vh;
    overflow-y: auto;
  }
  .arc .name {
    word-break: break-all;
  }

  .sftp {
    display: flex;
    flex-direction: column;
    min-height: 100%;
  }
  .pathbar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 10px;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
    position: sticky;
    top: 0;
    z-index: 2;
  }
  .path {
    flex: 1;
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
  }
  .ic {
    display: grid;
    place-items: center;
    width: 38px;
    height: 38px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--text-muted);
    flex: none;
  }
  .ic:disabled {
    opacity: 0.4;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 6px 8px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  li {
    display: flex;
    align-items: center;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .row {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 12px;
    background: transparent;
    border: none;
    color: var(--text);
    padding: 12px;
    text-align: left;
    min-width: 0;
  }
  .meta {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .kebab {
    background: transparent;
    border: none;
    color: var(--text-dim);
    font-size: 20px;
    width: 40px;
    align-self: stretch;
  }
  .dim {
    color: var(--text-dim);
  }
  .pad {
    padding: 16px;
  }
  .ok {
    margin: 0;
    padding: 8px 12px;
    color: var(--brand-primary);
  }
  .err {
    margin: 0;
    padding: 8px 12px;
    color: var(--state-danger);
  }

  .scrim {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    z-index: 40;
  }
  .sheet {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 41;
    background: var(--elevated);
    border-top-left-radius: 16px;
    border-top-right-radius: 16px;
    padding: 10px 10px calc(var(--safe-bottom) + 14px);
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .sheet-title {
    padding: 10px 12px;
    color: var(--text-dim);
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sheet button {
    display: flex;
    align-items: center;
    gap: 12px;
    background: transparent;
    border: none;
    color: var(--text);
    padding: 15px 12px;
    font-size: 16px;
    text-align: left;
    border-radius: var(--radius-sm);
  }
  .sheet button.danger {
    color: var(--state-danger);
  }

  .dialog {
    position: fixed;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    z-index: 41;
    width: min(90vw, 380px);
    background: var(--elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 18px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .dlg-title {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
  }
  .dialog input {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--text);
    padding: 12px;
    font-size: 16px;
    outline: none;
  }
  .dlg-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .dlg-actions button {
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--text);
    padding: 10px 16px;
    font-weight: 600;
  }
  .dlg-actions button.primary {
    background: var(--brand-primary);
    border-color: transparent;
    color: #fff;
  }
  .dlg-actions button.primary.danger {
    background: var(--state-danger);
  }

  .editor {
    position: fixed;
    inset: 0;
    z-index: 50;
    background: var(--bg);
    display: flex;
    flex-direction: column;
    padding-top: var(--safe-top);
  }
  .editor header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }
  .ename {
    flex: 1;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ro {
    flex: none;
    font-size: 11px;
    color: var(--text-dim);
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 3px 8px;
  }
  .save {
    background: var(--brand-primary);
    color: #fff;
    border-color: transparent;
  }
  .save:disabled {
    opacity: 0.5;
  }
  .edwrap {
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }
</style>
