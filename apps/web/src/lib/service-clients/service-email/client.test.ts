import { ok } from 'neverthrow';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  fetchWithToken: vi.fn(),
}));

vi.mock('@core/util/fetchWithToken', () => ({
  fetchWithToken: mocks.fetchWithToken,
}));

import { emailClient } from './client';

/** The `X-Email-Link-Id` header sent by the most recent request, if any. */
function sentLinkIdHeader(): string | undefined {
  const [, init] = mocks.fetchWithToken.mock.calls.at(-1)!;
  return init?.headers?.['X-Email-Link-Id'];
}

describe('email client inbox scoping', () => {
  beforeEach(() => {
    mocks.fetchWithToken.mockReset();
    mocks.fetchWithToken.mockResolvedValue(ok({ filter: { id: 'filter-1' } }));
  });

  const scopedCalls: Array<[string, (linkId?: string) => Promise<unknown>]> = [
    [
      'blockSender',
      (linkId) => emailClient.blockSender({ email_address: 'a@b.com' }, linkId),
    ],
    [
      'unblockSender',
      (linkId) =>
        emailClient.unblockSender({ email_address: 'a@b.com' }, linkId),
    ],
    [
      'upsertEmailFilter',
      (linkId) =>
        emailClient.upsertEmailFilter(
          { email_address: 'a@b.com', email_domain: null, is_important: true },
          linkId
        ),
    ],
    [
      'deleteEmailFilter',
      (linkId) => emailClient.deleteEmailFilter({ id: 'filter-1' }, linkId),
    ],
  ];

  it.each(scopedCalls)(
    '%s sends the link id header when given one',
    async (_name, call) => {
      await call('link-2');

      expect(sentLinkIdHeader()).toBe('link-2');
    }
  );

  it.each(scopedCalls)(
    '%s omits the link id header for the primary inbox',
    async (_name, call) => {
      await call(undefined);

      expect(sentLinkIdHeader()).toBeUndefined();
    }
  );
});
