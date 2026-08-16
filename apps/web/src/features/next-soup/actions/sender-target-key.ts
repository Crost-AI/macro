/**
 * Dedup key for a sender action across a multi-select.
 *
 * Blocks and sender filters are stored per inbox, so the same address selected
 * in two inboxes is two distinct targets and both need their own request.
 * Keying on the normalized address alone silently drops the second one.
 *
 * `linkId` is the `X-Email-Link-Id` value the request will carry, so entities
 * that resolve to the same inbox — including several that all fall through to
 * the primary-inbox default — collapse to one key.
 */
export function senderTargetKey(
  linkId: string | undefined,
  senderEmail: string
): string {
  // JSON-encoded pair rather than a joined string: it keeps the two fields
  // unambiguous whatever characters an address turns out to contain.
  return JSON.stringify([linkId ?? null, senderEmail.trim().toLowerCase()]);
}
