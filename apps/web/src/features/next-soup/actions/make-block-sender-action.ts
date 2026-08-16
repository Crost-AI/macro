import type { EntityData } from '@entity';
import { useNonPrimaryEmailLinkIdHeader } from '@queries/email/link';
import { blockSenderWithToast } from '@queries/email/thread';
import type { SoupState } from '../create-soup-state';
import { senderTargetKey } from './sender-target-key';

export const makeBlockSenderAction = () => {
  const toHeaderLinkId = useNonPrimaryEmailLinkIdHeader();

  const canExecute = (entity: EntityData): boolean => {
    return entity.type === 'email' && !!entity.senderEmail;
  };

  const execute = async (entities: EntityData[]) => {
    // Per (inbox, sender): blocks are stored per inbox, so the same sender in
    // two inboxes needs two calls, and the same sender twice in one inbox
    // needs only one.
    const seen = new Set<string>();
    for (const entity of entities) {
      if (entity.type !== 'email' || !entity.senderEmail) continue;
      const linkId = toHeaderLinkId(entity.linkId);
      const key = senderTargetKey(linkId, entity.senderEmail);
      if (seen.has(key)) continue;
      seen.add(key);
      await blockSenderWithToast(entity.senderEmail, linkId);
    }
  };

  const executeWithSoup = async (entities: EntityData[], _soup: SoupState) => {
    await execute(entities);
  };

  return { canExecute, execute, executeWithSoup };
};
