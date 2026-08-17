import { useCalendarUiFlag } from '@app/features/calendar/use-calendar-ui-flag';
import { useKeyedPersistentToasts } from '@core/component/Toast/useKeyedPersistentToasts';
import { useAddInboxFlow } from '@core/email-link';
import { useEmailLinksQuery } from '@queries/email/link';

/**
 * Surfaces a per-inbox "Enable calendar" prompt for every linked inbox whose
 * Google grant predates the calendar scope (or declined it), driven by
 * `needs_calendar_permission` from the links list. Re-running the connect flow
 * re-shows Google consent for the linked account and applies the upgraded
 * grant to the existing link, which kicks off the calendar backfill.
 *
 * Inboxes that also need a full reconnect are skipped: the reconnect prompt
 * covers them, and reconnecting records the calendar grant anyway.
 *
 * Closing the prompt sticks across reloads. Nothing is broken while calendar
 * is off, so re-asking every load is just nagging — Settings › Email keeps a
 * per-inbox "Enable calendar" button for whenever the user wants it.
 */
// TEMP(toast-restyle): a fake link so the enable-calendar toast is always
// visible while restyling, bypassing the calendar UI flag. The id is unique
// per page load because this prompt's persistKey remembers closes in
// localStorage — a stable id would vanish for good after one close. Stale
// dev ids get cleaned out of storage automatically once they stop appearing.
// Remove before merge.
const FAKE_CALENDAR_LINKS = [
  { id: `dev-calendar-${Date.now()}`, email_address: 'work@example.com' },
];

export function CalendarPermissionPrompt() {
  const calendarUiEnabled = useCalendarUiFlag();
  const linksQuery = useEmailLinksQuery();
  const startAddInbox = useAddInboxFlow();

  useKeyedPersistentToasts({
    items: () => [
      // TEMP(toast-restyle): remove before merge.
      ...FAKE_CALENDAR_LINKS,
      ...(calendarUiEnabled()
        ? (linksQuery.data?.links ?? []).filter(
            (link) => link.needs_calendar_permission && !link.needs_reauth
          )
        : []),
    ],
    key: (link) => link.id,
    persistKey: 'macro:calendar-prompt:dismissed',
    // Until the flag resolves and the links land, the empty list above means
    // "don't know yet", not "no inbox needs this" — stored dismissals must
    // survive that window.
    itemsLoaded: () => calendarUiEnabled() && linksQuery.isSuccess,
    toast: (link, dismiss) => ({
      title: 'Enable calendar',
      content(): string {
        return `Macro can now sync your Google Calendar. Grant calendar access for ${link.email_address} to turn it on.`;
      },
      actions: [
        {
          label: 'Grant access',
          onClick: () => {
            // Suppress re-prompting until the grant upgrades; on native the
            // page stays mounted while the OAuth flow runs.
            dismiss();
            startAddInbox();
          },
        },
      ],
    }),
  });

  return null;
}
