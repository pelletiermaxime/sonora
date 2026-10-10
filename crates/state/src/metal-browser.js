// Runs only in a user-requested, temporary browser. Read the site's rendered DOM;
// the site handles its own navigation, verification, and discography request.
(() => {
  if (window.top !== window || location.hostname !== 'www.metal-archives.com') return;
  let requested = false;
  let version = 0;
  const write = (name, value) => {
    document.cookie = `${name}=${value}; Path=/_sonora_metal_capture/; Secure; SameSite=Strict`;
  };
  const show = () => write('_sonora_metal_show', '1');
  const started = Date.now();
  const normalize = value => value.normalize('NFKC').toLowerCase().replace(/[^\p{L}\p{N}]/gu, '');
  // Only navigate automatically when the site's completed search proves there is one result.
  const search = () => {
    const table = document.querySelector('#searchResults');
    const links = table && [...table.querySelectorAll('tbody a[href]')].filter(link => {
      const url = new URL(link.href);
      return url.origin === location.origin && /^\/bands\/[^/]+\/\d+\/?$/.test(url.pathname);
    });
    if (!links?.length) return false;
    let total;
    try {
      const jq = window.jQuery;
      if (jq?.fn?.dataTable?.isDataTable(table)) {
        total = jq(table).DataTable().page.info().recordsDisplay;
      } else if (jq?.fn?.dataTableSettings) {
        total = jq.fn.dataTableSettings.find(settings => settings.nTable === table)?._iRecordsDisplay;
      }
    } catch (_) {}
    const summary = document.querySelector('#searchResults_info')?.textContent || '';
    total ??= Number(summary.match(/\bof\s+([\d,]+)\s+entr/i)?.[1]?.replaceAll(',', '')) || undefined;
    if (links.length === 1 && total === 1 &&
        normalize(links[0].textContent) === normalize(window.__sonora_metal_name || '')) {
      if (!requested) {
        requested = true;
        location.assign(links[0].href);
      }
    } else {
      show();
    }
    return true;
  };
  // Preview cookies never complete/close the browser; the final capture uses its own namespace.
  const publish = (html, done = false) => {
    const encoded = encodeURIComponent(html);
    const chunks = encoded.match(/.{1,3000}/g) || [];
    const marker = done ? '_sonora_metal_done' : '_sonora_metal_preview';
    const prefix = done ? '_sonora_metal_' : '_sonora_metal_preview_';
    write(marker, '');
    if (!chunks.length || chunks.length > 64) {
      write('_sonora_metal_done', 'error');
      return;
    }
    chunks.forEach((chunk, index) => write(`${prefix}${index}`, chunk));
    write(marker, done ? String(chunks.length) : `${++version}:${chunks.length}`);
  };
  const tick = setInterval(async () => {
    const info = document.querySelector('#band_info');
    if (!info) {
      if (document.readyState !== 'complete') return;
      if (/just a moment|attention required|verification required|verify.*human|security check/i.test(document.title) ||
          document.querySelector('#challenge-running, #challenge-stage, iframe[src*="challenges.cloudflare.com"]')) {
        show();
      } else if (!search() && Date.now() - started > 8000) {
        // An unexpected page must remain available for the user to inspect or navigate.
        show();
      }
      return;
    }
    const tabs = document.querySelector('#band_disco');
    const tab = tabs && [...tabs.querySelectorAll('a[href]')].find(a =>
      a.getAttribute('href') === '#band_disco_all' ||
      /\/band\/discography\/id\/\d+\/tab\/all\/?$/.test(a.getAttribute('href'))
    );
    const item = tab?.closest('li');
    const panelId = item?.getAttribute('aria-controls') || tab?.getAttribute('aria-controls');
    const complete = (panelId && document.getElementById(panelId)) ||
      document.querySelector('#band_disco_all') ||
      tabs?.querySelector(':scope > .ui-tabs-panel');
    const table = complete?.querySelector('table.discog');
    if (!table?.querySelector('tbody tr')) {
      if (!requested && tab && tabs.classList.contains('ui-tabs') &&
          !item?.classList.contains('ui-tabs-active') &&
          !item?.classList.contains('ui-tabs-selected')) {
        requested = true;
        tab.click();
      }
      return;
    }
    // Keep the identity and four discography cells, excluding unrelated page content.
    const identity = info.cloneNode(true);
    identity.querySelectorAll('script, style, img').forEach(node => node.remove());
    const result = document.createElement('table');
    result.className = 'discog';
    const body = result.createTBody();
    for (const row of table.querySelectorAll('tbody tr')) {
      const next = body.insertRow();
      for (const cell of [...row.cells].slice(0, 4)) {
        const target = next.insertCell();
        const anchor = cell.querySelector('a[href]');
        if (anchor && next.cells.length === 1) {
          const link = document.createElement('a');
          link.href = anchor.href;
          link.textContent = anchor.textContent;
          target.append(link);
        } else {
          target.textContent = cell.textContent;
        }
      }
    }
    clearInterval(tick);
    write('_sonora_metal_show', '0');
    publish(identity.outerHTML + result.outerHTML);
    // The discography only supplies years. Check recent album pages, sequentially and paced,
    // within the slots Sonora reserved before opening this browser.
    const limit = Math.max(0, Math.min(8, window.__sonora_metal_date_limit ?? 0));
    const year = new Date().getFullYear();
    const pending = [...body.rows].filter(row => {
      const value = row.cells[2]?.textContent.trim() || '';
      return /^\d{4}(?:-\d{2})?$/.test(value) && Number(value.slice(0, 4)) >= year;
    }).slice(0, limit);
    let requests = 0;
    let retryAfter = '';
    for (const row of pending) {
      const href = row.cells[0].querySelector('a')?.href;
      if (!href) continue;
      const url = new URL(href);
      if (url.origin !== location.origin || !/^\/albums\/[^/]+\/[^/]+\/\d+\/?$/.test(url.pathname)) continue;
      await new Promise(resolve => setTimeout(resolve, 2000));
      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(), 15000);
      try {
        requests++;
        const response = await fetch(url.href, { signal: controller.signal });
        retryAfter = response.headers.get('Retry-After') || '';
        if (!response.ok || retryAfter) break;
        const page = new DOMParser().parseFromString(await response.text(), 'text/html');
        const key = [...page.querySelectorAll('#album_info dl dt')]
          .find(field => field.textContent.trim() === 'Release date:');
        const field = key?.nextElementSibling;
        if (!field || field.tagName !== 'DD') break;
        row.cells[2].textContent = field.textContent.trim();
        publish(identity.outerHTML + result.outerHTML);
      } catch (_) {
        break;
      } finally {
        clearTimeout(timeout);
      }
    }
    const receipt = document.createElement('meta');
    receipt.name = 'sonora-date-requests';
    receipt.content = String(requests);
    const retry = document.createElement('meta');
    retry.name = 'sonora-date-retry-after';
    retry.content = retryAfter;
    publish(receipt.outerHTML + retry.outerHTML + identity.outerHTML + result.outerHTML, true);
  }, 500);
})();
