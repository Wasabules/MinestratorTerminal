/** Réglages app (thème + langue), persistés en localStorage, réactifs (runes). */

export type ThemePref = "system" | "dark" | "light";
export type LangPref = "system" | "fr" | "en";

function readLS(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}
function writeLS(key: string, val: string) {
  try {
    localStorage.setItem(key, val);
  } catch {
    /* ignore */
  }
}

class Settings {
  theme = $state<ThemePref>((readLS("theme") as ThemePref) || "system");
  lang = $state<LangPref>((readLS("lang") as LangPref) || "system");
  /** Surveillance en arrière-plan (service au premier plan). Off par défaut (batterie + notif permanente). */
  bgMonitoring = $state<boolean>(readLS("bgMonitoring") === "1");

  /** Locale effective (résout "system" via navigator.language). */
  get locale(): "fr" | "en" {
    if (this.lang === "fr" || this.lang === "en") return this.lang;
    return typeof navigator !== "undefined" && navigator.language?.toLowerCase().startsWith("en")
      ? "en"
      : "fr";
  }

  setTheme(t: ThemePref) {
    this.theme = t;
    writeLS("theme", t);
    this.applyTheme();
  }

  setLang(l: LangPref) {
    this.lang = l;
    writeLS("lang", l);
  }

  setBgMonitoring(on: boolean) {
    this.bgMonitoring = on;
    writeLS("bgMonitoring", on ? "1" : "0");
  }

  /** Applique le thème effectif sur <html data-theme>. */
  applyTheme() {
    if (typeof document === "undefined") return;
    let t: "dark" | "light";
    if (this.theme === "system") {
      t =
        typeof matchMedia !== "undefined" && matchMedia("(prefers-color-scheme: light)").matches
          ? "light"
          : "dark";
    } else {
      t = this.theme;
    }
    document.documentElement.setAttribute("data-theme", t);
  }
}

export const settings = new Settings();
