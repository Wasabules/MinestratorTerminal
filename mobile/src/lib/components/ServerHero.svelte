<script lang="ts">
  /**
   * Bannière de la vue serveur (haut de l'Aperçu) : le paysage du biome, le nom, la puce d'état,
   * l'adresse de connexion (copier, partager) et l'alimentation. Elle défile avec l'Aperçu ; la
   * barre du haut de `ServerView`, transparente par-dessus, devient pleine quand elle est passée.
   *
   * Sur le paysage, tout garde les couleurs du thème sombre, quel que soit le thème : les
   * paysages le sont toujours (DESIGN.md).
   */
  import { t } from "../i18n";
  import { isAnon } from "../anon/anon.svelte";
  import type { Biome } from "../servers/colors.svelte";
  import type { ServerListItem } from "../types";
  import BiomeLandscape from "./BiomeLandscape.svelte";
  import Icon from "./Icon.svelte";
  import Masked from "./Masked.svelte";
  import PowerControl from "./PowerControl.svelte";
  import StatusDot from "./StatusDot.svelte";
  import { runtimeMeta } from "../status";

  let {
    server,
    biome,
    runState,
    onPowerDone,
    height = $bindable(0),
  }: {
    server: ServerListItem;
    biome: Biome;
    /** État d'exécution (échantillon live) ; `null` : inconnu. */
    runState: string | null;
    onPowerDone?: () => void;
    /** Hauteur rendue (seuil de la barre du haut pleine). */
    height?: number;
  } = $props();

  const meta = $derived(runtimeMeta(runState, server.status));
  let copied = $state(false);

  /** Mode anonyme : la puce affiche le masque, la copie et le partage portent la vraie adresse. */
  async function copyAddr() {
    try {
      await navigator.clipboard.writeText(server.address);
      copied = true;
      setTimeout(() => (copied = false), 1400);
    } catch {
      /* presse-papiers indisponible */
    }
  }

  /**
   * Partage l'adresse via la feuille de partage Android (`navigator.share`), pour l'envoyer à
   * ses joueurs. Repli sur la copie si l'API manque ; une annulation n'enchaîne sur rien.
   */
  async function shareAddr() {
    try {
      if (navigator.share) {
        await navigator.share({ title: server.name, text: server.address });
        return;
      }
    } catch {
      return; // annulation utilisateur : ne pas enchaîner sur une copie surprise
    }
    void copyAddr();
  }
</script>

<section class="hero" style="background: {biome.sky}" bind:clientHeight={height}>
  <div class="land"><BiomeLandscape {biome} seed={server.id} height={Math.max(height, 1)} pixel={6} /></div>
  <div class="body">
    <h1 class="selectable">{server.name}</h1>
    <div class="chips">
      <span class="chip state" style="color: {meta.ink}">
        <StatusDot color={meta.color} pulse={runState === "running"} size={8} />
        {t(meta.key)}
      </span>
      {#if server.address}
        <button class="chip addr" onclick={copyAddr} aria-label={t("overview.copyAddress")}>
          <span class="mono"><Masked value={server.address} kind="address" /></span>
          {#if copied}
            <Icon name="check" size={15} stroke={2.4} />
            <span class="copied">{isAnon() ? t("anon.copiedReal") : t("overview.copied")}</span>
          {:else}
            <Icon name="copy" size={15} />
          {/if}
        </button>
        <button class="chip share" onclick={shareAddr} aria-label={t("overview.share")}>
          <Icon name="external" size={16} />
        </button>
      {/if}
    </div>
    <PowerControl
      serverId={server.id}
      {runState}
      restartWarning={server.restart_warning}
      stopWarning={server.stop_warning}
      onDone={onPowerDone}
    />
  </div>
</section>

<style>
  .hero {
    position: relative;
    overflow: hidden;
  }
  .land {
    position: absolute;
    inset: 0;
  }
  .body {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 12px;
    /* La barre du haut (transparente) recouvre le haut de la bannière. */
    padding: calc(var(--safe-top) + 74px) 16px 20px;
  }
  h1 {
    margin: 0;
    font-size: 30px;
    line-height: 1.05;
    color: #ffffff;
    overflow-wrap: anywhere;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .chips {
    --state-running: #5fd68c;
    --state-running-text: #8fe8b0;
    --state-pending: #f2b84b;
    --state-offline: #6b7a7f;
    --state-hibernate: #c79bff;
    --text-dim: #8c9a94;
    /* Une seule ligne : l'adresse se tronque plutôt que de renvoyer « Partager » à la ligne. */
    display: flex;
    gap: 8px;
    min-width: 0;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    min-height: 36px;
    padding: 0 12px;
    border-radius: 999px;
    border: none;
    background: #0d1113;
    color: #e4ebe8;
    font-weight: 800;
    font-size: 13.5px;
    line-height: 1.2;
    min-width: 0;
  }
  /* Bouton tactile : 44 px de cible malgré la pilule de 36 px (marge invisible). */
  button.chip {
    position: relative;
  }
  button.chip::after {
    content: "";
    position: absolute;
    inset: -4px 0;
  }
  .chip.state {
    flex: none;
    white-space: nowrap;
  }
  .chip.addr {
    flex: 0 1 auto;
  }
  .chip.addr .mono {
    font-weight: 500;
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .chip.addr :global(svg),
  .chip.share :global(svg) {
    color: #b3bfba;
    flex: none;
  }
  .chip.share {
    width: 36px;
    padding: 0;
    justify-content: center;
    flex: none;
  }
  .copied {
    color: #8fe8b0;
    white-space: nowrap;
  }
</style>
