<script lang="ts">
  import { api, humanizeError } from "../../ipc";
  import { t } from "../../i18n";
  import Icon from "../Icon.svelte";
  import type { SftpEntry } from "../../types";

  let { serverId }: { serverId: number } = $props();

  let cwd = $state("/");
  let entries = $state<SftpEntry[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);

  // Éditeur de fichier (overlay).
  let opened = $state<{ path: string; name: string; content: string; dirty: boolean; saving: boolean } | null>(null);
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
    try {
      const content = await api.sftpReadText(serverId, entry.path);
      opened = { path: entry.path, name: entry.name, content, dirty: false, saving: false };
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
    load();
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
    <button class="ic" onclick={load} aria-label={t("servers.refresh")}>
      <Icon name="refresh" size={18} />
    </button>
  </div>

  {#if notice}<p class="ok">{notice}</p>{/if}
  {#if error}<p class="err selectable">{error}</p>{/if}

  {#if loading}
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

<!-- Feuille d'actions par entrée -->
{#if actionFor}
  <div
    class="scrim"
    role="button"
    tabindex="-1"
    onclick={() => (actionFor = null)}
    onkeydown={(e) => e.key === "Escape" && (actionFor = null)}
  ></div>
  <div class="sheet">
    <div class="sheet-title selectable">{actionFor.name}</div>
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
  <div class="scrim" role="button" tabindex="-1" onclick={() => (modal = null)} onkeydown={() => {}}></div>
  <div class="dialog">
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
  <div class="editor">
    <header>
      <button class="ic" onclick={() => (opened = null)} aria-label="Fermer"><Icon name="back" size={20} /></button>
      <span class="ename selectable">{opened.name}{opened.dirty ? " •" : ""}</span>
      <button class="save" disabled={opened.saving} onclick={save}>
        <Icon name="save" size={16} /> {t("files.save")}
      </button>
    </header>
    <textarea
      class="selectable"
      spellcheck="false"
      autocapitalize="off"
      bind:value={opened.content}
      oninput={() => opened && (opened.dirty = true)}
    ></textarea>
  </div>
{/if}

<style>
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
  .save {
    display: flex;
    align-items: center;
    gap: 6px;
    background: var(--brand-primary);
    color: #fff;
    border: none;
    border-radius: var(--radius-sm);
    padding: 9px 14px;
    font-weight: 600;
  }
  .save:disabled {
    opacity: 0.5;
  }
  textarea {
    flex: 1;
    width: 100%;
    resize: none;
    border: none;
    outline: none;
    background: #0d1114;
    color: var(--text);
    font-family: var(--font-mono);
    font-size: 13px;
    line-height: 1.5;
    padding: 12px;
  }
</style>
