/**
 * Action Svelte : téléporte un élément vers <body>.
 *
 * Nécessaire pour les overlays `position:fixed` : un ancêtre avec `transform` (le pager) crée
 * un bloc conteneur qui « capture » le fixed → l'overlay se positionnerait par rapport à la piste
 * translatée (hors écran) au lieu du viewport. Le portail le sort de ce contexte.
 */
export function portal(node: HTMLElement) {
  document.body.appendChild(node);
  return {
    destroy() {
      node.remove();
    },
  };
}
