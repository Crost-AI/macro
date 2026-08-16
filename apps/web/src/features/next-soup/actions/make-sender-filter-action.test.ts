import type { EntityData } from '@entity';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const PRIMARY_LINK = 'link-primary';

vi.mock('@queries/email/link', () => ({
  useNonPrimaryEmailLinkIdHeader: () => (linkId: string | undefined | null) =>
    !linkId || linkId === PRIMARY_LINK ? undefined : linkId,
}));

import { makeSenderFilterAction } from './make-sender-filter-action';

function emailEntity(
  senderEmail: string | undefined,
  linkId?: string
): EntityData {
  return {
    type: 'email',
    id: `id-${senderEmail}-${linkId}`,
    senderEmail,
    linkId,
  } as EntityData;
}

describe('makeSenderFilterAction', () => {
  let action: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    action = vi.fn(async () => {});
  });

  it('targets the inbox each email belongs to', async () => {
    const filter = makeSenderFilterAction(action);

    await filter.execute([emailEntity('spam@example.com', 'link-2')]);

    expect(action).toHaveBeenCalledWith('spam@example.com', 'link-2');
  });

  it('leaves the inbox unset for the primary inbox', async () => {
    const filter = makeSenderFilterAction(action);

    await filter.execute([emailEntity('spam@example.com', PRIMARY_LINK)]);

    expect(action).toHaveBeenCalledWith('spam@example.com', undefined);
  });

  it('handles the same sender once per inbox it appears in', async () => {
    const filter = makeSenderFilterAction(action);

    await filter.execute([
      emailEntity('spam@example.com', 'link-2'),
      emailEntity('SPAM@example.com', 'link-3'),
      emailEntity('spam@example.com', PRIMARY_LINK),
    ]);

    expect(action.mock.calls).toEqual([
      ['spam@example.com', 'link-2'],
      ['SPAM@example.com', 'link-3'],
      ['spam@example.com', undefined],
    ]);
  });

  it('collapses repeats of one sender within a single inbox', async () => {
    const filter = makeSenderFilterAction(action);

    await filter.execute([
      emailEntity('spam@example.com', 'link-2'),
      emailEntity(' SPAM@Example.com ', 'link-2'),
    ]);

    expect(action).toHaveBeenCalledTimes(1);
  });

  it('collapses repeats that both fall through to the primary inbox', async () => {
    const filter = makeSenderFilterAction(action);

    await filter.execute([
      emailEntity('spam@example.com', PRIMARY_LINK),
      emailEntity('spam@example.com', undefined),
    ]);

    expect(action).toHaveBeenCalledTimes(1);
  });

  it('skips entities without a sender', async () => {
    const filter = makeSenderFilterAction(action);

    await filter.execute([
      emailEntity(undefined, 'link-2'),
      { type: 'document', id: 'doc-1' } as EntityData,
    ]);

    expect(action).not.toHaveBeenCalled();
  });
});
