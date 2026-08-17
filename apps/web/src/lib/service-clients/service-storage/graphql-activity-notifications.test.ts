import { ok } from 'neverthrow';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { EntityData } from '../../../features/entity/types/entity';
import type { WithNotification } from '../../../features/entity/types/notification';
import { unreadFilterFn } from '../../../features/entity/utils/filter';
import {
  RecordChannelActivityDocument,
  UpdateItemNotificationsDocument,
  UpdateNotificationsDocument,
} from './graphql/generated/graphql';
import { recordChannelActivity } from './graphql-channel-activity';
import {
  markItemNotificationsAsDone,
  markItemNotificationsAsSeen,
  updateNotifications,
} from './graphql-notifications';

const {
  graphqlSoupEnabledMock,
  markDoneMock,
  markItemDoneMock,
  markItemSeenMock,
  markSeenMock,
  markUndoneMock,
  mutationMock,
  postActivityMock,
} = vi.hoisted(() => ({
  graphqlSoupEnabledMock: vi.fn(() => true),
  markDoneMock: vi.fn(),
  markItemDoneMock: vi.fn(),
  markItemSeenMock: vi.fn(),
  markSeenMock: vi.fn(),
  markUndoneMock: vi.fn(),
  mutationMock: vi.fn(),
  postActivityMock: vi.fn(),
}));

vi.mock('@core/constant/featureFlags', () => ({
  ENABLE_GRAPHQL_SOUP: graphqlSoupEnabledMock,
}));

vi.mock('./client', () => ({
  storageServiceClient: { postActivity: postActivityMock },
}));

vi.mock('../service-notification/client', () => ({
  notificationServiceClient: {
    bulkMarkNotificationAsDone: markDoneMock,
    bulkMarkNotificationAsSeen: markSeenMock,
    bulkMarkNotificationAsUndone: markUndoneMock,
    bulkMarkItemNotificationsAsDone: markItemDoneMock,
    bulkMarkItemNotificationsAsSeen: markItemSeenMock,
  },
}));

vi.mock('./graphql-soup', () => ({
  getGraphqlSoupClient: () => ({ mutation: mutationMock }),
  mapGraphqlNotification: (notification: {
    id: string;
    eventType: string;
    entityId: string;
    done: boolean;
    viewedAt: string | null;
  }) => ({
    id: notification.id,
    notification_event_type: notification.eventType,
    entity_id: notification.entityId,
    entity_type: 'document',
    sent: true,
    done: notification.done,
    created_at: '2025-01-01T00:00:00Z',
    viewed_at: notification.viewedAt,
    updated_at: '2025-01-01T00:00:00Z',
    deleted_at: null,
    notification_metadata: { tag: 'test', content: {} },
    sender_id: null,
  }),
}));

describe('channel activity and notification GraphQL cache separation', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    graphqlSoupEnabledMock.mockReturnValue(true);
  });

  it('marks the linked notification seen while VIEW activity leaves unread state unchanged', async () => {
    const notificationRecord = {
      __typename: 'GraphqlNotification' as const,
      id: 'notification-1',
      eventType: 'channel_message_send',
      entityType: 'CHANNEL' as const,
      entityId: 'channel-1',
      sent: true,
      done: false,
      seen: false,
      createdAt: '2025-01-01T00:00:00Z',
      viewedAt: null as string | null,
      updatedAt: '2025-01-01T00:00:00Z',
      senderId: null,
      metadata: { messageId: 'message-1' },
    };
    const channel = {
      type: 'channel',
      notifications: () => [
        {
          id: notificationRecord.id,
          entity_type: 'channel',
          entity_id: notificationRecord.entityId,
          done: notificationRecord.done,
          viewed_at: notificationRecord.viewedAt,
          notification_event_type: notificationRecord.eventType,
          notification_metadata: {
            tag: notificationRecord.eventType,
            content: notificationRecord.metadata,
          },
        },
      ],
    } as unknown as WithNotification<EntityData>;

    mutationMock.mockImplementation((document) => ({
      toPromise: async () => {
        if (document === RecordChannelActivityDocument) {
          return {
            data: {
              recordChannelActivity: {
                __typename: 'GraphqlChannelActivity',
                id: 'activity-1',
                userId: 'macro|user@example.com',
                channelId: 'channel-1',
                createdAt: '2025-01-01T00:00:00Z',
                updatedAt: '2025-01-01T00:00:01Z',
                viewedAt: '2025-01-01T00:00:01Z',
                interactedAt: null,
              },
            },
          };
        }
        if (document === UpdateNotificationsDocument) {
          const updated = {
            ...notificationRecord,
            seen: true,
            viewedAt: '2025-01-01T00:00:02Z',
            updatedAt: '2025-01-01T00:00:02Z',
          };
          // Model the normalized cache's __typename:id merge. The channel's
          // notification edge keeps pointing at this same record.
          Object.assign(notificationRecord, updated);
          return { data: { updateNotifications: [updated] } };
        }
        throw new Error('unexpected GraphQL document');
      },
    }));

    expect(unreadFilterFn(channel)).toBe(true);

    await recordChannelActivity({
      channelId: 'channel-1',
      activityType: 'view',
    });
    expect(unreadFilterFn(channel)).toBe(true);

    const updated = await updateNotifications({
      notificationIds: ['notification-1'],
      operation: 'MARK_SEEN',
    });
    expect(updated[0].__typename).toBe('GraphqlNotification');
    expect(updated[0].id).toBe('notification-1');
    expect(mutationMock).toHaveBeenLastCalledWith(
      UpdateNotificationsDocument,
      {
        input: {
          notificationIds: ['notification-1'],
          operation: 'MARK_SEEN',
        },
      },
      {
        normalizedCacheOptimistic: {
          optimisticResponse: {
            updateNotifications: [
              {
                __typename: 'GraphqlNotification',
                id: 'notification-1',
                seen: true,
                viewedAt: expect.any(String),
              },
            ],
          },
          linkPatches: [],
          revalidations: [],
        },
      }
    );
    expect(unreadFilterFn(channel)).toBe(false);
  });

  it('updates item notifications through GraphQL and maps authoritative rows', async () => {
    mutationMock.mockReturnValue({
      toPromise: async () => ({
        data: {
          updateItemNotifications: [
            {
              __typename: 'GraphqlNotification',
              id: 'notification-item-1',
              eventType: 'document_mention',
              entityType: 'DOCUMENT',
              entityId: 'doc-1',
              sent: true,
              done: true,
              seen: false,
              createdAt: '2025-01-01T00:00:00Z',
              viewedAt: null,
              updatedAt: '2025-01-01T00:00:01Z',
              senderId: null,
              metadata: {},
            },
          ],
        },
      }),
    });

    const updated = await markItemNotificationsAsDone([
      { itemType: 'document', itemId: 'doc-1' },
      { itemType: 'message', itemId: 'message-1' },
    ]);

    expect(mutationMock).toHaveBeenCalledWith(UpdateItemNotificationsDocument, {
      input: {
        items: [
          { itemType: 'DOCUMENT', itemId: 'doc-1' },
          { itemType: 'MESSAGE', itemId: 'message-1' },
        ],
        operation: 'MARK_DONE',
      },
    });
    expect(updated).toEqual([
      expect.objectContaining({
        id: 'notification-item-1',
        entity_id: 'doc-1',
        done: true,
      }),
    ]);
  });

  it('uses only REST writes while GraphQL Soup is disabled', async () => {
    graphqlSoupEnabledMock.mockReturnValue(false);
    const activity = {
      id: 'activity-1',
      user_id: 'macro|user@example.com',
      channel_id: 'channel-1',
      created_at: '2025-01-01T00:00:00Z',
      updated_at: '2025-01-01T00:00:01Z',
      viewed_at: '2025-01-01T00:00:01Z',
      interacted_at: undefined,
    };
    postActivityMock.mockResolvedValue(ok(activity));
    markSeenMock.mockResolvedValue(ok({ success: true }));
    markDoneMock.mockResolvedValue(ok({ success: true }));
    markUndoneMock.mockResolvedValue(ok({ success: true }));
    markItemSeenMock.mockResolvedValue(
      ok([
        {
          owner_id: 'macro|user@example.com',
          id: 'item-notification-1',
          notification_event_type: 'document_mention',
          entity_type: 'document',
          entity_id: 'doc-1',
          sent: true,
          done: false,
          created_at: '2025-01-01T00:00:00Z',
          viewed_at: '2025-01-01T00:00:01Z',
          updated_at: '2025-01-01T00:00:01Z',
          deleted_at: null,
          notification_metadata: {},
          sender_id: null,
        },
      ])
    );

    await expect(
      recordChannelActivity({
        channelId: 'channel-1',
        activityType: 'view',
      })
    ).resolves.toEqual(activity);
    await expect(
      updateNotifications({
        notificationIds: ['notification-1'],
        operation: 'MARK_SEEN',
      })
    ).resolves.toEqual([]);
    await updateNotifications({
      notificationIds: ['notification-1'],
      operation: 'MARK_DONE',
    });
    await updateNotifications({
      notificationIds: ['notification-1'],
      operation: 'MARK_UNDONE',
    });
    await expect(
      markItemNotificationsAsSeen([{ itemType: 'document', itemId: 'doc-1' }])
    ).resolves.toEqual([
      expect.objectContaining({
        id: 'item-notification-1',
        entity_id: 'doc-1',
      }),
    ]);

    expect(postActivityMock).toHaveBeenCalledWith({
      channel_id: 'channel-1',
      activity_type: 'view',
    });
    expect(markSeenMock).toHaveBeenCalledWith({
      notificationIds: ['notification-1'],
    });
    expect(markDoneMock).toHaveBeenCalledWith({
      notificationIds: ['notification-1'],
    });
    expect(markUndoneMock).toHaveBeenCalledWith({
      notificationIds: ['notification-1'],
    });
    expect(markItemSeenMock).toHaveBeenCalledWith({
      items: [{ itemType: 'document', itemId: 'doc-1' }],
    });
    expect(mutationMock).not.toHaveBeenCalled();
  });
});
