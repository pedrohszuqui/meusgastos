// Service worker do Meus Gastos (web): permite abrir o app sem internet.
// Estratégia "rede primeiro": com internet sempre pega a versão mais nova do site;
// sem internet usa a cópia guardada. As chamadas ao Google não passam por aqui.
const CACHE = "meusgastos-app";

self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (e) => e.waitUntil(self.clients.claim()));

self.addEventListener("fetch", (e) => {
  const req = e.request;
  if (req.method !== "GET" || new URL(req.url).origin !== self.location.origin) return;
  e.respondWith(
    fetch(req)
      .then((resp) => {
        if (resp.ok) {
          const copy = resp.clone();
          caches.open(CACHE).then((c) => c.put(req, copy));
        }
        return resp;
      })
      .catch(() => caches.match(req, { ignoreSearch: true }))
  );
});
