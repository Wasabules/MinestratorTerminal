<script lang="ts">
  import { t } from "../../i18n";
  // À venir : liste des joueurs en ligne (via liveLight.players.list) + actions
  // swipe (kick/ban/op/whitelist) branchées sur api.playerAction.
  // (serverId sera consommé quand la vue sera implémentée.)
  let { serverId: _serverId }: { serverId: number } = $props();
</script>

<div class="view">
  <p class="dim">{t("players.soon")}</p>
</div>

<style>
  .view {
    padding: 24px 16px;
  }
  .dim {
    color: var(--text-dim);
  }
</style>
