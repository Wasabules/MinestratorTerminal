/**
 * Mode anonyme dans l'éditeur de fichiers (CodeMirror) : les IP, e-mails et adresses de serveurs
 * du document sont **affichés** masqués, le document lui-même reste intact — enregistrement,
 * export, copie et analyse par le Copilote portent sur le vrai texte.
 *
 * Chaque correspondance devient un widget atomique : le curseur saute l'adresse d'un bloc, on ne
 * modifie pas « à l'aveugle » l'intérieur d'une valeur qu'on ne voit pas. Seules les lignes
 * visibles sont analysées.
 */

import { RangeSetBuilder } from '@codemirror/state';
import {
  Decoration,
  EditorView,
  ViewPlugin,
  WidgetType,
  type DecorationSet,
  type ViewUpdate,
} from '@codemirror/view';
import { t } from '#lib/i18n/index.ts';
import { sensitiveMatches } from './mask.ts';

class MaskWidget extends WidgetType {
  constructor(readonly mask: string) {
    super();
  }
  eq(other: MaskWidget): boolean {
    return other.mask === this.mask;
  }
  toDOM(): HTMLElement {
    const el = document.createElement('span');
    el.className = 'cm-anon-mask';
    el.textContent = this.mask;
    el.title = t('anon.maskedHint');
    return el;
  }
}

function build(view: EditorView): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>();
  const doc = view.state.doc;
  let lastLine = 0; // une ligne à cheval sur deux plages visibles n'est traitée qu'une fois
  for (const { from, to } of view.visibleRanges) {
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

const plugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = build(view);
    }
    update(u: ViewUpdate) {
      if (u.docChanged || u.viewportChanged) this.decorations = build(u.view);
    }
  },
  {
    decorations: (v) => v.decorations,
    provide: (p) =>
      EditorView.atomicRanges.of((view) => view.plugin(p)?.decorations ?? Decoration.none),
  },
);

/** Même pastille discrète que `<Masked>` (l'éditeur est toujours sombre, `.force-dark`). */
const theme = EditorView.baseTheme({
  '.cm-anon-mask': {
    padding: '0 3px',
    borderRadius: '4px',
    backgroundColor: 'rgba(241, 245, 243, 0.1)',
    color: '#b3bfba',
  },
});

/** Extension à poser (dans un `Compartment`) tant que le mode anonyme est actif. */
export const anonEditorMask = [plugin, theme];
