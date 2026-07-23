/** Menu contextuel « Copilote » sur sélection de texte (rune `$state` partagée). */

interface MenuState {
  open: boolean;
  x: number;
  y: number;
  text: string;
  serverId: number;
  serverName: string;
}

const state = $state<MenuState>({
  open: false,
  x: 0,
  y: 0,
  text: '',
  serverId: 0,
  serverName: '',
});

export function openCopilotMenu(o: {
  x: number;
  y: number;
  text: string;
  serverId: number;
  serverName: string;
}): void {
  state.open = true;
  state.x = o.x;
  state.y = o.y;
  state.text = o.text;
  state.serverId = o.serverId;
  state.serverName = o.serverName;
}

export function closeCopilotMenu(): void {
  state.open = false;
}

export function copilotMenu(): MenuState {
  return state;
}
