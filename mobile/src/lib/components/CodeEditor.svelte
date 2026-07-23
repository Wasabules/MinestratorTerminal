<script lang="ts">
  import { untrack } from "svelte";
  import { EditorView, keymap, lineNumbers, highlightActiveLine, drawSelection } from "@codemirror/view";
  import { EditorState } from "@codemirror/state";
  import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
  import {
    syntaxHighlighting,
    defaultHighlightStyle,
    indentOnInput,
    bracketMatching,
    foldGutter,
  } from "@codemirror/language";
  import { oneDark } from "@codemirror/theme-one-dark";
  import { langFor } from "../codemirror-langs";

  let { value, filename, onChange }: { value: string; filename: string; onChange: (v: string) => void } =
    $props();

  let host: HTMLDivElement | null = $state(null);

  $effect(() => {
    if (!host) return;
    // Valeurs initiales lues SANS réactivité : l'éditeur n'est pas recréé quand on tape
    // (le flux sort via onChange ; on ne réinjecte pas `value`).
    const initial = untrack(() => value);
    const fname = untrack(() => filename);
    const dark =
      typeof document !== "undefined" && document.documentElement.getAttribute("data-theme") !== "light";

    const view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: initial,
        extensions: [
          lineNumbers(),
          foldGutter(),
          history(),
          drawSelection(),
          highlightActiveLine(),
          indentOnInput(),
          bracketMatching(),
          syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
          keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
          EditorView.lineWrapping,
          langFor(fname),
          dark ? oneDark : [],
          EditorView.updateListener.of((u) => {
            if (u.docChanged) onChange(u.state.doc.toString());
          }),
          EditorView.theme({
            "&": { height: "100%", fontSize: "13px", backgroundColor: "transparent" },
            ".cm-scroller": { fontFamily: "var(--font-mono)" },
            ".cm-gutters": { backgroundColor: "transparent", border: "none" },
          }),
        ],
      }),
    });

    return () => view.destroy();
  });
</script>

<div class="cm-host" bind:this={host}></div>

<style>
  .cm-host {
    height: 100%;
    overflow: hidden;
    background: #0d1114;
  }
  /* CodeMirror utilise un contenteditable : autoriser la sélection (le body la coupe). */
  :global(.cm-editor) {
    height: 100%;
    -webkit-user-select: text;
    user-select: text;
  }
  :global(.cm-editor .cm-content) {
    -webkit-user-select: text;
    user-select: text;
  }
</style>
