import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
  classifyLinkNavigation,
  installExternalLinkGuard,
  isExternalNavigation,
} from './externalLinkGuard';
import { openUrl } from './openUrl';

vi.mock('./openUrl', () => ({ openUrl: vi.fn(() => Promise.resolve()) }));

const APP_ORIGIN = 'http://localhost:1420';

describe('isExternalNavigation', () => {
  it('flags a remote page, which is the one-way navigation we must stop', () => {
    expect(isExternalNavigation('https://example.com/built-site', APP_ORIGIN)).toBe(true);
  });

  it('leaves the app’s own hash routes alone', () => {
    expect(isExternalNavigation('#/settings/notifications', APP_ORIGIN)).toBe(false);
    expect(isExternalNavigation(`${APP_ORIGIN}/#/chat`, APP_ORIGIN)).toBe(false);
  });

  it('ignores non-http schemes — those are handled by their own components', () => {
    expect(isExternalNavigation('mailto:someone@example.com', APP_ORIGIN)).toBe(false);
    expect(isExternalNavigation('data:text/html,<p>hi</p>', APP_ORIGIN)).toBe(false);
    expect(isExternalNavigation('openhuman://thread/1', APP_ORIGIN)).toBe(false);
  });
});

describe('classifyLinkNavigation', () => {
  it('blocks a same-origin path that is not a hash route', () => {
    // The app routes on the hash, so `/settings` is a page load that leaves the
    // running app behind — not remote, so there is nothing to open elsewhere.
    expect(classifyLinkNavigation('/settings', APP_ORIGIN)).toBe('block');
    expect(classifyLinkNavigation(`${APP_ORIGIN}/settings`, APP_ORIGIN)).toBe('block');
  });

  it('blocks a same-origin hash URL that decorates a different document', () => {
    // `/other-page#/chat` is same-origin and carries a hash, but it still loads
    // `/other-page`. Only a hash on the document already open is a route.
    expect(classifyLinkNavigation('/other-page#/chat', APP_ORIGIN)).toBe('block');
    expect(classifyLinkNavigation(`${APP_ORIGIN}/other#/chat`, APP_ORIGIN)).toBe('block');
    expect(classifyLinkNavigation('/app.html#/chat', APP_ORIGIN, '/app.html')).toBe('ignore');

    // A hash route must match the query too: `/app.html?a=1#/chat` clicked
    // from `/app.html?b=2` reloads the document, hash or not.
    expect(classifyLinkNavigation('/app.html?a=1#/chat', APP_ORIGIN, '/app.html', '?b=2')).toBe(
      'block'
    );
    expect(classifyLinkNavigation('/app.html?a=1#/chat', APP_ORIGIN, '/app.html', '?a=1')).toBe(
      'ignore'
    );
    // Dropping an existing query is a navigation as well.
    expect(classifyLinkNavigation('/app.html#/chat', APP_ORIGIN, '/app.html', '?a=1')).toBe(
      'block'
    );
  });

  it('leaves hash routes and foreign schemes alone, and sends remote pages out', () => {
    expect(classifyLinkNavigation('#/chat', APP_ORIGIN)).toBe('ignore');
    expect(classifyLinkNavigation(`${APP_ORIGIN}/#/chat`, APP_ORIGIN)).toBe('ignore');
    expect(classifyLinkNavigation('mailto:a@b.c', APP_ORIGIN)).toBe('ignore');
    expect(classifyLinkNavigation('https://example.com', APP_ORIGIN)).toBe('external');
  });
});

describe('installExternalLinkGuard', () => {
  let teardown: () => void = () => {};

  beforeEach(() => {
    vi.mocked(openUrl).mockClear();
    document.body.innerHTML = '';
  });

  afterEach(() => teardown());

  const clickAnchor = (html: string): MouseEvent => {
    document.body.innerHTML = html;
    const anchor = document.querySelector('a') as HTMLAnchorElement;
    const event = new MouseEvent('click', { bubbles: true, cancelable: true, button: 0 });
    anchor.dispatchEvent(event);
    return event;
  };

  it('stops an external link from navigating the webview and opens it outside', () => {
    teardown = installExternalLinkGuard();

    const event = clickAnchor('<a href="https://example.com/site">my site</a>');

    expect(event.defaultPrevented).toBe(true);
    expect(openUrl).toHaveBeenCalledWith('https://example.com/site');
  });

  it('stops a same-origin page load without sending it to the OS browser', () => {
    teardown = installExternalLinkGuard();

    const event = clickAnchor('<a href="/settings">settings</a>');

    expect(event.defaultPrevented).toBe(true);
    expect(openUrl).not.toHaveBeenCalled();
  });

  it('lets in-app routes through untouched', () => {
    teardown = installExternalLinkGuard();

    const event = clickAnchor('<a href="#/chat">back to chat</a>');

    expect(event.defaultPrevented).toBe(false);
    expect(openUrl).not.toHaveBeenCalled();
    teardown();
  });

  // The chat bubble's own anchor already calls preventDefault + openUrl; if the
  // guard did not defer, one click would open two tabs.
  it('defers to a component that already handled the click itself', () => {
    teardown = installExternalLinkGuard();
    document.body.innerHTML = '<a href="https://example.com/site">handled</a>';
    const anchor = document.querySelector('a') as HTMLAnchorElement;
    anchor.addEventListener('click', e => e.preventDefault());

    anchor.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true, button: 0 }));

    expect(openUrl).not.toHaveBeenCalled();
  });

  it('stops listening after teardown', () => {
    installExternalLinkGuard()();

    const event = clickAnchor('<a href="https://example.com/site">my site</a>');

    expect(event.defaultPrevented).toBe(false);
    expect(openUrl).not.toHaveBeenCalled();
  });
});
