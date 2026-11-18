<script lang="ts">
  // Polices embarquées (paquets @fontsource, licence OFL) : servies par l'app, jamais par un
  // CDN — le webview ne fait aucune requête réseau.
  import "@fontsource-variable/nunito";
  import "@fontsource-variable/jetbrains-mono";
  import "../app.css";
  let { children } = $props();
</script>

{@render children()}
