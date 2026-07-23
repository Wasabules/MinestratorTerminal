/** Choix du langage CodeMirror selon l'extension du fichier (mêmes langages que le desktop). */
import type { Extension } from "@codemirror/state";
import { StreamLanguage } from "@codemirror/language";
import { json } from "@codemirror/lang-json";
import { yaml } from "@codemirror/lang-yaml";
import { javascript } from "@codemirror/lang-javascript";
import { java } from "@codemirror/lang-java";
import { xml } from "@codemirror/lang-xml";
import { html } from "@codemirror/lang-html";
import { css } from "@codemirror/lang-css";
import { markdown } from "@codemirror/lang-markdown";
import { properties } from "@codemirror/legacy-modes/mode/properties";
import { toml } from "@codemirror/legacy-modes/mode/toml";
import { shell } from "@codemirror/legacy-modes/mode/shell";

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
      return StreamLanguage.define(properties);
    case "toml":
      return StreamLanguage.define(toml);
    case "sh":
    case "bash":
      return StreamLanguage.define(shell);
    default:
      return [];
  }
}
