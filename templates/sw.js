// Service worker — templated so `{{ app_version }}` is baked into the
// script's own bytes on every request. This is what makes the browser's
// update-detection actually fire (it byte-diffs a refetched /sw.js against
// the installed one), and it means the cache names below are always in
// sync with the running binary, not a separate static file that could
// drift out of step with it.
//
// Scope: read-only offline browsing of a fixed allow-list of pages, plus
// cache-first static assets. Deliberately does NOT support offline writes
// (logging a play, editing anything) — those always require a live
// connection. See CLAUDE.md for the full design rationale.
//
// Recovery: if this file ever misbehaves in the field, a future release
// can ship a version whose fetch handler never calls respondWith (or
// whose activate handler deletes every cache outright) and every client
// self-heals the next time it's online — no manual intervention needed
// on anyone's device.
const VERSION = "{{ app_version }}";
const STATIC_CACHE = `bgtracker-static-${VERSION}`;
const PAGE_CACHE = `bgtracker-pages-${VERSION}`;
const NETWORK_TIMEOUT_MS = 8000;

// GET-only, same-origin HTML pages safe to serve from cache when the
// network is down or too slow. Anything not matched here (admin pages,
// auth, notifications, exports, photos, all POSTs) is left completely
// untouched by this service worker.
const CACHEABLE_HTML = [
    /^\/$/,
    /^\/collection\/(?!add\b)[^/]+$/,
    /^\/plays$/,
    /^\/plays\/\d+$/,
    /^\/stats$/,
    /^\/stats\/head-to-head$/,
    /^\/games$/,
    /^\/games\/\d+$/,
    /^\/users\/[^/]+$/,
    /^\/settings$/,
];

function isCacheableHtml(pathname) {
    return CACHEABLE_HTML.some((re) => re.test(pathname));
}

self.addEventListener("install", () => {
    self.skipWaiting();
});

self.addEventListener("activate", (event) => {
    event.waitUntil(
        (async () => {
            const keep = new Set([STATIC_CACHE, PAGE_CACHE]);
            const names = await caches.keys();
            await Promise.all(
                names
                    .filter((n) => n.startsWith("bgtracker-") && !keep.has(n))
                    .map((n) => caches.delete(n))
            );
            await self.clients.claim();
        })()
    );
});

async function cacheFirst(event) {
    const request = event.request;
    const cache = await caches.open(STATIC_CACHE);
    const cached = await cache.match(request);
    if (cached) return cached;
    try {
        const response = await fetch(request);
        if (response.ok) {
            event.waitUntil(cache.put(request, response.clone()));
        }
        return response;
    } catch (err) {
        return new Response("Offline and not cached", { status: 503 });
    }
}

// Reads the cached response's own Date header (set automatically by the
// server on the original fetch) and stamps the HTML with data attributes
// so the page can show an honest "showing a saved copy from X" banner
// instead of silently serving stale content. Fails safe: any problem here
// falls back to serving the cached body completely unmodified.
async function markAsStale(cachedResponse) {
    try {
        const cachedAt = cachedResponse.headers.get("date") || "";
        const html = await cachedResponse.clone().text();
        const marked = html.replace(
            /<html([^>]*)>/i,
            `<html$1 data-offline-cache="1" data-cached-at="${cachedAt}">`
        );
        if (marked === html) {
            // Regex didn't match anything — serve the original unmodified
            // rather than silently dropping the banner with no signal.
            return cachedResponse;
        }
        const headers = new Headers(cachedResponse.headers);
        headers.delete("content-length");
        headers.delete("content-encoding");
        return new Response(marked, { status: 200, headers });
    } catch (err) {
        return cachedResponse;
    }
}

async function networkFirstWithFallback(event) {
    const request = event.request;
    const cache = await caches.open(PAGE_CACHE);
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), NETWORK_TIMEOUT_MS);
    try {
        const response = await fetch(request, { signal: controller.signal });
        clearTimeout(timeout);
        // Only cache genuine successful, non-redirected responses — a
        // 302-to-/login (expired session) or a 4xx/5xx must never get
        // treated as "the page" for this URL.
        if (response.ok && !response.redirected) {
            event.waitUntil(cache.put(request, response.clone()));
        }
        return response;
    } catch (err) {
        clearTimeout(timeout);
        const cached = await cache.match(request);
        if (cached) return markAsStale(cached);
        return new Response(
            "You're offline and this page hasn't been loaded before, so there's nothing saved to show.",
            { status: 503, headers: { "Content-Type": "text/plain" } }
        );
    }
}

self.addEventListener("fetch", (event) => {
    try {
        const request = event.request;
        if (request.method !== "GET") return;

        const url = new URL(request.url);
        if (url.origin !== self.location.origin) return;

        // Any request to /login (explicit logout, or a redirect from an
        // expired/invalid session) clears the page cache first. Cache
        // Storage is origin-scoped, not session-scoped, so on a device
        // shared by two people this is what stops a stale cached page
        // from one account being handed to whoever's using the app next.
        if (url.pathname === "/login") {
            event.waitUntil(caches.delete(PAGE_CACHE));
            return;
        }

        if (url.pathname.startsWith("/static/")) {
            event.respondWith(cacheFirst(event));
        } else if (isCacheableHtml(url.pathname)) {
            event.respondWith(networkFirstWithFallback(event));
        }
    } catch (err) {
        // Any bug in this handler must degrade to plain network passthrough,
        // never a broken page.
        event.respondWith(fetch(event.request));
    }
});
