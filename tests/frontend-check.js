// Frontend harness: extracts the launcher's inline script, evaluates only the
// music-chain / SFX / boot functions under a DOM + Tauri mock, and asserts
// the behaviours that matter for v0.2.1.
// Run: node tests/frontend-check.js
const fs = require('fs');
const path = require('path');

const html = fs.readFileSync(path.join(__dirname, '..', 'dist', 'index.html'), 'utf-8');
const script = html.match(/<script>([\s\S]*?)<\/script>/)[1];

// --- minimal DOM shim -------------------------------------------------------
function makeEl(id) {
  return {
    id: id || '',
    style: {},
    dataset: {},
    innerHTML: '',
    textContent: '',
    className: '',
    value: '',
    checked: false,
    classList: { _s: new Set(), add(c){ this._s.add(c); }, remove(c){ this._s.delete(c); }, contains(c){ return this._s.has(c); }, toggle(c){ this._s.has(c) ? this._s.delete(c) : this._s.add(c); } },
    addEventListener(){}, removeEventListener(){},
    appendChild(){}, setAttribute(){}, getAttribute(){ return null; },
    querySelector(){ return null; }, querySelectorAll(){ return []; },
    getBoundingClientRect(){ return { left: 0, top: 0, width: 100, height: 100 }; },
    focus(){}, click(){}, play(){ return Promise.resolve(); }, pause(){},
    getContext(){ return null; },
  };
}
const elements = {};
function getEl(id) {
  if (!elements[id]) elements[id] = makeEl(id);
  return elements[id];
}
const bodyEl = makeEl('body');
global.document = {
  getElementById: (id) => getEl(id),
  createElement: (tag) => makeEl(''),
  querySelectorAll: () => [],
  querySelector: () => null,
  body: bodyEl,
  addEventListener(){},
};
global.window = global;
global.addEventListener = () => {};
global.requestAnimationFrame = (fn) => setTimeout(fn, 16);
global.performance = { now: () => Date.now() };
global.navigator = { getGamepads: () => [] };

// --- Tauri mock -------------------------------------------------------------
const invokeLog = [];
let mockSettings = { music_enabled: '1', music_volume: '30', sfx_enabled: '1', sfx_volume: '80', skip_boot: '0', ambient_music_path: '' };
let mockSearchResults = {}; // key: source|fallback -> array of candidates
let mockKhinsider = null;   // null = not found, string = cached path
let mockCachedMusic = null; // null = none
global.window.__TAURI__ = {
  core: {
    invoke: (cmd, args) => {
      invokeLog.push({ cmd, args });
      switch (cmd) {
        case 'get_setting': return Promise.resolve(mockSettings[args.key] ?? null);
        case 'set_setting': mockSettings[args.key] = args.value; return Promise.resolve(null);
        case 'ensure_ytdlp': return Promise.resolve(null);
        case 'get_game_music': return Promise.resolve(mockCachedMusic);
        case 'get_sfx_dir': return Promise.resolve('C:\\Users\\test\\AppData\\Roaming\\com.sonic.launcher\\sfx');
        case 'search_theme_music': {
          const key = (args.source || 'youtube') + '|' + (args.fallback ? '1' : '0');
          const list = mockSearchResults[key] || [];
          return Promise.resolve(list);
        }
        case 'download_theme_music': return Promise.resolve('C:\\music\\theme.m4a');
        case 'download_theme_music_url': return Promise.resolve('C:\\music\\theme.m4a');
        case 'download_khinsider_theme': return Promise.resolve(mockKhinsider);
        case 'get_app_version': return Promise.resolve('0.2.1');
        default: return Promise.resolve(null);
      }
    },
    convertFileSrc: (p) => 'asset://localhost/' + p.replace(/\\/g, '/'),
  },
};
// fetch mock: bundled sfx files exist as mp3; app-data files do not (unless flagged)
let appDataSfx = new Set();
global.fetch = (url) => {
  const isAppData = url.includes('com.sonic.launcher');
  const stem = (url.match(/\/([^\/]+)\.(mp3|wav|ogg|m4a)$/) || [])[1];
  if (isAppData) {
    return Promise.resolve({ ok: appDataSfx.has(stem), arrayBuffer: () => Promise.resolve(new ArrayBuffer(8)) });
  }
  // bundled dist/sfx/<stem>.mp3 exists
  return Promise.resolve({ ok: url.endsWith('.mp3'), arrayBuffer: () => Promise.resolve(new ArrayBuffer(8)) });
};
// AudioContext mock
class FakeGain { constructor(){ this.gain = { value: 0, cancelScheduledValues(){}, setValueAtTime(){}, linearRampToValueAtTime(){} }; } connect(){} }
class FakeSource { constructor(){ this.buffer = null; this.loop = false; } connect(){} start(){} stop(){} }
class FakeAudioContext {
  constructor(){ this.state = 'running'; this.currentTime = 0; this.destination = {}; }
  createGain(){ return new FakeGain(); }
  createBufferSource(){ return new FakeSource(); }
  decodeAudioData(){ return Promise.resolve({ duration: 1 }); }
  resume(){ return Promise.resolve(); }
}
global.AudioContext = FakeAudioContext;
global.window.AudioContext = FakeAudioContext;

// --- extract only the functions under test (brace-balanced) -----------------
function extractFn(src, name) {
  const re = new RegExp('function\\s+' + name + '\\s*\\(');
  const m = src.match(re);
  if (!m) throw new Error('function not found: ' + name);
  let i = m.index;
  let depth = 0, started = false, j = i;
  for (; j < src.length; j++) {
    const c = src[j];
    if (c === '{') { depth++; started = true; }
    else if (c === '}') { depth--; if (started && depth === 0) { j++; break; } }
  }
  return src.slice(i, j);
}

const NEEDED_FNS = [
  'tauriCore', 'getSetting', 'themeAudioEl', 'themeOk', 'pickThemeResult',
  'tryThemeSearch', 'tryKhinsider', 'playCachedMusic', 'playThemeSrc',
  'stopThemeMusic', 'playAmbientIfSet', 'scheduleThemeMusic',
  'sfxAudioContext', 'sfxSetVolume', 'sfxPreload', 'sfxTryUrls', 'playSfx',
  'playBootSfx', 'fadeAudioParam', 'startPs5Ambient', 'stopPs5Ambient',
  'resumeHomeAudio', 'enterHome', 'spawnParticles', 'bootSeq', 'skipBoot',
  'isBootVisible', 'tileFocusedHTML', 'escHtml', 'showToast',
];
const VAR_DEFS = [];

let code = '';
for (const f of NEEDED_FNS) {
  try { code += extractFn(script, f) + '\n'; }
  catch (e) { console.log('WARN: could not extract', f, '-', e.message); }
}

// --- test scaffolding -------------------------------------------------------
const assert = require('assert');
let passed = 0, failed = 0;
function check(name, fn) {
  return Promise.resolve()
    .then(fn)
    .then(() => { passed++; console.log('  ok -', name); })
    .catch((e) => { failed++; console.log('  FAIL -', name, '::', e.message); });
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function main() {
  // Globals the extracted functions need (declared here, in main's scope,
  // so eval'd function declarations resolve them).
  var themeAudio, themeDebounce, themeAutoTried, GAMES, sel, tileEls;
  var SFX_STEMS = ['boot', 'nav', 'select', 'launch', 'back', 'cc_open', 'ambient'];
  var SFX_EXTS = ['mp3', 'wav', 'ogg', 'm4a'];
  var sfxCtx, sfxGain, sfxBuffers = {}, sfxEnabled = true, sfxVolume = 0.8;
  var ambientSrc, ambientGain, ambientPlaying = false;
  var bootToken = 0;
  var THEME_EXCLUDED = ['cover','remix','piano','1 hour','1hr','live','extended','loop','8d','slowed','reverb'];
  // Evaluate the extracted code in this context (globals above are in scope).
  eval(code);

  // Seed the game list the chain needs.
  GAMES = [{ key: 'g1', row: 'Hollow Knight', source: 'steam', appid: '367520', exePath: '' }];
  sel = 0;
  themeAudio = makeEl('audio');
  themeAudio.volume = 0.3;
  tileEls = [makeEl('store'), makeEl('tile0')];

  console.log('Music chain order:');
  await check('YouTube theme -> OST -> KHInsider -> SoundCloud -> ambient', async () => {
    invokeLog.length = 0;
    mockCachedMusic = null;
    mockSearchResults = {};
    mockKhinsider = null;
    mockSettings.ambient_music_path = '';
    themeAutoTried = {};
    scheduleThemeMusic();
    await sleep(900);
    const cmds = invokeLog.map((c) => c.cmd + ':' + JSON.stringify(c.args || {}));
    const seq = invokeLog.map((c) => c.cmd);
    // ensure_ytdlp first, then three search_theme_music calls, then khinsider, then soundcloud search
    assert(seq.includes('ensure_ytdlp'), 'missing ensure_ytdlp: ' + seq.join(','));
    const searches = invokeLog.filter((c) => c.cmd === 'search_theme_music');
    assert.strictEqual(searches.length, 3, 'expected 3 search calls, got ' + searches.length + ' :: ' + cmds.join(' | '));
    assert.strictEqual(searches[0].args.source, 'youtube');
    assert.strictEqual(searches[0].args.fallback, false);
    assert.strictEqual(searches[1].args.source, 'youtube');
    assert.strictEqual(searches[1].args.fallback, true);
    assert.strictEqual(searches[2].args.source, 'soundcloud');
    const khi = invokeLog.find((c) => c.cmd === 'download_khinsider_theme');
    assert(khi, 'missing download_khinsider_theme');
    // KHInsider must run after the YouTube OST search and before the SoundCloud search
    const idx = (cmd) => invokeLog.findIndex((c) => c.cmd === cmd);
    assert(idx('download_khinsider_theme') > invokeLog.findIndex((c) => c.cmd === 'search_theme_music' && c.args.fallback === true), 'khinsider before youtube OST');
    assert(idx('download_khinsider_theme') < invokeLog.findIndex((c) => c.cmd === 'search_theme_music' && c.args.source === 'soundcloud'), 'khinsider after soundcloud');
  });

  await check('KHInsider hit stops the chain (no SoundCloud call)', async () => {
    invokeLog.length = 0;
    mockCachedMusic = null;
    mockSearchResults = {};
    mockKhinsider = 'C:\\music\\hollow-knight.mp3';
    themeAutoTried = {};
    scheduleThemeMusic();
    await sleep(900);
    const searches = invokeLog.filter((c) => c.cmd === 'search_theme_music');
    assert.strictEqual(searches.length, 2, 'expected 2 search calls (youtube only), got ' + searches.length);
    assert(!invokeLog.some((c) => c.cmd === 'search_theme_music' && c.args.source === 'soundcloud'), 'soundcloud should not run after a khinsider hit');
  });

  await check('SoundCloud theme query uses the same 45-480s filter', async () => {
    const ok45 = themeOk({ duration_secs: 45, title: 'Game Theme' });
    const ok480 = themeOk({ duration_secs: 480, title: 'Game Theme' });
    const bad44 = themeOk({ duration_secs: 44, title: 'Game Theme' });
    const bad481 = themeOk({ duration_secs: 481, title: 'Game Theme' });
    assert(ok45 && ok480 && !bad44 && !bad481, '45-480s filter wrong');
  });

  console.log('Boot entrance:');
  await check('enterHome adds .home-entering and removes it after ~1100ms', async () => {
    document.body.classList.remove('home-entering');
    enterHome();
    assert(document.body.classList.contains('home-entering'), 'class not added');
    await sleep(1250);
    assert(!document.body.classList.contains('home-entering'), 'class not removed');
  });
  await check('skipBoot hides the boot and runs the entrance', async () => {
    const b = getEl('boot');
    b.style.display = 'flex';
    skipBoot();
    assert.strictEqual(b.style.display, 'none', 'boot not hidden');
    assert(document.body.classList.contains('home-entering'), 'entrance not started on skip');
    await sleep(1250);
  });

  console.log('SFX engine:');
  await check('sfxPreload resolves app-data first, then bundled dist/sfx/', async () => {
    sfxBuffers = {};
    appDataSfx = new Set(['nav']); // user overrode nav in app-data
    sfxPreload();
    await sleep(200);
    assert(sfxBuffers['nav'], 'nav not loaded');
    assert(sfxBuffers['boot'], 'boot (bundled) not loaded');
    assert(sfxBuffers['ambient'], 'ambient (bundled) not loaded');
    assert(Object.keys(sfxBuffers).length === 7, 'expected 7 stems, got ' + Object.keys(sfxBuffers).length);
  });
  await check('playSfx is silent when a stem is missing (no throw)', async () => {
    sfxBuffers = {};
    playSfx('nav');
    playSfx('boot');
  });
  await check('PS5 ambient starts only when no theme, stops when a theme plays', async () => {
    sfxBuffers = { ambient: { duration: 120 } };
    ambientPlaying = false;
    startPs5Ambient();
    assert(ambientPlaying, 'ambient did not start');
    stopPs5Ambient(0.05);
    await sleep(200);
    assert(!ambientPlaying, 'ambient did not stop');
  });

  console.log(`\n${passed} passed, ${failed} failed`);
  process.exit(failed ? 1 : 0);
}

main().catch((e) => { console.error(e); process.exit(1); });
