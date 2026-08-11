import {
  compileToAst,
  defineQueryFilters,
  type Query,
  queryStateFrom,
} from '@app/features/next-soup/filters/filter-store';
import type { SoupEntityTag } from '../normalized-cache/types';
import { makeGraphqlSoupInput } from './ast';

/** Canonical entity identity accepted by exact GraphQL Soup filters. */
export type GraphqlSoupEntityRef = {
  id: string;
  type: SoupEntityTag;
};

/** Builds an exact heterogeneous GraphQL Soup query using entity-id filters. */
export function makeGraphqlEntitySoupInput(entities: GraphqlSoupEntityRef[]) {
  const include: NonNullable<Query['include']> = {};
  for (const entity of entities) {
    switch (entity.type) {
      case 'calendarEvent':
        include.calendarEventId = [
          ...(include.calendarEventId ?? []),
          entity.id,
        ];
        break;
      case 'document':
        include.documentId = [...(include.documentId ?? []), entity.id];
        break;
      case 'project':
        include.folderIdSelf = [...(include.folderIdSelf ?? []), entity.id];
        break;
      case 'chat':
        include.chatId = [...(include.chatId ?? []), entity.id];
        break;
      case 'emailThread':
        include.threadId = [...(include.threadId ?? []), entity.id];
        break;
      case 'channel':
        include.channelId = [...(include.channelId ?? []), entity.id];
        break;
      case 'channelThread':
        include.channelThreadId = [
          ...(include.channelThreadId ?? []),
          entity.id,
        ];
        break;
      case 'call':
        include.callId = [...(include.callId ?? []), entity.id];
        break;
      case 'crmCompany':
        include.crmCompanyId = [...(include.crmCompanyId ?? []), entity.id];
        break;
      case 'foreignEntity':
        include.foreignEntityRecordId = [
          ...(include.foreignEntityRecordId ?? []),
          entity.id,
        ];
        break;
      case 'reminder':
        include.reminderId = [...(include.reminderId ?? []), entity.id];
        break;
    }
  }

  const query = defineQueryFilters({ include });
  const body = compileToAst(queryStateFrom({ ...query, emailView: 'all' }));
  return makeGraphqlSoupInput({
    params: {
      limit: Math.max(1, entities.length),
      sort_method: 'updated_at',
    },
    body,
  });
}
