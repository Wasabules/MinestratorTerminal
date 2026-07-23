<script lang="ts">
  import { icons } from "../icons";
  let {
    name,
    size = 22,
    stroke = 2,
  }: { name: keyof typeof icons | string; size?: number; stroke?: number } = $props();
  // Contenu statique (constantes internes) → {@html} sûr.
  const inner = $derived(icons[name] ?? "");
</script>

<svg
  xmlns="http://www.w3.org/2000/svg"
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width={stroke}
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  {@html inner}
</svg>
