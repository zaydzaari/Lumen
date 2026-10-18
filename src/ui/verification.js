// Runs only in the top-level SpotiDownloader page, never inside Cloudflare frames.
(() => {
  if (window.top !== window || location.origin !== 'https://spotidownloader.com') return;
  const notify = (kind, token) => window.ipc.postMessage(JSON.stringify({kind, token}));
  const nativeFetch = window.fetch;
  window.fetch = async function(...args) {
    const response = await nativeFetch.apply(this, args);
    if (response.url === 'https://api.spotidownloader.com/session') {
      if (!response.ok) notify('failed');
      else response.clone().json().then(data => {
        if (data.success === true && typeof data.token === 'string') notify('token', data.token);
        else notify('failed');
      }).catch(() => notify('failed'));
    }
    return response;
  };
  const start = () => {
    const style = document.createElement('style');
    style.id = 'lumen-verification-style';
    // Keep the provider's actual widget and its contents intact. Only the host
    // page layout is reduced; the user performs all challenge interaction.
    style.textContent = `html,body{background:#121212!important;overflow:hidden!important}
      body>*{visibility:hidden!important}
      #cf-turnstile,#cf-turnstile *{visibility:visible!important}
      #cf-turnstile{position:fixed!important;left:50%!important;top:50%!important;
        transform:translate(-50%,-50%)!important;z-index:2147483647!important}`;
    document.head.append(style);
    notify('ready');
    let attempts = 0;
    const begun = Date.now();
    const timer = setInterval(() => {
      if (document.getElementById('cf-turnstile')) { clearInterval(timer); return; }
      if (Date.now() - begun > 12000) { clearInterval(timer); notify('page-needed'); return; }
      const input = document.querySelector('input.searchInput');
      if (!input || attempts >= 3 || Date.now() - begun < (attempts + 1) * 2000) return;
      attempts++;
      // Use the site's ordinary search action to display its existing widget.
      // Never click, reset or otherwise automate the CAPTCHA itself.
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
      setter.call(input, __LUMEN_TRACK_URL__);
      input.dispatchEvent(new Event('input', {bubbles:true}));
      setTimeout(() => input.dispatchEvent(new KeyboardEvent('keydown', {key:'Enter', code:'Enter', bubbles:true})), 100);
    }, 250);
  };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start, {once:true});
  else start();
})();
