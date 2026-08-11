import type { FileDiff } from '@service-agent-fold/generated/types';
import { diffLines } from 'diff';
import { createMemo, For, Show } from 'solid-js';
import { match } from 'ts-pattern';

/** How many unchanged lines to keep around a change before collapsing. */
const CONTEXT_LINES = 3;

type DiffLine = { kind: 'add' | 'remove' | 'context'; text: string };
type DiffRow = DiffLine | { kind: 'collapsed'; count: number };

/** Flatten `diffLines`' runs into one entry per line. */
function toLines(oldText: string, newText: string): DiffLine[] {
  const lines: DiffLine[] = [];
  for (const change of diffLines(oldText, newText)) {
    const kind = change.added ? 'add' : change.removed ? 'remove' : 'context';
    const value = change.value.endsWith('\n')
      ? change.value.slice(0, -1)
      : change.value;
    if (value.length === 0) continue;
    for (const text of value.split('\n')) lines.push({ kind, text });
  }
  return lines;
}

/** Collapse long unchanged runs to a few lines of context on each side. */
function collapseContext(lines: DiffLine[]): DiffRow[] {
  const rows: DiffRow[] = [];
  let index = 0;
  while (index < lines.length) {
    if (lines[index].kind !== 'context') {
      rows.push(lines[index]);
      index++;
      continue;
    }
    let end = index;
    while (end < lines.length && lines[end].kind === 'context') end++;
    const runLength = end - index;
    const isFirstRun = index === 0;
    const isLastRun = end === lines.length;

    if (runLength <= CONTEXT_LINES * 2) {
      rows.push(...lines.slice(index, end));
    } else {
      if (!isFirstRun) rows.push(...lines.slice(index, index + CONTEXT_LINES));
      const shown =
        (isFirstRun ? 0 : CONTEXT_LINES) + (isLastRun ? 0 : CONTEXT_LINES);
      rows.push({ kind: 'collapsed', count: runLength - shown });
      if (!isLastRun) rows.push(...lines.slice(end - CONTEXT_LINES, end));
    }
    index = end;
  }
  return rows;
}

function diffRows(diff: FileDiff): DiffRow[] {
  return collapseContext(toLines(diff.oldText ?? '', diff.newText));
}

function DiffRowView(props: { row: DiffRow }) {
  return match(props.row)
    .with({ kind: 'collapsed' }, (row) => (
      <div class="px-3 py-1 text-ink-placeholder select-none">
        ⋯ {row.count} unchanged {row.count === 1 ? 'line' : 'lines'}
      </div>
    ))
    .with({ kind: 'add' }, (row) => (
      <div class="flex gap-2 bg-success-bg px-3">
        <span class="shrink-0 text-success select-none">+</span>
        <span class="min-w-0 whitespace-pre-wrap wrap-break-word text-ink">
          {row.text}
        </span>
      </div>
    ))
    .with({ kind: 'remove' }, (row) => (
      <div class="flex gap-2 bg-failure-bg px-3">
        <span class="shrink-0 text-failure select-none">-</span>
        <span class="min-w-0 whitespace-pre-wrap wrap-break-word text-ink-muted">
          {row.text}
        </span>
      </div>
    ))
    .with({ kind: 'context' }, (row) => (
      <div class="flex gap-2 px-3">
        <span class="shrink-0 text-ink-placeholder select-none"> </span>
        <span class="min-w-0 whitespace-pre-wrap wrap-break-word text-ink-muted">
          {row.text}
        </span>
      </div>
    ))
    .exhaustive();
}

function FileDiffBody(props: { diff: FileDiff }) {
  const rows = createMemo(() => diffRows(props.diff));
  return (
    <div class="overflow-x-auto rounded bg-surface py-1 font-mono text-xs">
      <For each={rows()}>{(row) => <DiffRowView row={row} />}</For>
    </div>
  );
}

/** One or more files an edit call reported, each as a unified diff. */
export function FoldedDiff(props: { diffs: FileDiff[] }) {
  return (
    <div class="flex flex-col gap-2">
      <For each={props.diffs}>
        {(diff) => (
          <div class="flex flex-col gap-1">
            <Show when={props.diffs.length > 1}>
              <span class="truncate font-mono text-xs text-ink-extra-muted">
                {diff.path}
              </span>
            </Show>
            <FileDiffBody diff={diff} />
          </div>
        )}
      </For>
    </div>
  );
}
