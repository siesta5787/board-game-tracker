// Minimal service worker — its only job is to satisfy the browser's
// installability requirement (a registered SW with a fetch handler).
// Deliberately does no caching: this app's data changes constantly and a
// stale cache would be worse than no cache at all for a personal tracker.
self.addEventListener("install", () => {
    self.skipWaiting();
});

self.addEventListener("activate", (event) => {
    event.waitUntil(self.clients.claim());
});

self.addEventListener("fetch", (event) => {
    event.respondWith(fetch(event.request));
});
