/** File modifications: +/− badge in the row, Pierre-rendered diffs in the body. */

import type { ToolDetail } from '@service-agent-fold/generated/types';
import { diffLines } from 'diff';
import { Show } from 'solid-js';
import { DiffChanges, PierreDiff, ToolCard } from '../../ui';
import { pathsSubtitle, type ToolCallCommon } from './shared';

/** Sum added/removed lines across a call's file diffs for the +/− badge. */
function countDiffChanges(
  diffs: { oldText?: string | null; newText: string }[]
) {
  let additions = 0;
  let deletions = 0;
  for (const diff of diffs) {
    for (const change of diffLines(diff.oldText ?? '', diff.newText)) {
      if (change.added) additions += change.count ?? 0;
      if (change.removed) deletions += change.count ?? 0;
    }
  }
  return { additions, deletions };
}

export function EditToolCall(props: {
  detail: Extract<ToolDetail, { kind: 'edit' }>;
  common: ToolCallCommon;
}) {
  const changes = () => countDiffChanges(props.detail.diffs);

  return (
    <ToolCard
      title={props.common.label}
      subtitle={pathsSubtitle(props.detail.diffs.map((diff) => diff.path))}
      trailing={props.common.trailing ?? <DiffChanges {...changes()} />}
      status={props.common.status}
      muted={props.common.muted}
    >
      <Show when={props.detail.diffs.length > 0}>
        <PierreDiff diffs={props.detail.diffs} />
      </Show>
    </ToolCard>
  );
}
