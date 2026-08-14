import type { ListView } from '@app/constants/list-views';
import { useCalendarUiFlag } from '@app/features/calendar/use-calendar-ui-flag';
import { SidebarCreateMenu } from '@app/features/command/sidebar/sidebar-create-menu';
import { requestSearchFocus } from '@app/features/next-soup/soup-view/search-controllers';
import { useAnalytics } from '@app/lib/analytics/analytics-context';
import { globalSplitManager } from '@app/signal/splitLayout';
import { RailFavoritesMenu } from '@components/app/app-sidebar/rail-favorites-menu';
import {
  navigateToSidebarView,
  type SidebarState,
} from '@components/app/app-sidebar/sidebar';
import { useRailBadgeCounts } from '@components/app/app-sidebar/use-rail-badge-counts';
import { useSplitLayout } from '@components/app/split-layout/layout';
import {
  ContextMenuContent,
  MenuItem,
  MenuSeparator,
} from '@core/component/ContextMenu';
import { ENABLE_CALLS, ENABLE_CRM } from '@core/constant/featureFlags';
import { useSettingsState } from '@core/constant/SettingsState';
import { useSettingsTabAvailable } from '@core/constant/settingsTabsConfig';
import { registerHotkey } from '@core/hotkey/hotkeys';
import { type HotkeyToken, TOKENS } from '@core/hotkey/tokens';
import WideCalendarIcon from '@icon/wide-calendar.svg';
import { AnimatedCallIcon } from '@icon/wide-call';
import { AnimatedChannelIcon } from '@icon/wide-channel';
import { AnimatedCompanyIcon } from '@icon/wide-company';
import { AnimatedEmailIcon } from '@icon/wide-email';
import { AnimatedFileMdIcon } from '@icon/wide-fileMd';
import { AnimatedInboxIcon } from '@icon/wide-inbox';
import { AnimatedSearchIcon } from '@icon/wide-search';
import { AnimatedStarIcon } from '@icon/wide-star';
import { AnimatedTaskIcon } from '@icon/wide-task';
import { ContextMenu } from '@kobalte/core/context-menu';
import GearIcon from '@phosphor/gear.svg';
import { makePersisted } from '@solid-primitives/storage';
import { useLocation } from '@solidjs/router';
import { Button, cn } from '@ui';
import {
  type Component,
  createMemo,
  createSignal,
  For,
  type JSX,
  Show,
} from 'solid-js';
import { Dynamic } from 'solid-js/web';

interface RailLink {
  id: ListView | (string & {});
  label: string;
  icon: Component<
    JSX.SvgSVGAttributes<SVGSVGElement> | { triggerAnimation?: boolean }
  >;
  hotkeyToken: HotkeyToken;
  /** Fired directly (e.g. `/`) rather than behind the `g` leader. */
  standaloneHotkey?: boolean;
  /**
   * Per-icon svg size override. The wide-* icon set bakes different padding
   * into its viewBoxes, so a few glyphs render visually smaller at the shared
   * size and need a bump to look even with the rest.
   */
  iconClass?: string;
}

const RAIL_LINKS: readonly RailLink[] = [
  {
    id: 'search',
    label: 'Search',
    icon: AnimatedSearchIcon,
    hotkeyToken: TOKENS.sidebar.goTo.search,
    standaloneHotkey: true,
    iconClass: '[&_svg]:size-6',
  },
  {
    id: 'inbox',
    label: 'Inbox',
    icon: AnimatedInboxIcon,
    hotkeyToken: TOKENS.sidebar.goTo.inbox,
  },
  {
    id: 'mail',
    label: 'Email',
    icon: AnimatedEmailIcon,
    hotkeyToken: TOKENS.sidebar.goTo.mail,
    iconClass: '[&_svg]:size-5.5',
  },
  {
    id: 'channels',
    label: 'Channels',
    icon: AnimatedChannelIcon,
    hotkeyToken: TOKENS.sidebar.goTo.channels,
  },
  {
    id: 'documents',
    label: 'Files',
    icon: AnimatedFileMdIcon,
    hotkeyToken: TOKENS.sidebar.goTo.documents,
  },
  {
    id: 'tasks',
    label: 'Tasks',
    icon: AnimatedTaskIcon,
    hotkeyToken: TOKENS.sidebar.goTo.tasks,
  },
  {
    id: 'calendar',
    label: 'Calendar',
    icon: WideCalendarIcon,
    hotkeyToken: TOKENS.sidebar.goTo.calendar,
  },
  {
    id: 'agents',
    label: 'Agents',
    icon: AnimatedStarIcon,
    hotkeyToken: TOKENS.sidebar.goTo.agents,
  },
  {
    id: 'companies',
    label: 'Customers',
    icon: AnimatedCompanyIcon,
    hotkeyToken: TOKENS.sidebar.goTo.companies,
  },
  {
    id: 'calls',
    label: 'Calls',
    icon: AnimatedCallIcon,
    hotkeyToken: TOKENS.sidebar.goTo.calls,
  },
];

const RAIL_BUTTON_BASE =
  // p-0 kills the Button default (md: p-2), which would leave a 14px content
  // box in the fixed 32px button and flex-squeeze wider svgs out of aspect.
  'size-8 p-0 shrink-0 rounded-md flex items-center justify-center cursor-default text-ink-extra-muted not-disabled:hover:bg-ink/3 [&_svg]:size-5';
const RAIL_BUTTON_ACTIVE = 'bg-ink/6 not-disabled:hover:bg-ink/6 text-ink';

function unreadCountLabel(count: number): string {
  return count > 99 ? '99+' : String(count);
}

/**
 * Rail links the user hid via the right-click "Hide from sidebar" action.
 * Module-level so every menu's "Restore sidebar defaults" resets the same
 * persisted list.
 */
const [hiddenRailLinks, setHiddenRailLinks] = makePersisted(
  createSignal<string[]>([]),
  { name: 'rail-hidden-links' }
);

const RailLinkButton = (props: {
  link: RailLink;
  /** Unread count shown as a corner badge when > 0. */
  badgeCount?: () => number | undefined;
}) => {
  const [isHovering, setIsHovering] = createSignal(false);
  const analytics = useAnalytics();
  const layout = useSplitLayout();
  const location = useLocation();

  // Always read the manager signal live: it is undefined until the split
  // layout mounts, which happens after the sidebar.
  const isActive = () => {
    const activeContent = globalSplitManager()?.activeSplit()?.content();
    if (!activeContent) {
      const paths = location.pathname.split('/').filter(Boolean);
      return paths.includes(props.link.id);
    }
    return activeContent?.id === props.link.id;
  };

  const content = () => ({ type: 'component', id: props.link.id }) as const;
  const canOpenInNewSplit = () =>
    globalSplitManager()?.canAppendSplit() ?? false;
  const openInCurrentSplit = () => {
    layout.openWithSplit(content(), {
      allowDuplicate: true,
      mergeHistory: false,
      referredFrom: 'sidebar',
    });
    globalSplitManager()?.returnFocus();
  };
  const openInNewSplit = () => {
    const manager = globalSplitManager();
    if (!manager || !manager.canAppendSplit()) return;
    analytics.track('split_created', { from: 'sidebar' });
    manager.createNewSplit({
      content: content(),
      activate: true,
      allowDuplicate: true,
      referredFrom: 'sidebar',
    });
  };

  return (
    <ContextMenu>
      <ContextMenu.Trigger
        class="shrink-0"
        onContextMenu={(e: MouseEvent) => e.stopPropagation()}
      >
        <Button
          variant="ghost"
          data-sidebar-link={props.link.id}
          data-active={isActive() ? '' : undefined}
          class={cn(
            RAIL_BUTTON_BASE,
            isActive() && RAIL_BUTTON_ACTIVE,
            props.link.iconClass
          )}
          label={`Go to ${props.link.label}`}
          tooltipPlacement="right"
          hotkey={
            props.link.standaloneHotkey
              ? props.link.hotkeyToken
              : [TOKENS.sidebar.goToLeader, props.link.hotkeyToken]
          }
          onMouseEnter={() => setIsHovering(true)}
          onMouseLeave={() => setIsHovering(false)}
          onMouseDown={(e: MouseEvent) => {
            if (e.button !== 0) return;
            analytics.track('sidebar_click', { view: props.link.id });
            e.preventDefault();

            let currentContentHandle = globalSplitManager()?.activeSplit();
            const currentContent = currentContentHandle?.content();
            const isSameContent =
              currentContent?.type === 'component' &&
              currentContent?.id === props.link.id;

            if (!isSameContent || e.shiftKey) {
              currentContentHandle = navigateToSidebarView({
                viewId: props.link.id,
                shiftKey: e.shiftKey,
                activeSplit: currentContentHandle,
                openWithSplit: layout.openWithSplit,
                referredFrom: 'sidebar',
              });
            }

            if (props.link.id === 'search' && currentContentHandle) {
              requestSearchFocus(currentContentHandle.id);
            }

            globalSplitManager()?.returnFocus();
          }}
        >
          <Dynamic
            component={props.link.icon}
            triggerAnimation={isHovering()}
          />
          <Show when={(props.badgeCount?.() ?? 0) > 0}>
            <span class="pointer-events-none absolute -right-1 -top-1 z-10 flex h-4 min-w-4 items-center justify-center rounded-full bg-accent px-1 text-[10px] font-medium leading-none text-surface tabular-nums">
              {unreadCountLabel(props.badgeCount?.() ?? 0)}
            </span>
          </Show>
        </Button>
      </ContextMenu.Trigger>
      <ContextMenu.Portal>
        <ContextMenuContent class="text-xs text-ink-muted">
          <MenuItem
            text="Open in new split"
            onClick={openInNewSplit}
            disabled={!canOpenInNewSplit()}
          />
          <MenuItem text="Open in current split" onClick={openInCurrentSplit} />
          <MenuSeparator />
          <MenuItem
            text="Hide from sidebar"
            onClick={() =>
              setHiddenRailLinks((hidden) => [
                ...new Set([...hidden, props.link.id]),
              ])
            }
          />
          <MenuItem
            text="Restore sidebar defaults"
            onClick={() => setHiddenRailLinks([])}
            disabled={hiddenRailLinks().length === 0}
          />
        </ContextMenuContent>
      </ContextMenu.Portal>
    </ContextMenu>
  );
};

type IconRailSidebarProps = {
  sidebarState?: SidebarState;
  onOpenChange: (open: boolean) => void;
  overlayOpen?: boolean;
  onOverlayOpenChange?: (open: boolean) => void;
};

/**
 * The narrow vertical icon-bar sidebar: Create on top, one icon per soup view,
 * Settings pinned to the bottom. Clicking an icon opens that view in the
 * active split (shift-click: new split). When collapsed (`slim`), the rail
 * reappears as a hover overlay driven by the edge strip in `Layout`.
 */
export const IconRailSidebar = (props: IconRailSidebarProps) => {
  const { openSettings, selectTab, settingsOpen } = useSettingsState();
  const isTabAvailable = useSettingsTabAvailable();
  const calendarUiEnabled = useCalendarUiFlag();
  const badgeCounts = useRailBadgeCounts();

  const links = createMemo(() =>
    RAIL_LINKS.filter((link) => {
      if (hiddenRailLinks().includes(link.id)) return false;
      if (link.id === 'calendar') return calendarUiEnabled();
      if (link.id === 'calls') return ENABLE_CALLS();
      if (link.id === 'companies') return ENABLE_CRM();
      return true;
    })
  );

  const isCollapsed = () => props.sidebarState === 'slim';
  const isOverlayExpanded = () => isCollapsed() && props.overlayOpen === true;
  const isVisible = () => !isCollapsed() || isOverlayExpanded();

  registerHotkey({
    hotkey: 'cmd+.',
    scopeId: 'global',
    hotkeyToken: TOKENS.global.toggleSidebar,
    description: 'Toggle sidebar',
    runWithInputFocused: true,
    keyDownHandler: (e) => {
      e?.preventDefault();
      props.onOpenChange(isCollapsed());
      return true;
    },
  });

  const openSettingsPanel = () => {
    if (!isTabAvailable('Account')) return;
    if (settingsOpen()) {
      selectTab('Account');
      return;
    }
    openSettings('Account');
  };

  return (
    <Show when={isVisible()}>
      {/* Right-clicking rail whitespace offers the sidebar-wide reset; the
          per-icon menus stop propagation so they never stack on top of it. */}
      <ContextMenu>
        <ContextMenu.Trigger
          as="div"
          class={cn(
            'flex flex-col items-center gap-3 bg-surface px-2 pb-3 pt-3',
            isOverlayExpanded() &&
              'fixed left-0 inset-y-0 z-modal-content border-r border-edge-muted shadow-menu'
          )}
          onPointerEnter={() => {
            if (isOverlayExpanded()) props.onOverlayOpenChange?.(true);
          }}
          onPointerLeave={() => {
            if (isOverlayExpanded()) props.onOverlayOpenChange?.(false);
          }}
        >
          <SidebarCreateMenu
            isSlim={() => true}
            variant="icon"
            triggerClass="size-8 [&_svg]:size-5!"
          />
          {/* overflow-y-auto clips horizontal overflow too, so the scrollport
            carries its own padding (offset by negative margins) to keep the
            corner badges, which poke 4px outside their buttons, unclipped. */}
          <nav class="-mx-1 mt-1 flex min-h-0 flex-1 flex-col items-center gap-3 overflow-y-auto px-1 pt-1">
            <For each={links()}>
              {(link) => (
                <RailLinkButton
                  link={link}
                  badgeCount={() => badgeCounts()[link.id]}
                />
              )}
            </For>
          </nav>
          <RailFavoritesMenu
            // Phosphor glyphs fill ~80% of their viewBox; bump to match.
            class={cn(RAIL_BUTTON_BASE, '[&_svg]:size-6')}
          />
          <Button
            variant="ghost"
            class={cn(
              RAIL_BUTTON_BASE,
              settingsOpen() && RAIL_BUTTON_ACTIVE,
              // The phosphor gear glyph fills only ~81% of its viewBox; at
              // 26px the visible gear matches the wide-* glyphs (~20-21px).
              '[&_svg]:size-6.5'
            )}
            label="Settings"
            tooltipPlacement="right"
            hotkey={TOKENS.global.toggleSettings}
            onMouseDown={(e: MouseEvent) => {
              if (e.button !== 0) return;
              e.preventDefault();
            }}
            onClick={openSettingsPanel}
          >
            <GearIcon />
          </Button>
        </ContextMenu.Trigger>
        <ContextMenu.Portal>
          <ContextMenuContent class="text-xs text-ink-muted">
            <MenuItem
              text="Restore sidebar defaults"
              onClick={() => setHiddenRailLinks([])}
              disabled={hiddenRailLinks().length === 0}
            />
          </ContextMenuContent>
        </ContextMenu.Portal>
      </ContextMenu>
    </Show>
  );
};
