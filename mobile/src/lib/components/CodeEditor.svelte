<script lang="ts">
  /**
   * Éditeur de fichiers (CodeMirror). Zone toujours sombre (`.force-dark`, thème One Dark) quel que
   * soit le thème de l'app, comme la console et l'éditeur desktop.
   *
   * Mode anonyme : les IP, e-mails et adresses de serveurs du document sont **affichés** masqués
   * (même rendu que `<Masked>`), le document reste intact — enregistrement et copie portent sur le
   * vrai texte. Chaque masque est un bloc atomique : le curseur saute l'adresse d'un coup.
   */
  import { untrack } from "svelte";
  import {
    EditorView,
    keymap,
    lineNumbers,
    highlightActiveLine,
    drawSelection,
    Decoration,
    ViewPlugin,
    WidgetType,
    type DecorationSet,
    type ViewUpdate,
  } from "@codemirror/view";
  import { EditorState, Compartment, RangeSetBuilder } from "@codemirror/state";
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
  import { isAnon } from "../anon/anon.svelte";
  import { sensitiveMatches } from "../anon/mask";
  import { t } from "../i18n";

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

  // --- Mode anonyme : masques posés par-dessus le texte visible ---
  // Pas de propriété de paramètre (`readonly mask` au constructeur) : le TypeScript des
  // composants Svelte ne fait qu'effacer les types.
  class MaskWidget extends WidgetType {
    mask: string;
    constructor(mask: string) {
      super();
      this.mask = mask;
    }
    eq(other: MaskWidget): boolean {
      return other.mask === this.mask;
    }
    toDOM(): HTMLElement {
      const el = document.createElement("span");
      el.className = "cm-anon-mask";
      el.textContent = this.mask;
      el.setAttribute("aria-label", t("anon.masked"));
      return el;
    }
  }

  /** Décorations des lignes visibles (une ligne à cheval sur deux plages n'est traitée qu'une fois). */
  function buildMasks(v: EditorView): DecorationSet {
    const builder = new RangeSetBuilder<Decoration>();
    const doc = v.state.doc;
    let lastLine = 0;
    for (const { from, to } of v.visibleRanges) {
      for (let pos = from; pos <= to; ) {
        const line = doc.lineAt(pos);
        if (line.number > lastLine) {
          lastLine = line.number;
          for (const m of sensitiveMatches(line.text)) {
            builder.add(
              line.from + m.from,
              line.from + m.to,
              Decoration.replace({ widget: new MaskWidget(m.mask) }),
            );
          }
        }
        pos = line.to + 1;
      }
    }
    return builder.finish();
  }

  const maskPlugin = ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;
      constructor(v: EditorView) {
        this.decorations = buildMasks(v);
      }
      update(u: ViewUpdate) {
        if (u.docChanged || u.viewportChanged) this.decorations = buildMasks(u.view);
      }
    },
    {
      decorations: (p) => p.decorations,
      provide: (p) =>
        EditorView.atomicRanges.of((v) => v.plugin(p)?.decorations ?? Decoration.none),
    },
  );
  /** Même pastille discrète que `<Masked>` (l'éditeur est toujours sombre). */
  const anonMask = [
    maskPlugin,
    EditorView.baseTheme({
      ".cm-anon-mask": {
        padding: "0 3px",
        borderRadius: "4px",
        backgroundColor: "rgba(241, 245, 243, 0.1)",
        color: "#b3bfba",
      },
    }),
  ];
  const anonComp = new Compartment();

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
    const anon = untrack(() => isAnon());

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
          // Toujours sombre : la zone l'est dans les deux thèmes (`.force-dark`).
          oneDark,
          anonComp.of(anon ? anonMask : []),
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

  // Bascule du mode anonyme : masques posés ou retirés aussitôt, sans recréer l'éditeur.
  $effect(() => {
    const on = isAnon();
    view?.dispatch({ effects: anonComp.reconfigure(on ? anonMask : []) });
  });
</script>

<div
  class="cm-host force-dark"
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
    background: var(--chrome);
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
