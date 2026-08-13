import { getWebOrigin } from '@core/util/webOrigin';
import type { GetUnfurlResponse } from '@service-unfurl/generated/schemas/getUnfurlResponse';

/** Slack previews ~2 links per message, Discord up to 5; we split the middle. */
export const MAX_LINK_PREVIEWS = 3;

const CODE_FENCE_RE = /```[\s\S]*?(```|$)/g;
const INLINE_CODE_RE = /`[^`\n]*`/g;
const M_LINK_RE = /<m-link>([\s\S]*?)<\/m-link>/g;
const MENTION_TAG_RE = /<(m-[a-z-]+)>[\s\S]*?<\/\1>/g;
const MD_LINK_RE = /\[[^\]]*\]\((https?:\/\/[^\s)]+)\)/g;
const BARE_URL_RE = /https?:\/\/[^\s<>]+/g;

const MACRO_HOSTS = new Set(['macro.com', 'www.macro.com', 'dev.macro.com']);

/** In-app entity links render as mentions/attachments already — no unfurl. */
function isInternalAppUrl(url: URL): boolean {
  const isMacroHost =
    MACRO_HOSTS.has(url.hostname) || url.origin === getWebOrigin();
  return isMacroHost && url.pathname.startsWith('/app');
}

/**
 * Bare URLs swallow adjacent prose: trailing sentence punctuation, and a
 * closing paren when the paren isn't part of the URL itself (wiki-style
 * `..._(disambiguation)` URLs keep theirs).
 */
function trimBareUrl(url: string): string {
  let trimmed = url;
  for (;;) {
    const next = trimmed.replace(/[.,;:!?'"”’]+$/, '');
    if (next.endsWith(')')) {
      const opens = next.split('(').length;
      const closes = next.split(')').length;
      if (closes > opens) {
        trimmed = next.slice(0, -1);
        continue;
      }
    }
    if (next === trimmed) return next;
    trimmed = next;
  }
}

/** The JSON payload inside an `<m-link>` tag (the editor's link node). */
type MLinkPayload = { url?: string };

function parseMLinkUrl(payload: string): string | undefined {
  try {
    return (JSON.parse(payload) as MLinkPayload).url;
  } catch {
    return undefined;
  }
}

/**
 * Extracts the URLs in a message body eligible for a rich link preview, in
 * document order: the editor's `<m-link>` nodes, markdown-link targets, and
 * bare autolinked URLs, minus anything inside code, other mention tags, or
 * pointing back into the app. Deduped, capped at {@link MAX_LINK_PREVIEWS}.
 */
export function extractUnfurlableUrls(content: string): string[] {
  const stripped = content
    .replace(CODE_FENCE_RE, ' ')
    .replace(INLINE_CODE_RE, ' ');

  const candidates: string[] = [];
  const withoutMLinks = stripped.replace(M_LINK_RE, (_, payload: string) => {
    const url = parseMLinkUrl(payload);
    if (url) candidates.push(url);
    return ' ';
  });
  const withoutMentions = withoutMLinks.replace(MENTION_TAG_RE, ' ');
  const withoutMdLinks = withoutMentions.replace(
    MD_LINK_RE,
    (_, url: string) => {
      candidates.push(url);
      return ' ';
    }
  );
  for (const match of withoutMdLinks.match(BARE_URL_RE) ?? []) {
    candidates.push(trimBareUrl(match));
  }

  const seen = new Set<string>();
  const urls: string[] = [];
  for (const candidate of candidates) {
    let parsed: URL;
    try {
      parsed = new URL(candidate);
    } catch {
      continue;
    }
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') continue;
    if (isInternalAppUrl(parsed)) continue;
    if (seen.has(parsed.href)) continue;
    seen.add(parsed.href);
    urls.push(candidate);
    if (urls.length >= MAX_LINK_PREVIEWS) break;
  }
  return urls;
}

/**
 * A card is only worth the space when the page gave us something beyond the
 * URL itself — the server falls back to echoing the URL as the title.
 */
export function shouldRenderUnfurl(unfurl: GetUnfurlResponse): boolean {
  if (unfurl.description || unfurl.image_url) return true;
  return Boolean(unfurl.title) && unfurl.title !== unfurl.url;
}

export function extractDomain(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, '');
  } catch {
    return url;
  }
}
