import { FavoriteIcon } from '@app/features/favorites/FavoriteIcon';
import { globalSplitManager } from '@app/signal/splitLayout';
import {
  favoriteSplitContent,
  useFavoriteDisplayName,
  useFavoriteDmRecipientId,
} from '@app/util/favorites';
import { useSplitLayout } from '@components/app/split-layout/layout';
import {
  ContextMenuContent,
  MenuItem,
  MenuSeparator,
} from '@core/component/ContextMenu';
import { ContextMenu } from '@kobalte/core/context-menu';
import HeartIcon from '@phosphor/heart.svg';
import {
  useFavoritesData,
  useRemoveFavoriteMutation,
} from '@queries/favorites/favorites';
import type { Favorite } from '@service-storage/generated/schemas/favorite';
import { Button, NavRow } from '@ui';
import { createEffect, createSignal, For, onCleanup, Show } from 'solid-js';

const FavoritePopoverRow = (props: {
  favorite: Favorite;
  onContextMenuOpenChange: (open: boolean) => void;
  /** Close the favorites popover (after an action navigates away). */
  close: () => void;
}) => {
  const layout = useSplitLayout();
  const removeMutation = useRemoveFavoriteMutation();
  const displayName = useFavoriteDisplayName(props.favorite);
  const dmRecipientId = useFavoriteDmRecipientId(props.favorite);

  const openFavorite = (preferNewSplit: boolean) => {
    layout.openWithSplit(favoriteSplitContent(props.favorite), {
      referredFrom: 'sidebar',
      activate: true,
      preferNewSplit,
    });
    globalSplitManager()?.returnFocus();
    props.close();
  };

  const canOpenInNewSplit = () =>
    globalSplitManager()?.canAppendSplit() ?? false;

  return (
    <ContextMenu onOpenChange={props.onContextMenuOpenChange}>
      <ContextMenu.Trigger class="w-full">
        <NavRow class="h-9" fullWidth onClick={() => openFavorite(false)}>
          <div class="flex size-5 shrink-0 items-center justify-center">
            <FavoriteIcon
              favorite={props.favorite}
              class={dmRecipientId() ? 'size-[18px]' : 'size-3.5'}
            />
          </div>
          <span class="min-w-0 flex-1 truncate text-left text-ink">
            {displayName()}
          </span>
        </NavRow>
      </ContextMenu.Trigger>
      <ContextMenu.Portal>
        <ContextMenuContent class="text-xs text-ink-muted">
          <MenuItem
            text="Open in new split"
            onClick={() => openFavorite(true)}
            disabled={!canOpenInNewSplit()}
          />
          <MenuItem
            text="Open in current split"
            onClick={() => openFavorite(false)}
          />
          <MenuSeparator />
          <MenuItem
            text="Remove from favorites"
            onClick={() =>
              removeMutation.mutate({
                entityType: props.favorite.entityType,
                entityId: props.favorite.entityId,
              })
            }
          />
        </ContextMenuContent>
      </ContextMenu.Portal>
    </ContextMenu>
  );
};

/**
 * The icon rail's Favorites menu: a heart button toggling a plain anchored
 * popover of favorite rows. Deliberately NOT a Kobalte menu: rows carry their
 * own right-click ContextMenu, and menu-item semantics (roving highlight,
 * dismissable layers) fight nested context menus — a plain panel below the
 * context menu's z-modal layer behaves like the old sidebar rows did.
 */
export const RailFavoritesMenu = (props: {
  class?: string;
  /** Extra tooltip suppression (e.g. while any rail context menu is open). */
  tooltipDisabled?: () => boolean;
}) => {
  // Non-suspending accessor: a pending or failed favorites query must not
  // take the rail down; the panel just shows its empty state until loaded.
  const favoritesData = useFavoritesData();
  const favorites = () => favoritesData()?.favorites ?? [];

  const [open, setOpen] = createSignal(false);
  const [contextMenuOpen, setContextMenuOpen] = createSignal(false);
  let rootEl: HTMLDivElement | undefined;

  // Manual outside-dismiss (the panel is not a Kobalte layer): a row's
  // portaled context menu counts as "outside" the panel DOM, so dismissal is
  // suspended while one is open — Kobalte owns that menu's own dismissal.
  createEffect(() => {
    if (!open()) return;
    const onPointerDown = (event: PointerEvent) => {
      if (contextMenuOpen()) return;
      if (rootEl?.contains(event.target as Node)) return;
      setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !contextMenuOpen()) setOpen(false);
    };
    document.addEventListener('pointerdown', onPointerDown, true);
    document.addEventListener('keydown', onKeyDown);
    onCleanup(() => {
      document.removeEventListener('pointerdown', onPointerDown, true);
      document.removeEventListener('keydown', onKeyDown);
    });
  });

  return (
    <div class="relative shrink-0" ref={rootEl}>
      <Button
        variant="ghost"
        class={props.class}
        label="Favorites"
        tooltipPlacement="right"
        tooltipDisabled={
          open() || contextMenuOpen() || props.tooltipDisabled?.()
        }
        onMouseDown={(e: MouseEvent) => {
          if (e.button !== 0) return;
          e.preventDefault();
        }}
        onClick={() => setOpen(!open())}
      >
        <HeartIcon />
      </Button>
      <Show when={open()}>
        <div class="absolute bottom-0 left-full z-float ml-2 flex max-h-96 w-64 flex-col overflow-y-auto rounded-xl border border-edge bg-surface p-1.5 shadow-menu">
          <Show
            when={favorites().length > 0}
            fallback={
              <div class="px-2.5 py-2 text-sm text-ink-muted">
                No favorites yet
              </div>
            }
          >
            <For each={favorites()}>
              {(favorite) => (
                <FavoritePopoverRow
                  favorite={favorite}
                  onContextMenuOpenChange={setContextMenuOpen}
                  close={() => setOpen(false)}
                />
              )}
            </For>
          </Show>
        </div>
      </Show>
    </div>
  );
};
