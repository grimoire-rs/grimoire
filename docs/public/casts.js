/**
 * C-024 — cast embedding.
 *
 * Every `<div data-cast="/casts/<name>.cast">` in a page body becomes an
 * asciinema player, constructed on first intersection and never autoplayed.
 *
 * The 181 KB player bundle is injected from here rather than from Starlight's
 * `head` array, so the twenty-one pages that carry no embed never download it.
 * The vendored bundle is a classic script assigning a `AsciinemaPlayer` global,
 * not an ES module, so a script element is the only entry point it has.
 */
(() => {
  const embeds = document.querySelectorAll('[data-cast]');
  if (embeds.length === 0) return;

  const PLAYER_SRC = '/asciinema-player.min.js';
  let pending;
  const loadPlayer = () =>
    (pending ??= new Promise((resolve, reject) => {
      const script = document.createElement('script');
      script.src = PLAYER_SRC;
      script.onload = () => resolve(window.AsciinemaPlayer);
      script.onerror = () => reject(new Error(`cannot load ${PLAYER_SRC}`));
      document.head.append(script);
    }));

  // ponytail: no retry and no error UI. The `<noscript>` link beside every
  // embed is already the fallback path to the raw recording, and a failed
  // fetch of a same-origin static file means the page itself did not load.
  const mount = (el) =>
    loadPlayer().then((player) =>
      player.create(el.dataset.cast, el, {
        idleTimeLimit: 2,
        fit: 'width',
        poster: el.dataset.castPoster,
        autoPlay: false,
      }),
    );

  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        observer.unobserve(entry.target);
        mount(entry.target);
      }
    },
    // Start the bundle fetch just before the embed reaches the viewport, so
    // the terminal is painted by the time the reader gets to it.
    { rootMargin: '200px' },
  );

  for (const el of embeds) observer.observe(el);
})();
