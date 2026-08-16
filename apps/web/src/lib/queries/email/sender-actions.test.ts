import { err, ok } from 'neverthrow';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  blockSender: vi.fn(),
  unblockSender: vi.fn(),
  upsertEmailFilter: vi.fn(),
  deleteEmailFilter: vi.fn(),
  toastSuccess: vi.fn(),
  toastFailure: vi.fn(),
  telemetryError: vi.fn(),
  invalidateAllSoup: vi.fn(),
}));

vi.mock('@service-email/client', () => ({
  emailClient: {
    blockSender: mocks.blockSender,
    unblockSender: mocks.unblockSender,
    upsertEmailFilter: mocks.upsertEmailFilter,
    deleteEmailFilter: mocks.deleteEmailFilter,
  },
}));

vi.mock('@core/component/Toast/Toast', () => ({
  toast: {
    success: mocks.toastSuccess,
    failure: mocks.toastFailure,
    dismiss: vi.fn(),
  },
}));

vi.mock('@macro-inc/observability', () => ({
  Telemetry: { error: mocks.telemetryError },
}));

// Spread the original: the query client imports its normalizer setup from here.
vi.mock('../soup/normalized-cache', async (importOriginal) => ({
  ...(await importOriginal<object>()),
  invalidateAllSoup: mocks.invalidateAllSoup,
}));

import {
  blockSenderWithToast,
  markSenderNoiseWithToast,
  markSenderSignalWithToast,
  SENDER_ACTION,
} from './thread';

const SENDER = 'Sender@Example.com';
const NON_PRIMARY_LINK = 'link-2';

/** The failure the endpoints return; the toast drops it, telemetry must not. */
const AUTH_FAILURE = [
  { code: 'HTTP_ERROR', message: 'HTTP error! status: 401' },
];

/** Run the Undo action attached to the most recent success toast. */
async function undoLastSuccessToast(): Promise<void> {
  const [, options] = mocks.toastSuccess.mock.calls.at(-1)!;
  await options.actions[0].onClick();
}

/** Attributes passed alongside the error on the most recent telemetry report. */
function lastReportedAttributes(): Record<string, unknown> {
  return mocks.telemetryError.mock.calls.at(-1)![1];
}

/** The error object passed to telemetry on the most recent report. */
function lastReportedError(): { message: string; errors: unknown[] } {
  return mocks.telemetryError.mock.calls.at(-1)![0];
}

describe('sender actions', () => {
  beforeEach(() => {
    for (const mock of Object.values(mocks)) mock.mockReset();
    mocks.blockSender.mockResolvedValue(ok(undefined));
    mocks.unblockSender.mockResolvedValue(ok(undefined));
    mocks.upsertEmailFilter.mockResolvedValue(
      ok({ filter: { id: 'filter-1' } })
    );
    mocks.deleteEmailFilter.mockResolvedValue(ok(undefined));
  });

  describe('inbox targeting', () => {
    it('blocks and unblocks in the inbox the email belongs to', async () => {
      await blockSenderWithToast(SENDER, NON_PRIMARY_LINK);

      expect(mocks.blockSender).toHaveBeenCalledWith(
        { email_address: SENDER },
        NON_PRIMARY_LINK
      );

      await undoLastSuccessToast();

      expect(mocks.unblockSender).toHaveBeenCalledWith(
        { email_address: SENDER },
        NON_PRIMARY_LINK
      );
    });

    it('leaves the inbox unset when blocking in the primary inbox', async () => {
      await blockSenderWithToast(SENDER);

      expect(mocks.blockSender).toHaveBeenCalledWith(
        { email_address: SENDER },
        undefined
      );
    });

    it.each([
      ['signal', markSenderSignalWithToast, true],
      ['noise', markSenderNoiseWithToast, false],
    ] as const)(
      'files a %s filter in the inbox the email belongs to, and undoes it there',
      async (_label, markSender, isImportant) => {
        await markSender(SENDER, NON_PRIMARY_LINK);

        expect(mocks.upsertEmailFilter).toHaveBeenCalledWith(
          { email_address: SENDER, is_important: isImportant },
          NON_PRIMARY_LINK
        );

        await undoLastSuccessToast();

        expect(mocks.deleteEmailFilter).toHaveBeenCalledWith(
          { id: 'filter-1' },
          NON_PRIMARY_LINK
        );
      }
    );

    it('leaves the inbox unset when filtering in the primary inbox', async () => {
      await markSenderSignalWithToast(SENDER);

      expect(mocks.upsertEmailFilter).toHaveBeenCalledWith(
        expect.anything(),
        undefined
      );
    });
  });

  describe('failure reporting', () => {
    const failures: Array<
      [string, string, () => Promise<void>, { inbox: string }]
    > = [
      [
        'block',
        SENDER_ACTION.block,
        async () => {
          mocks.blockSender.mockResolvedValue(err(AUTH_FAILURE));
          await blockSenderWithToast(SENDER, NON_PRIMARY_LINK);
        },
        { inbox: 'explicit' },
      ],
      [
        'unblock (undo)',
        SENDER_ACTION.unblock,
        async () => {
          mocks.unblockSender.mockResolvedValue(err(AUTH_FAILURE));
          await blockSenderWithToast(SENDER, NON_PRIMARY_LINK);
          await undoLastSuccessToast();
        },
        { inbox: 'explicit' },
      ],
      [
        'signal/noise upsert',
        SENDER_ACTION.upsertFilter,
        async () => {
          mocks.upsertEmailFilter.mockResolvedValue(err(AUTH_FAILURE));
          await markSenderNoiseWithToast(SENDER, NON_PRIMARY_LINK);
        },
        { inbox: 'explicit' },
      ],
      [
        'filter delete (undo)',
        SENDER_ACTION.deleteFilter,
        async () => {
          mocks.deleteEmailFilter.mockResolvedValue(err(AUTH_FAILURE));
          await markSenderNoiseWithToast(SENDER, NON_PRIMARY_LINK);
          await undoLastSuccessToast();
        },
        { inbox: 'explicit' },
      ],
      [
        'block in the primary inbox',
        SENDER_ACTION.block,
        async () => {
          mocks.blockSender.mockResolvedValue(err(AUTH_FAILURE));
          await blockSenderWithToast(SENDER);
        },
        { inbox: 'primary-default' },
      ],
    ];

    it.each(failures)(
      'reports the original %s error instead of swallowing it',
      async (_name, action, run, expected) => {
        await run();

        expect(mocks.telemetryError).toHaveBeenCalledTimes(1);
        // The Result error survives intact, so a 401 stays attributable.
        expect(lastReportedError().errors).toEqual(AUTH_FAILURE);
        expect(lastReportedError().message).toContain('401');
        expect(lastReportedAttributes()).toMatchObject({
          action,
          errorCodes: 'HTTP_ERROR',
          inbox: expected.inbox,
        });
        // The user still sees a toast; reporting does not replace it.
        expect(mocks.toastFailure).toHaveBeenCalled();
      }
    );

    it('keeps the sender address out of the reported attributes', async () => {
      mocks.blockSender.mockResolvedValue(err(AUTH_FAILURE));

      await blockSenderWithToast(SENDER, NON_PRIMARY_LINK);

      expect(JSON.stringify(lastReportedAttributes())).not.toContain(
        'Example.com'
      );
    });

    it('reports nothing when the call succeeds', async () => {
      await blockSenderWithToast(SENDER, NON_PRIMARY_LINK);
      await undoLastSuccessToast();

      expect(mocks.telemetryError).not.toHaveBeenCalled();
    });
  });
});
