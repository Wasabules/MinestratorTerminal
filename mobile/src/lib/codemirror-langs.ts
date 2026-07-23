/** Choix du langage CodeMirror selon l'extension du fichier (mêmes langages que le desktop). */
import type { Extension } from "@codemirror/state";
import { StreamLanguage, type StreamParser } from "@codemirror/language";
import { json } from "@codemirror/lang-json";
import { yaml } from "@codemirror/lang-yaml";
import { javascript } from "@codemirror/lang-javascript";
import { java } from "@codemirror/lang-java";
import { xml } from "@codemirror/lang-xml";
import { html } from "@codemirror/lang-html";
import { css } from "@codemirror/lang-css";
import { markdown } from "@codemirror/lang-markdown";
import { toml } from "@codemirror/legacy-modes/mode/toml";
import { shell } from "@codemirror/legacy-modes/mode/shell";

/**
 * Highlighter maison `properties`/`ini` (server.properties, bukkit `.properties`, `.env`…).
 * Le mode legacy colore mal ce format ; ici on distingue nettement clé / séparateur / valeur /
 * commentaire / en-tête de section `[section]`.
 */
const propertiesParser: StreamParser<{ inValue: boolean }> = {
  startState: () => ({ inValue: false }),
  token(stream, state) {
    if (stream.sol()) {
      state.inValue = false;
      stream.eatSpace();
      const ch = stream.peek();
      if (ch === "#" || ch === "!" || ch === ";") {
        stream.skipToEnd();
        return "comment";
      }
      if (ch === "[") {
        stream.skipToEnd();
        return "heading"; // [section]
      }
    }
    if (state.inValue) {
      stream.skipToEnd();
      return "string";
    }
    if (stream.eat("=") || stream.eat(":")) {
      state.inValue = true;
      return "operator";
    }
    if (stream.eatWhile(/[^=:\s]/)) return "propertyName";
    if (stream.eatSpace()) return null;
    stream.next();
    return null;
  },
};

function propertiesLang(): Extension {
  return StreamLanguage.define(propertiesParser);
}

export function langFor(filename: string): Extension {
  const name = filename.toLowerCase();
  const ext = name.includes(".") ? name.split(".").pop()! : name;
  switch (ext) {
    case "json":
    case "json5":
    case "mcmeta":
      return json();
    case "yml":
    case "yaml":
      return yaml();
    case "js":
    case "mjs":
    case "cjs":
      return javascript();
    case "java":
      return java();
    case "xml":
      return xml();
    case "html":
    case "htm":
      return html();
    case "css":
      return css();
    case "md":
    case "markdown":
      return markdown();
    case "properties":
    case "ini":
    case "conf":
    case "cfg":
    case "cnf":
    case "env":
      return propertiesLang();
    case "toml":
      return StreamLanguage.define(toml);
    case "sh":
    case "bash":
      return StreamLanguage.define(shell);
    default:
      return [];
  }
}
