/**
 * Aides partagées par la gestion des MyBox. Les validations qui font foi sont en Rust
 * (`core/src/mybox.rs`) : celles-ci ne servent qu'à guider la saisie (bornes des curseurs, boutons
 * désactivés) et reprennent les mêmes règles.
 */
import { isAppError } from '#lib/ipc.ts';
import type { MyBoxCapacity, MyBoxServerSlot } from '#lib/types.ts';

/** Code MineStrator d'une erreur d'API (`API_MYBOX_NEED_OFFLINE_SERVER`…), sinon `null`. */
export function apiCode(e: unknown): string | null {
  return isAppError(e) && e.kind === 'api' && e.code ? e.code : null;
}

/** Snapshot avant suppression possible ? (Conditions du panel, comme `mybox::snapshot_possible`.) */
export function snapshotPossible(cap: MyBoxCapacity, s: MyBoxServerSlot): boolean {
  return (
    !cap.free &&
    !cap.expired &&
    !cap.suspended &&
    ((!s.expired && !s.suspended) || (s.suspended && s.paused))
  );
}

/** Pourcentage borné, pour les jauges. */
export function percent(part: number, total: number): number {
  return total > 0 ? Math.max(0, Math.min(100, (part / total) * 100)) : 0;
}

/** Champ requis à masquer à la saisie (mot de passe, jeton…). */
export function isSecretField(key: string): boolean {
  return /PASS|TOKEN|SECRET|KEY/i.test(key);
}
