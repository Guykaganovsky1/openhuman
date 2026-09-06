import { openUrl } from './openUrl';

/**
 * True when `href` would take the main webview away from the app itself.
 *
 * The desktop shell is a single webview with no browser chrome — no back
 * button, no address bar — so any top-level navigation to a remote page is
 * one-way: the chat is gone until the app is restarted. Only the app's own
 * origin (its hash routes, `#/chat`, `#/settings/...`) is safe to follow.
 *
 * `about:`/`blob:`/`data:` and in-page anchors are left alone: they are not
 * remote pages, and a `data:` preview is a deliberate in-app render.
 */
export type LinkNavigation = 'ignore' | 'block' | 'external';

/**
 * What a click on `href` would do to the main webview.
 *
 * - `external` — a remote page. Hand it to the OS browser.
 * - `block` — the app's own origin but NOT a hash route (`/settings`, or
 *   `/other-page#/chat`, whose hash decorates a different document).
 *   This app routes on the hash, so such a URL is a real page load: the webview
 *   leaves the running app and there is no chrome to come back with. It is not
 *   a remote page either, so handing it to the OS browser would be wrong —
 *   preventing the navigation is the whole remedy.
 * - `ignore` — an in-page anchor, a hash route, or a non-http(s) scheme
 *   (`mailto:`, `data:`, `openhuman://`). None of these strand the user.
 */
export function classifyLinkNavigation(
  href: string,
  appOrigin: string,
  appPathname = '/',
  appSearch = ''
): LinkNavigation {
  const trimmed = href.trim();
  if (!trimmed || trimmed.startsWith('#')) return 'ignore';
  let url: URL;
  try {
    url = new URL(trimmed, appOrigin);
  } catch {
    return 'ignore';
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') return 'ignore';
  if (url.origin !== appOrigin) return 'external';
  // A hash alone does not make a hash route: `/other-page#/chat` carries one and
  // still loads `/other-page`. Only a hash on the document already loaded is an
  // in-app route — and "already loaded" includes the query, since `/app?a=1`
  // clicked from `/app?b=2` is a document navigation however it is hashed.
  return url.hash && url.pathname === appPathname && url.search === appSearch ? 'ignore' : 'block';
}

/** True when `href` would take the webview to a remote page. */
export function isExternalNavigation(href: string, appOrigin: string): boolean {
  return classifyLinkNavigation(href, appOrigin) === 'external';
}

/**
 * Install a document-level, capture-phase guard that keeps a link click from
 * navigating the main webview away from the app.
 *
 * Chat bubbles already route their links through `openUrl` (see
 * `AgentMessageBubble`'s `MarkdownAnchor`), but that is one component's
 * discipline, and every other rendered anchor — tool output, a panel, raw
 * HTML inside a message — inherits the webview's default behaviour instead.
 * When one of those is clicked the shell navigates and the user is stranded
 * on the page with no way back, which is what this guard exists to prevent.
 *
 * It listens in the BUBBLE phase, deliberately. In the capture phase this
 * document-level listener would run *before* the component's own handler, so
 * a chat link would be opened once here and again by `MarkdownAnchor` — two
 * browser tabs for one click. Bubbling lets the owning component go first;
 * anything it already called `preventDefault` on is skipped, and the default
 * navigation has still not happened by the time this runs, so preventing it
 * here is not too late.
 *
 * Returns the teardown function.
 */
export function installExternalLinkGuard(doc: Document = document): () => void {
  const onClick = (event: MouseEvent) => {
    if (event.defaultPrevented || event.button !== 0) return;
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;

    const target = event.target as Element | null;
    const anchor = target?.closest?.('a[href]') as HTMLAnchorElement | null;
    if (!anchor) return;

    const href = anchor.getAttribute('href') ?? '';
    const kind = classifyLinkNavigation(
      href,
      doc.location.origin,
      doc.location.pathname,
      doc.location.search
    );
    if (kind === 'ignore') return;

    event.preventDefault();
    if (kind === 'block') {
      // Same origin, no hash: a page load that would drop the running app.
      // There is nothing to open elsewhere — stopping it is the fix.
      return;
    }
    void openUrl(anchor.href).catch(() => {
      // The OS handler refused; staying in the app beats a one-way navigation.
    });
  };

  doc.addEventListener('click', onClick);
  return () => doc.removeEventListener('click', onClick);
}
