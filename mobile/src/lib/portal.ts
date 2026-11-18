/**
 * Action Svelte : téléporte un élément vers <body>.
 *
 * Nécessaire pour les overlays `position:fixed` : un ancêtre avec `transform` (le pager) crée
 * un bloc conteneur qui « capture » le fixed → l'overlay se positionnerait par rapport à la piste
 * translatée (hors écran) au lieu du viewport. Le portail le sort de ce contexte.
 *
 * Sorti de la vue serveur, l'élément perdrait l'accent du biome (variables posées sur le
 * conteneur `.biome`) : on lui recopie la palette brute et on le marque `data-biome`, qu'app.css
 * résout comme `.biome`. Un attribut plutôt qu'une classe : Svelte réécrit `class` à chaque
 * mise à jour d'une classe dynamique, pas les attributs qu'il ne gère pas.
 */
const BIOME_VARS = ["--b-accent", "--b-accent-l", "--b-ink", "--b-soft", "--b-soft-text", "--b-soft-l"];

export function portal(node: HTMLElement) {
  const host = node.parentElement?.closest<HTMLElement>(".biome, [data-biome]");
  if (host) {
    for (const name of BIOME_VARS) {
      const v = host.style.getPropertyValue(name);
      if (v) node.style.setProperty(name, v);
    }
    node.dataset.biome = "";
  }
  document.body.appendChild(node);
  return {
    destroy() {
      node.remove();
    },
  };
}
