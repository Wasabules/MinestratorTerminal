// Tauri n'a pas de serveur Node : on rend l'app en SPA (adapter-static + fallback).
// Voir svelte.config.js et https://v2.tauri.app/start/frontend/sveltekit/
export const ssr = false;
export const prerender = false;
