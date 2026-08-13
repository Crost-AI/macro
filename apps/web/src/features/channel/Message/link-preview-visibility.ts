import { makePersisted } from '@solid-primitives/storage';
import { createSignal } from 'solid-js';

/**
 * Global "show link previews" preference (Settings → Appearance → Interface).
 * Persisted to localStorage so the preference survives reloads.
 */
export const [showLinkPreviews, setShowLinkPreviews] = makePersisted(
  createSignal<boolean>(true),
  { name: 'channel.showLinkPreviews' }
);

/** Cap so a long-lived client can't grow the hidden list unboundedly. */
const MAX_HIDDEN_ENTRIES = 500;

/** Newest-last `messageId|url` keys of previews the user hid on this client. */
const [hiddenPreviews, setHiddenPreviews] = makePersisted(
  createSignal<string[]>([]),
  { name: 'channel.hiddenLinkPreviews' }
);

function hiddenKey(messageId: string, url: string): string {
  return `${messageId}|${url}`;
}

/** Whether the user hid this message's preview of `url` (reactive). */
export function isLinkPreviewHidden(messageId: string, url: string): boolean {
  return hiddenPreviews().includes(hiddenKey(messageId, url));
}

/**
 * Hides one link preview on one message, persisted per client. Hiding is
 * local-only: unlike Slack's sender-side "remove preview" it does not affect
 * what other participants see.
 */
export function hideLinkPreview(messageId: string, url: string): void {
  const key = hiddenKey(messageId, url);
  setHiddenPreviews((prev) =>
    [...prev.filter((entry) => entry !== key), key].slice(-MAX_HIDDEN_ENTRIES)
  );
}

/** Undo a local hide (rollback when the server-side removal fails). */
export function unhideLinkPreview(messageId: string, url: string): void {
  const key = hiddenKey(messageId, url);
  setHiddenPreviews((prev) => prev.filter((entry) => entry !== key));
}
