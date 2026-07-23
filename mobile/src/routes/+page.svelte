<script lang="ts">
  import { api } from "$lib/ipc";
  import { auth } from "$lib/stores/auth.svelte";
  import Onboarding from "$lib/components/Onboarding.svelte";
  import ServersList from "$lib/components/ServersList.svelte";
  import ServerView from "$lib/components/ServerView.svelte";
  import type { ServerListItem } from "$lib/types";

  let selected = $state<ServerListItem | null>(null);

  // Boot : une clé est-elle déjà stockée ? Si oui, on récupère le profil.
  $effect(() => {
    (async () => {
      try {
        if (await api.hasStoredKey()) {
          auth.setUser(await api.getUser());
        }
      } catch {
        auth.setUser(null);
      } finally {
        auth.booted = true;
      }
    })();
  });
</script>

{#if !auth.booted}
  <div class="splash"><div class="logo">⛏️</div></div>
{:else if !auth.isAuthed}
  <Onboarding />
{:else if selected}
  <ServerView server={selected} onBack={() => (selected = null)} />
{:else}
  <ServersList onOpen={(s) => (selected = s)} />
{/if}

<style>
  .splash {
    height: 100dvh;
    display: grid;
    place-items: center;
  }
  .logo {
    width: 72px;
    height: 72px;
    display: grid;
    place-items: center;
    font-size: 34px;
    border-radius: 20px;
    background: var(--brand-gradient);
  }
</style>
