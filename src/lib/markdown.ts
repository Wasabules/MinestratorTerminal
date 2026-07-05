/**
 * Rendu Markdown → HTML **sûr**, sans dépendance.
 *
 * Tout le texte est échappé ; seules des balises whitelistées sont émises (pas de HTML brut
 * issu du modèle → pas de risque XSS). Sous-ensemble courant : titres, gras/italique, code
 * inline, blocs de code, listes, citations, liens, règles horizontales.
 */

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

/** Formatage en ligne (sur une chaîne déjà découpée en ligne/paragraphe). */
function inline(src: string): string {
  let t = escapeHtml(src);
  // code inline `…`
  t = t.replace(/`([^`]+)`/g, (_m, c) => `<code>${c}</code>`);
  // gras **…**
  t = t.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
  // italique *…* (sans toucher aux ** déjà traités)
  t = t.replace(/(^|[^*])\*([^*\n]+)\*/g, '$1<em>$2</em>');
  // liens [texte](url) — url http(s) uniquement
  t = t.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, (_m, txt, url) => {
    const safe = /^https?:\/\//i.test(url) ? url : '#';
    return `<a href="${safe}" target="_blank" rel="noopener noreferrer">${txt}</a>`;
  });
  return t;
}

const BLOCK_START = /^(#{1,6}\s|```|>\s?|\s*[-*+]\s|\s*\d+\.\s)|^(---|\*\*\*|___)\s*$/;

/** Une ligne de séparation de tableau GFM : `|---|:--:|---|` (tirets, deux-points, pipes). */
function isTableSeparator(line: string): boolean {
  const s = line.trim();
  return /^\|?[\s:|-]+\|?$/.test(s) && s.includes('-') && s.includes('|');
}

/** Découpe une ligne de tableau `| a | b |` en cellules nettoyées. */
function splitRow(line: string): string[] {
  let s = line.trim();
  if (s.startsWith('|')) s = s.slice(1);
  if (s.endsWith('|')) s = s.slice(0, -1);
  return s.split('|').map((c) => c.trim());
}

/** La ligne `i` amorce-t-elle un tableau GFM (en-tête `| … |` + séparateur juste après) ? */
function tableStartsAt(lines: string[], i: number): boolean {
  return i + 1 < lines.length && lines[i].includes('|') && isTableSeparator(lines[i + 1]);
}

export function renderMarkdown(md: string): string {
  const lines = md.replace(/\r\n/g, '\n').split('\n');
  const out: string[] = [];
  let list: 'ul' | 'ol' | null = null;
  const closeList = () => {
    if (list) {
      out.push(`</${list}>`);
      list = null;
    }
  };

  let i = 0;
  while (i < lines.length) {
    const line = lines[i];

    // Bloc de code ```lang … ```
    if (/^```/.test(line)) {
      closeList();
      const buf: string[] = [];
      i++;
      while (i < lines.length && !/^```\s*$/.test(lines[i])) {
        buf.push(lines[i]);
        i++;
      }
      i++; // saute la clôture
      out.push(`<pre><code>${escapeHtml(buf.join('\n'))}</code></pre>`);
      continue;
    }

    const h = line.match(/^(#{1,6})\s+(.*)$/);
    if (h) {
      closeList();
      const lvl = Math.min(h[1].length, 6);
      out.push(`<h${lvl}>${inline(h[2])}</h${lvl}>`);
      i++;
      continue;
    }

    if (/^(---|\*\*\*|___)\s*$/.test(line)) {
      closeList();
      out.push('<hr />');
      i++;
      continue;
    }

    const bq = line.match(/^>\s?(.*)$/);
    if (bq) {
      closeList();
      out.push(`<blockquote>${inline(bq[1])}</blockquote>`);
      i++;
      continue;
    }

    const ol = line.match(/^\s*\d+\.\s+(.*)$/);
    if (ol) {
      if (list !== 'ol') {
        closeList();
        out.push('<ol>');
        list = 'ol';
      }
      out.push(`<li>${inline(ol[1])}</li>`);
      i++;
      continue;
    }

    const ul = line.match(/^\s*[-*+]\s+(.*)$/);
    if (ul) {
      if (list !== 'ul') {
        closeList();
        out.push('<ul>');
        list = 'ul';
      }
      out.push(`<li>${inline(ul[1])}</li>`);
      i++;
      continue;
    }

    // Tableau GFM : ligne d'en-tête `| a | b |` suivie d'un séparateur `|---|---|`.
    if (tableStartsAt(lines, i)) {
      closeList();
      const headers = splitRow(line);
      i += 2;
      const rows: string[][] = [];
      while (i < lines.length && lines[i].trim() !== '' && lines[i].includes('|')) {
        rows.push(splitRow(lines[i]));
        i++;
      }
      let html = '<table><thead><tr>';
      for (const h of headers) html += `<th>${inline(h)}</th>`;
      html += '</tr></thead><tbody>';
      for (const row of rows) {
        html += '<tr>';
        for (let c = 0; c < headers.length; c++) html += `<td>${inline(row[c] ?? '')}</td>`;
        html += '</tr>';
      }
      out.push(`${html}</tbody></table>`);
      continue;
    }

    if (line.trim() === '') {
      closeList();
      i++;
      continue;
    }

    // Paragraphe : accumule les lignes jusqu'au prochain bloc/vide.
    closeList();
    const para: string[] = [line];
    i++;
    while (
      i < lines.length &&
      lines[i].trim() !== '' &&
      !BLOCK_START.test(lines[i]) &&
      !tableStartsAt(lines, i) // un tableau collé au paragraphe doit rester un tableau
    ) {
      para.push(lines[i]);
      i++;
    }
    out.push(`<p>${inline(para.join(' '))}</p>`);
  }

  closeList();
  return out.join('\n');
}
