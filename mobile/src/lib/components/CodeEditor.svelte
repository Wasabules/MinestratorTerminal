<script lang="ts">
  import { untrack } from "svelte";
  import { EditorView, keymap, lineNumbers, highlightActiveLine, drawSelection } from "@codemirror/view";
  import { EditorState, Compartment } from "@codemirror/state";
  import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
  import {
    syntaxHighlighting,
    defaultHighlightStyle,
    indentOnInput,
    bracketMatching,
    foldGutter,
  } from "@codemirror/language";
  import { search, openSearchPanel, searchKeymap, highlightSelectionMatches } from "@codemirror/search";
  import { oneDark } from "@codemirror/theme-one-dark";
  import { langFor } from "../codemirror-langs";

  let {
    value,
    filename,
    onChange,
    readOnly = false,
  }: { value: string; filename: string; onChange: (v: string) => void; readOnly?: boolean } = $props();

  let host: HTMLDivElement | null = $state(null);
  let view: EditorView | null = null;
  let pinching = $state(false);

  const fontComp = new Compartment();
  function clampFont(n: number) {
    return Math.max(8, Math.min(28, Math.round(n)));
  }
  let fontSize = clampFont(Number(readLS("editor.font")) || 13);
  function readLS(k: string) {
    try {
      return localStorage.getItem(k);
    } catch {
      return null;
    }
  }
  function fontTheme(px: number) {
    return EditorView.theme({ "&": { fontSize: `${px}px` } });
  }
  function setFont(n: number) {
    const v = clampFont(n);
    if (v === fontSize && view) return;
    fontSize = v;
    try {
      localStorage.setItem("editor.font", String(v));
    } catch {
      /* ignore */
    }
    view?.dispatch({ effects: fontComp.reconfigure(fontTheme(v)) });
  }

  /** Ouvre le panneau de recherche (bouton loupe du parent). */
  export function openSearch() {
    if (view) {
      openSearchPanel(view);
      view.focus();
    }
  }

  // --- Pinch to zoom (deux doigts) ---
  let pinchDist = 0;
  let pinchFont = 13;
  function touchDist(t: TouchList) {
    return Math.hypot(t[0].clientX - t[1].clientX, t[0].clientY - t[1].clientY);
  }
  function onTouchStart(e: TouchEvent) {
    if (e.touches.length === 2) {
      pinching = true;
      pinchDist = touchDist(e.touches);
      pinchFont = fontSize;
    }
  }
  function onTouchMove(e: TouchEvent) {
    if (pinching && e.touches.length === 2) {
      e.preventDefault();
      const ratio = touchDist(e.touches) / (pinchDist || 1);
      setFont(pinchFont * ratio);
    }
  }
  function onTouchEnd(e: TouchEvent) {
    if (e.touches.length < 2) pinching = false;
  }

  $effect(() => {
    if (!host) return;
    const initial = untrack(() => value);
    const fname = untrack(() => filename);
    const ro = untrack(() => readOnly);
    const dark =
      typeof document !== "undefined" && document.documentElement.getAttribute("data-theme") !== "light";

    view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: initial,
        extensions: [
          lineNumbers(),
          foldGutter(),
          history(),
          drawSelection(),
          highlightActiveLine(),
          highlightSelectionMatches(),
          indentOnInput(),
          bracketMatching(),
          syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
          search({ top: true }),
          keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap, indentWithTab]),
          EditorView.lineWrapping,
          EditorView.contentAttributes.of({
            autocapitalize: "off",
            autocorrect: "off",
            autocomplete: "off",
            spellcheck: "false",
            "data-gramm": "false",
          }),
          fontComp.of(fontTheme(fontSize)),
          EditorState.readOnly.of(ro),
          EditorView.editable.of(!ro),
          langFor(fname),
          dark ? oneDark : [],
          EditorView.updateListener.of((u) => {
            if (u.docChanged) onChange(u.state.doc.toString());
          }),
          EditorView.theme({
            "&": { height: "100%", backgroundColor: "transparent" },
            ".cm-scroller": { fontFamily: "var(--font-mono)" },
            ".cm-gutters": { backgroundColor: "transparent", border: "none" },
          }),
        ],
      }),
    });

    return () => {
      view?.destroy();
      view = null;
    };
  });
</script>

<div
  class="cm-host"
  class:pinching
  role="group"
  bind:this={host}
  ontouchstart={onTouchStart}
  ontouchmove={onTouchMove}
  ontouchend={onTouchEnd}
></div>

<style>
  .cm-host {
    height: 100%;
    overflow: hidden;
    background: #0d1114;
  }
  /* Pendant un pinch : couper le scroll/gestes natifs pour un zoom fluide. */
  .cm-host.pinching {
    touch-action: none;
  }
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
