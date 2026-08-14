import {
  compileToAst,
  queryStateFrom,
} from '@app/features/next-soup/filters/filter-store';
import { getInboxSignalFilters } from '@app/features/next-soup/sidebar/soup-filter-presets';
import { useGlobalNotificationSource } from '@components/app/GlobalAppState';
import {
  filterNotDoneNotifications,
  filterValidNotifications,
  isEmailEntity,
} from '@entity';
import { stackNotifications, type UnifiedNotification } from '@notifications';
import { notificationIsRead } from '@notifications/notification-helpers';
import { useSoupAstItemsQuery } from '@queries/soup/items';
import { createMemo } from 'solid-js';

const SIGNAL_QUERY_LIMIT = 100;
const SIGNAL_QUERY_STALE_MS = 5 * 60 * 1000;

/**
 * Which rail icon an unread entity's badge count rolls up into. Only
 * Channels rolls up from the raw notification cache; Inbox and Email are
 * computed from the Signal soup query below, and the remaining rail icons
 * deliberately carry no badge.
 */
const BADGE_VIEW_BY_ENTITY_TYPE: Partial<Record<string, string>> = {
  channel: 'channels',
};

/**
 * Unread counts for the badged rail icons (inbox, mail, channels), keyed by
 * rail link id.
 *
 * Inbox and Email mirror the inbox Signal tab: the same soup query decides
 * membership (email importance, recency windows, per-type done state) and the
 * notification cache decides which of those entities are unread. Channels
 * counts unread channel entities straight from the cache.
 */
export function useRailBadgeCounts() {
  const notificationSource = useGlobalNotificationSource();

  const signalQuery = useSoupAstItemsQuery(
    () => ({
      params: { limit: SIGNAL_QUERY_LIMIT, sort_method: 'updated_at' },
      body: compileToAst(queryStateFrom(getInboxSignalFilters())),
    }),
    () => ({ staleTime: SIGNAL_QUERY_STALE_MS })
  );

  const notificationsByEntityId = createMemo(() => {
    const map = new Map<string, UnifiedNotification[]>();
    for (const notification of notificationSource.notifications()) {
      const list = map.get(notification.entity_id);
      if (list) list.push(notification);
      else map.set(notification.entity_id, [notification]);
    }
    return map;
  });

  return createMemo<Partial<Record<string, number>>>(() => {
    const counts: Record<string, number> = {};

    // Inbox rows render one row per notification STACK (a channel can carry
    // both a top-level-messages stack and a thread-replies stack), so the
    // badge counts unread stacks via the same stacking pipeline the rows use.
    for (const entity of signalQuery.data?.entities ?? []) {
      const notifications = notificationsByEntityId().get(entity.id);
      if (!notifications?.length) continue;
      const stacks = stackNotifications(
        filterNotDoneNotifications(filterValidNotifications(notifications))
      );
      const unreadStacks = stacks.filter((stack) =>
        stack.notifications.some(
          (notification) => !notificationIsRead(notification)
        )
      ).length;
      if (unreadStacks === 0) continue;
      counts.inbox = (counts.inbox ?? 0) + unreadStacks;
      if (isEmailEntity(entity))
        counts.mail = (counts.mail ?? 0) + unreadStacks;
    }

    for (const [entityKey, notifications] of Object.entries(
      notificationSource.notificationsByEntity()
    )) {
      if (!notifications?.some((n) => !notificationIsRead(n))) continue;
      const view = BADGE_VIEW_BY_ENTITY_TYPE[entityKey.split('@')[0]];
      if (view) counts[view] = (counts[view] ?? 0) + 1;
    }

    return counts;
  });
}
