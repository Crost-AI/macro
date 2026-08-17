import { ENABLE_GRAPHQL_SOUP } from '@core/constant/featureFlags';
import { throwOnErr } from '@core/util/result';
import type { UnifiedNotification } from '@notifications/types';
import { notificationServiceClient } from '../service-notification/client';
import type { NotificationItemRefRequest } from '../service-notification/generated/schemas/notificationItemRefRequest';
import type { NotificationItemType as RestNotificationItemType } from '../service-notification/generated/schemas/notificationItemType';
import {
  type NotificationItemType as GraphqlNotificationItemType,
  type NotificationItemUpdateOperation,
  type NotificationUpdateOperation,
  UpdateItemNotificationsDocument,
  type UpdateItemNotificationsMutation,
} from './graphql/generated/graphql';
import { getGraphqlSoupClient, mapGraphqlNotification } from './graphql-soup';
import {
  executeGraphqlUpdateNotifications,
  type GraphqlUpdateNotificationsArgs,
  type GraphqlUpdateNotificationsResult,
} from './graphql-update-notifications';

export type { NotificationItemUpdateOperation, NotificationUpdateOperation };

/** Item reference accepted by transport-neutral item-scoped notification mutations. */
export type NotificationItemRef = NotificationItemRefRequest;

/** Arguments for updating every active notification matching the supplied items. */
export type UpdateItemNotificationsArgs = {
  items: NotificationItemRef[];
  operation: NotificationItemUpdateOperation;
};

const GRAPHQL_ITEM_TYPES: Record<
  RestNotificationItemType,
  GraphqlNotificationItemType
> = {
  email: 'EMAIL',
  message: 'MESSAGE',
  channel: 'CHANNEL',
  document: 'DOCUMENT',
  project: 'PROJECT',
  chat: 'CHAT',
  call: 'CALL',
  task: 'TASK',
  github: 'GITHUB',
  reminder: 'REMINDER',
  calendar: 'CALENDAR',
};

/** Update user-owned notification statuses through the configured transport. */
export async function updateNotifications(
  args: GraphqlUpdateNotificationsArgs
): Promise<GraphqlUpdateNotificationsResult> {
  if (!ENABLE_GRAPHQL_SOUP()) {
    const request = { notificationIds: args.notificationIds };
    switch (args.operation) {
      case 'MARK_SEEN':
        await throwOnErr(
          async () =>
            await notificationServiceClient.bulkMarkNotificationAsSeen(request)
        );
        break;
      case 'MARK_DONE':
        await throwOnErr(
          async () =>
            await notificationServiceClient.bulkMarkNotificationAsDone(request)
        );
        break;
      case 'MARK_UNDONE':
        await throwOnErr(
          async () =>
            await notificationServiceClient.bulkMarkNotificationAsUndone(
              request
            )
        );
        break;
    }
    return [];
  }

  const result = await executeGraphqlUpdateNotifications(
    getGraphqlSoupClient(),
    args
  );
  if (result.error) throw result.error;
  if (!result.data) {
    throw new Error('updateNotifications mutation returned no data');
  }
  return result.data.updateNotifications;
}

/** Update all active notifications matching items through the configured transport. */
export async function updateItemNotifications(
  args: UpdateItemNotificationsArgs
): Promise<UnifiedNotification[]> {
  if (args.items.length === 0) return [];

  if (!ENABLE_GRAPHQL_SOUP()) {
    const result = await throwOnErr(async () => {
      const request = { items: args.items };
      return args.operation === 'MARK_SEEN'
        ? await notificationServiceClient.bulkMarkItemNotificationsAsSeen(
            request
          )
        : await notificationServiceClient.bulkMarkItemNotificationsAsDone(
            request
          );
    });

    return result.map(({ owner_id: _, ...notification }) => notification);
  }

  const result = await getGraphqlSoupClient()
    .mutation(UpdateItemNotificationsDocument, {
      input: {
        items: args.items.map((item) => ({
          itemType: GRAPHQL_ITEM_TYPES[item.itemType],
          itemId: item.itemId,
        })),
        operation: args.operation,
      },
    })
    .toPromise();
  if (result.error) throw result.error;
  if (!result.data) {
    throw new Error('updateItemNotifications mutation returned no data');
  }

  return (
    result.data as UpdateItemNotificationsMutation
  ).updateItemNotifications.map(mapGraphqlNotification);
}

/** Mark every active unseen notification matching items as seen. */
export function markItemNotificationsAsSeen(
  items: NotificationItemRef[]
): Promise<UnifiedNotification[]> {
  return updateItemNotifications({ items, operation: 'MARK_SEEN' });
}

/** Mark every active notification matching items as done. */
export function markItemNotificationsAsDone(
  items: NotificationItemRef[]
): Promise<UnifiedNotification[]> {
  return updateItemNotifications({ items, operation: 'MARK_DONE' });
}
