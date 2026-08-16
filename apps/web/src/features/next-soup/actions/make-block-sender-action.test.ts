import type { EntityData } from '@entity';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const PRIMARY_LINK = 'link-primary';

const mocks = vi.hoisted(() => ({
  blockSenderWithToast: vi.fn(async () => {}),
}));

vi.mock('@queries/email/link', () => ({
  useNonPrimaryEmailLinkIdHeader: () => (linkId: string | undefined | null) =>
    !linkId || linkId === PRIMARY_LINK ? undefined : linkId,
}));

vi.mock('@queries/email/thread', () => ({
  blockSenderWithToast: mocks.blockSenderWithToast,
}));

import { makeBlockSenderAction } from './make-block-sender-action';

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

describe('makeBlockSenderAction', () => {
  beforeEach(() => {
    mocks.blockSenderWithToast.mockClear();
  });

  it('blocks in the inbox the email belongs to', async () => {
    const block = makeBlockSenderAction();

    await block.execute([emailEntity('spam@example.com', 'link-2')]);

    expect(mocks.blockSenderWithToast).toHaveBeenCalledWith(
      'spam@example.com',
      'link-2'
    );
  });

  it('leaves the inbox unset for the primary inbox', async () => {
    const block = makeBlockSenderAction();

    await block.execute([emailEntity('spam@example.com', PRIMARY_LINK)]);

    expect(mocks.blockSenderWithToast).toHaveBeenCalledWith(
      'spam@example.com',
      undefined
    );
  });

  it('blocks the same sender once per inbox it appears in', async () => {
    const block = makeBlockSenderAction();

    await block.execute([
      emailEntity('spam@example.com', 'link-2'),
      emailEntity('SPAM@example.com', 'link-3'),
    ]);

    expect(mocks.blockSenderWithToast.mock.calls).toEqual([
      ['spam@example.com', 'link-2'],
      ['SPAM@example.com', 'link-3'],
    ]);
  });

  it('collapses repeats of one sender within a single inbox', async () => {
    const block = makeBlockSenderAction();

    await block.execute([
      emailEntity('spam@example.com', 'link-2'),
      emailEntity(' SPAM@Example.com ', 'link-2'),
    ]);

    expect(mocks.blockSenderWithToast).toHaveBeenCalledTimes(1);
  });

  it('skips entities without a sender', async () => {
    const block = makeBlockSenderAction();

    await block.execute([emailEntity(undefined, 'link-2')]);

    expect(mocks.blockSenderWithToast).not.toHaveBeenCalled();
  });
});
