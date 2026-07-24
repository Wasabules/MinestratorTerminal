<script lang="ts">
  import { api } from "$lib/ipc";
  import { auth } from "$lib/stores/auth.svelte";
  import { settings } from "$lib/stores/settings.svelte";
  import Onboarding from "$lib/components/Onboarding.svelte";
  import ServersList from "$lib/components/ServersList.svelte";
  import ServerView from "$lib/components/ServerView.svelte";
  import SettingsView from "$lib/components/SettingsView.svelte";
  import UpdateBanner from "$lib/components/UpdateBanner.svelte";
  import type { MyBoxSummary, ServerListItem } from "$lib/types";

  let selected = $state<{ server: ServerListItem; box: MyBoxSummary | null } | null>(null);
  let showSettings = $state(false);

  // Thème : applique au boot + suit le système quand le préréglage est "system".
  $effect(() => {
    settings.applyTheme();
    if (typeof matchMedia === "undefined") return;
    const mq = matchMedia("(prefers-color-scheme: light)");
    const onChange = () => settings.theme === "system" && settings.applyTheme();
    mq.addEventListener?.("change", onChange);
    return () => mq.removeEventListener?.("change", onChange);
  });

  // Boot auth.
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

{#if auth.isAuthed}
  <UpdateBanner />
{/if}

{#if !auth.booted}
  <div class="splash"><div class="logo"></div></div>
{:else if !auth.isAuthed}
  <Onboarding />
{:else if showSettings}
  <SettingsView onBack={() => (showSettings = false)} />
{:else if selected}
  <ServerView server={selected.server} mybox={selected.box} onBack={() => (selected = null)} />
{:else}
  <ServersList
    onOpen={(s, box) => (selected = { server: s, box })}
    onSettings={() => (showSettings = true)}
  />
{/if}

<style>
  .splash {
    height: 100dvh;
    display: grid;
    place-items: center;
  }
  .logo {
    width: 76px;
    height: 76px;
    border-radius: 22px;
    background: var(--brand-gradient);
  }
</style>
