// Starts in a hidden native browser, revealing it only if verification needs user interaction.
// Only the public release table is exported; site cookies and unrelated content stay here.
(() => {
  if (window.top !== window || location.hostname !== 'www.metal-archives.com') return;
  const config = window.__sonora_metal_feed;
  if (!config) return;
  const started = Date.now();
  const write = (name, value) => {
    document.cookie = `${name}=${value}; Path=/_sonora_metal_capture/; Secure; SameSite=Strict`;
  };
  const publish = async (result) => {
    let payload = JSON.stringify(result);
    if (typeof CompressionStream !== 'undefined') {
      const input = new Blob([payload]).stream().pipeThrough(new CompressionStream('gzip'));
      const bytes = new Uint8Array(await new Response(input).arrayBuffer());
      let binary = '';
      for (const byte of bytes) binary += String.fromCharCode(byte);
      payload = 'gzip:' + btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
    }
    const chunks = encodeURIComponent(payload).match(/.{1,3000}/g) || [];
    if (!chunks.length || chunks.length > 64) throw new Error('release feed exceeds bridge capacity');
    chunks.forEach((chunk, index) => write(`_sonora_metal_${index}`, chunk));
    write('_sonora_metal_done', String(chunks.length));
  };
  const compact = (row) => row.slice(0, 5).map((value, index) => {
    const doc = new DOMParser().parseFromString(String(value), 'text/html');
    if (index < 2) {
      const source = doc.querySelector('a[href]');
      if (!source) throw new Error('missing release link');
      const url = new URL(source.getAttribute('href'), location.origin);
      if (url.origin !== location.origin) throw new Error('unexpected release host');
      const anchor = document.createElement('a');
      anchor.setAttribute('href', url.pathname);
      anchor.textContent = source.textContent;
      return anchor.outerHTML;
    }
    const span = document.createElement('span');
    span.textContent = doc.body.textContent.trim();
    return span.innerHTML;
  });
  const tick = setInterval(async () => {
    let first;
    try { first = JSON.parse(document.body?.innerText || ''); } catch (_) {
      if (document.readyState === 'complete' &&
          (/just a moment|attention required|verification required|verify.*human|security check/i.test(document.title) ||
           document.querySelector('#challenge-running, #challenge-stage, iframe[src*="challenges.cloudflare.com"]') ||
           Date.now() - started > 8000)) {
        write('_sonora_metal_show', '1');
      }
      return;
    }
    if (!Array.isArray(first.aaData)) return;
    write('_sonora_metal_show', '0');
    clearInterval(tick);
    let requests = 1;
    let retryAfter = '';
    let status = 0;
    try {
      const total = Number(first.iTotalDisplayRecords);
      if (!Number.isSafeInteger(total) || total < 0) throw new Error('missing release total');
      const rows = first.aaData.map(compact);
      let offset = first.aaData.length;
      if (config.title && config.message) {
        const heading = document.createElement('h1');
        heading.textContent = config.title;
        const message = document.createElement('p');
        message.textContent = config.message;
        document.body.replaceChildren(heading, message);
      }
      while (offset < total) {
        if (requests >= config.pages || !offset) throw new Error('incomplete release pagination');
        await new Promise(resolve => setTimeout(resolve, 2000));
        const url = new URL(location.href);
        url.searchParams.set('iDisplayStart', String(offset));
        const controller = new AbortController();
        const timeout = setTimeout(() => controller.abort(), 15000);
        let next;
        try {
          requests++;
          const response = await fetch(url.href, {
            headers: { 'X-Requested-With': 'XMLHttpRequest' },
            signal: controller.signal,
          });
          retryAfter = response.headers.get('Retry-After') || '';
          status = response.status;
          if (!response.ok || retryAfter) throw new Error('release request refused');
          next = await response.json();
        } finally { clearTimeout(timeout); }
        if (!Array.isArray(next.aaData) || !next.aaData.length || Number(next.iTotalDisplayRecords) !== total) {
          throw new Error('incomplete release page');
        }
        rows.push(...next.aaData.map(compact));
        offset += next.aaData.length;
      }
      if (rows.length !== total) throw new Error('incomplete release feed');
      await publish({ aaData: rows, iTotalDisplayRecords: total, sonoraRequests: requests });
    } catch (_) {
      try {
        await publish({ sonoraError: 'incomplete', sonoraRequests: requests, sonoraRetryAfter: retryAfter, sonoraStatus: status });
      } catch (_) { write('_sonora_metal_done', 'error'); }
    }
  }, 500);
})();
