<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { EditorState, Compartment, type Extension } from '@codemirror/state';
  import { EditorView, keymap, lineNumbers, highlightActiveLine } from '@codemirror/view';
  import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
  import { search, searchKeymap, highlightSelectionMatches } from '@codemirror/search';
  import { StreamLanguage, LanguageSupport, indentUnit } from '@codemirror/language';
  import { oneDark } from '@codemirror/theme-one-dark';
  import { json } from '@codemirror/lang-json';
  import { yaml } from '@codemirror/lang-yaml';
  import { javascript } from '@codemirror/lang-javascript';
  import { html } from '@codemirror/lang-html';
  import { xml } from '@codemirror/lang-xml';
  import { css } from '@codemirror/lang-css';
  import { markdown } from '@codemirror/lang-markdown';
  import { java } from '@codemirror/lang-java';
  import { properties } from '@codemirror/legacy-modes/mode/properties';
  import { shell } from '@codemirror/legacy-modes/mode/shell';
  import { toml } from '@codemirror/legacy-modes/mode/toml';
  import { api, humanizeError } from '$lib/ipc';
  import { openCopilotMenu } from '$lib/copilot/menu.svelte';
  import { PASTE_SERVICES, pasteExport } from '$lib/paste';
  import { t } from '$lib/i18n';
  import Icon from '../Icon.svelte';

  let {
    serverId,
    path,
    name,
    content,
    onClose,
    onSaved,
    readonly = false,
  }: {
    serverId: number;
    path: string;
    name: string;
    content: string;
    onClose: () => void;
    onSaved?: () => void;
    /** Affichage LECTURE SEULE (contenu d'archive) : édition et sauvegarde désactivées. */
    readonly?: boolean;
  } = $props();

  let host: HTMLDivElement;
  let view: EditorView | undefined;
  let dirty = $state(false);
  let saving = $state(false);
  let wrap = $state(false);
  let expOpen = $state(false);
  let message = $state<string | null>(null);
  let msgKind = $state<'ok' | 'err'>('ok');
  let msgTimer: ReturnType<typeof setTimeout> | undefined;

  const lang = $derived(detect(name));
  const wrapComp = new Compartment();

  function detect(fname: string): { label: string; ext: Extension; isJson: boolean } {
    const e = fname.split('.').pop()?.toLowerCase() ?? '';
    const stream = (mode: Parameters<typeof StreamLanguage.define>[0]) =>
      new LanguageSupport(StreamLanguage.define(mode));
    switch (e) {
      case 'json':
        return { label: 'JSON', ext: json(), isJson: true };
      case 'yml':
      case 'yaml':
        return { label: 'YAML', ext: yaml(), isJson: false };
      case 'js':
      case 'mjs':
      case 'cjs':
        return { label: 'JavaScript', ext: javascript(), isJson: false };
      case 'ts':
        return { label: 'TypeScript', ext: javascript({ typescript: true }), isJson: false };
      case 'html':
      case 'htm':
        return { label: 'HTML', ext: html(), isJson: false };
      case 'xml':
        return { label: 'XML', ext: xml(), isJson: false };
      case 'css':
        return { label: 'CSS', ext: css(), isJson: false };
      case 'md':
      case 'markdown':
        return { label: 'Markdown', ext: markdown(), isJson: false };
      case 'java':
        return { label: 'Java', ext: java(), isJson: false };
      case 'properties':
        return { label: 'Properties', ext: stream(properties), isJson: false };
      case 'sh':
      case 'bash':
        return { label: 'Shell', ext: stream(shell), isJson: false };
      case 'toml':
        return { label: 'TOML', ext: stream(toml), isJson: false };
      default:
        return { label: t('sftp.plainText'), ext: [], isJson: false };
    }
  }

  onMount(() => {
    const state = EditorState.create({
      doc: content,
      extensions: [
        lineNumbers(),
        highlightActiveLine(),
        highlightSelectionMatches(),
        search({ top: true }),
        history(),
        indentUnit.of('  '),
        keymap.of([
          ...defaultKeymap,
          ...historyKeymap,
          ...searchKeymap,
          indentWithTab,
          {
            key: 'Mod-s',
            preventDefault: true,
            run: () => {
              void save();
              return true;
            },
          },
        ]),
        lang.ext,
        oneDark,
        ...(readonly ? [EditorState.readOnly.of(true), EditorView.editable.of(false)] : []),
        wrapComp.of([]),
        EditorView.updateListener.of((u) => {
          if (u.docChanged) dirty = true;
        }),
        EditorView.theme({
          '&': { height: '100%' },
          '.cm-scroller': { fontFamily: 'var(--font-mono)', fontSize: '13px' },
        }),
      ],
    });
    view = new EditorView({ state, parent: host });
  });

  onDestroy(() => {
    clearTimeout(msgTimer);
    view?.destroy();
  });

  function flash(text: string, kind: 'ok' | 'err' = 'ok') {
    message = text;
    msgKind = kind;
    clearTimeout(msgTimer);
    msgTimer = setTimeout(() => (message = null), 1800);
  }

  function replaceDoc(text: string) {
    if (!view) return;
    view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text } });
  }

  async function save() {
    if (!view) return;
    saving = true;
    try {
      await api.sftpWriteText(serverId, path, view.state.doc.toString());
      dirty = false;
      flash(t('sftp.saved'));
      onSaved?.();
    } catch (e) {
      flash(humanizeError(e), 'err');
    } finally {
      saving = false;
    }
  }

  function format() {
    if (!view || !lang.isJson) return;
    try {
      replaceDoc(JSON.stringify(JSON.parse(view.state.doc.toString()), null, 2));
    } catch {
      flash(t('sftp.invalidJson'), 'err');
    }
  }

  function minify() {
    if (!view || !lang.isJson) return;
    try {
      replaceDoc(JSON.stringify(JSON.parse(view.state.doc.toString())));
    } catch {
      flash(t('sftp.invalidJson'), 'err');
    }
  }

  function compact() {
    if (!view) return;
    const cleaned = view.state.doc
      .toString()
      .split('\n')
      .map((l) => l.replace(/[ \t]+$/, ''))
      .join('\n')
      .replace(/\n{3,}/g, '\n\n')
      .replace(/^\n+/, '')
      .replace(/\n+$/, '\n');
    replaceDoc(cleaned);
  }

  function toggleWrap() {
    wrap = !wrap;
    view?.dispatch({ effects: wrapComp.reconfigure(wrap ? EditorView.lineWrapping : []) });
  }

  async function doExport(service: string) {
    expOpen = false;
    if (!view) return;
    try {
      await pasteExport(service, view.state.doc.toString());
      flash(t('sftp.exported'));
    } catch (e) {
      flash(humanizeError(e), 'err');
    }
  }

  function onContextMenu(e: MouseEvent) {
    if (!view) return;
    const { from, to } = view.state.selection.main;
    if (from === to) return; // pas de sélection → menu natif
    e.preventDefault();
    openCopilotMenu({
      x: e.clientX,
      y: e.clientY,
      text: view.state.sliceDoc(from, to),
      serverId,
      serverName: `${name} · #${serverId}`,
    });
  }
</script>

<div class="editor">
  <div class="bar">
    <div class="left">
      <span class="fname">{name}</span>
      {#if readonly}
        <span class="lang ro">{t('sftp.readonly')}</span>
      {:else if dirty}
        <span class="dirtydot" title={t('sftp.unsaved')}></span>
      {/if}
      <span class="lang">{lang.label}</span>
    </div>

    <div class="tools">
      {#if !readonly}
        {#if lang.isJson}
          <button class="tool" onclick={format}>{t('sftp.format')}</button>
          <button class="tool" onclick={minify}>{t('sftp.minify')}</button>
          <span class="vsep"></span>
        {/if}
        <button class="tool" onclick={compact}>{t('sftp.compact')}</button>
      {/if}
      <button class="tool" class:on={wrap} onclick={toggleWrap}>{t('sftp.wrap')}</button>
      <span class="exp">
        <button class="tool" class:on={expOpen} onclick={() => (expOpen = !expOpen)}>{t('sftp.export')} ▾</button>
        {#if expOpen}
          <button class="expback" onclick={() => (expOpen = false)} aria-label={t('common.close')}></button>
          <div class="expmenu">
            {#each PASTE_SERVICES as s (s.id)}
              <button class="expitem" onclick={() => doExport(s.id)}>{s.label}</button>
            {/each}
          </div>
        {/if}
      </span>
    </div>

    <div class="grow"></div>

    {#if message}<span class="msg" class:err={msgKind === 'err'}>{message}</span>{/if}
    {#if !readonly}
      <button class="save" onclick={save} disabled={saving || !dirty}>{saving ? '…' : t('sftp.save')}</button>
    {/if}
    <button class="close" onclick={onClose} title={t('common.close')}><Icon name="x" size={16} /></button>
  </div>

  <div class="cm" bind:this={host} oncontextmenu={onContextMenu} role="presentation"></div>
</div>

<style>
  /* L'éditeur est toujours sombre (CodeMirror one-dark) → palette fixe, pas de tokens de thème. */
  .editor {
    height: 100%;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    background: #0d1214;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 12px;
    background: #141b1e;
    border-bottom: 1px solid #232c31;
    flex: none;
  }
  .left {
    display: flex;
    align-items: center;
    gap: 9px;
    min-width: 0;
  }
  .fname {
    font-family: var(--font-mono);
    font-weight: 600;
    font-size: 13px;
    color: #e6edf0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 320px;
  }
  .dirtydot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: #f0b429;
    flex: none;
  }
  .lang {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.04em;
    color: #56d4ac;
    background: rgba(0, 155, 114, 0.16);
    border-radius: 6px;
    padding: 3px 9px;
    flex: none;
  }
  .lang.ro {
    color: var(--text-muted);
    background: color-mix(in srgb, var(--text) 10%, transparent);
  }
  .tools {
    display: flex;
    align-items: center;
    gap: 4px;
    margin-left: 4px;
  }
  .vsep {
    width: 1px;
    height: 18px;
    background: #2a343a;
    margin: 0 4px;
  }
  .tool {
    background: transparent;
    border: 1px solid #2a343a;
    border-radius: 7px;
    cursor: pointer;
    font: inherit;
    font-size: 12px;
    color: #aeb9be;
    padding: 5px 11px;
    transition: background 0.12s ease, color 0.12s ease, border-color 0.12s ease;
  }
  .tool:hover {
    color: #eef3f5;
    background: #1e262a;
    border-color: #3a454b;
  }
  .tool.on {
    color: #fff;
    background: #009b72;
    border-color: #009b72;
  }
  .exp {
    position: relative;
    display: inline-flex;
  }
  .expback {
    position: fixed;
    inset: 0;
    background: none;
    border: none;
    z-index: 5;
    cursor: default;
  }
  .expmenu {
    position: absolute;
    top: calc(100% + 5px);
    left: 0;
    z-index: 6;
    min-width: 140px;
    background: #141b1e;
    border: 1px solid #2a343a;
    border-radius: 9px;
    box-shadow: 0 10px 26px rgba(0, 0, 0, 0.5);
    padding: 5px;
    display: flex;
    flex-direction: column;
  }
  .expitem {
    text-align: left;
    background: none;
    border: none;
    cursor: pointer;
    font: inherit;
    font-size: 12.5px;
    color: #d6e2e6;
    padding: 7px 10px;
    border-radius: 6px;
  }
  .expitem:hover {
    background: #1e262a;
  }
  .grow {
    flex: 1;
  }
  .msg {
    font-size: 12px;
    color: #56d4ac;
  }
  .msg.err {
    color: #ff6b6b;
  }
  .save {
    background: #009b72;
    color: #fff;
    border: none;
    border-radius: 7px;
    font: inherit;
    font-weight: 600;
    font-size: 12.5px;
    padding: 6px 15px;
    cursor: pointer;
  }
  .save:hover:not(:disabled) {
    filter: brightness(1.09);
  }
  .save:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .close {
    display: inline-flex;
    align-items: center;
    background: transparent;
    border: none;
    color: #8a99a0;
    cursor: pointer;
    padding: 6px 10px;
    border-radius: 7px;
    line-height: 1;
  }
  .close:hover {
    color: #eef3f5;
    background: #1e262a;
  }
  .cm {
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }
</style>
