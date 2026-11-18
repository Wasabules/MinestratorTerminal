<script lang="ts">
  /**
   * Valeur identifiante soumise au mode anonyme (`../anon/anon.svelte.ts`) : en clair mode
   * inactif, remplacée par son masque sinon. Le basculement est instantané (lecture réactive).
   *
   * `kind` choisit le masque :
   * - `address` : adresse de connexion ou IP seule (`•••.•••.•••.•••:25565`, `••••••.minesr.com`) ;
   * - `email` : `•••@•••` ;
   * - `text` : texte libre, seules les IP, adresses et e-mails qu'il contient sont masqués.
   *
   * Le masque est une pastille discrète, non sélectionnable ; les lecteurs d'écran entendent
   * « masqué » plutôt qu'une suite de points.
   */
  import { isAnon } from "../anon/anon.svelte";
  import { EMAIL_MASK, maskAddress, maskSegments, type Segment } from "../anon/mask";
  import { t } from "../i18n";

  let {
    value,
    kind = "text",
  }: { value: string | null | undefined; kind?: "address" | "email" | "text" } = $props();

  const segments = $derived.by((): Segment[] => {
    const v = value ?? "";
    if (!isAnon() || !v) return [{ text: v, masked: false }];
    if (kind === "address") return [{ text: maskAddress(v), masked: true }];
    if (kind === "email") return [{ text: EMAIL_MASK, masked: true }];
    return maskSegments(v);
  });
</script>

{#each segments as s, i (i)}{#if s.masked}<span class="masked"
      ><span aria-hidden="true">{s.text}</span><span class="sr">{t("anon.masked")}</span></span
    >{:else}{s.text}{/if}{/each}

<style>
  /* Pastille posée sur la couleur du texte : lisible sur les cartes, les feuilles et les paysages. */
  .masked {
    position: relative; /* ancre le libellé pour lecteurs d'écran */
    font-family: var(--font-mono);
    font-size: 0.95em;
    letter-spacing: 0.02em;
    padding: 0 0.3em;
    border-radius: 4px;
    background: color-mix(in srgb, currentColor 12%, transparent);
    white-space: nowrap;
    -webkit-user-select: none;
    user-select: none;
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    margin: -1px;
    padding: 0;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }
</style>
