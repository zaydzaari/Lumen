// Run with: node tests/verification_bridge.cjs. No network or CAPTCHA interaction.
const {readFileSync} = require('node:fs');
const {runInNewContext} = require('node:vm');
const assert = require('node:assert/strict');
const source = readFileSync('src/ui/verification.js', 'utf8')
  .replace('__LUMEN_TRACK_URL__', JSON.stringify('https://open.spotify.com/track/fixture'));
function environment(origin, framed = false) {
  const messages = [];
  const window = {ipc: {postMessage: value => messages.push(JSON.parse(value))}, fetch: async response => response};
  window.top = framed ? {} : window;
  const document = {readyState:'loading', addEventListener:() => {}};
  runInNewContext(source, {window, document, location:{origin}});
  return {window, messages};
}
(async () => {
  const valid = environment('https://spotidownloader.com');
  await valid.window.fetch({url:'https://api.spotidownloader.com/session', ok:true,
    clone:() => ({json:async () => ({success:true, token:'synthetic-fixture'})})});
  await new Promise(setImmediate);
  assert.equal(valid.messages[0].kind,'token');
  assert.equal(valid.messages[0].token,'synthetic-fixture');
  for (const response of [
    {url:'https://api.spotidownloader.com.evil.example/session',ok:true},
    {url:'https://api.spotidownloader.com/session',ok:false},
    {url:'https://api.spotidownloader.com/download',ok:true},
  ]) await valid.window.fetch(response);
  assert.equal(valid.messages.filter(m => m.kind === 'token').length,1);
  assert.equal(valid.messages.filter(m => m.kind === 'failed').length,1);
  for (const env of [environment('https://evil.example'), environment('https://spotidownloader.com',true)]) {
    await env.window.fetch({url:'https://api.spotidownloader.com/session',ok:true});
    assert.equal(env.messages.length,0);
  }
  console.log('Verification bridge: provider session capture and frame/origin isolation passed.');
})().catch(error => { console.error(error); process.exitCode = 1; });
