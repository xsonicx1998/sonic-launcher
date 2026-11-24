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
    parentElement: { style: {} },
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
let fastExitHandler = null; // set by the launch-fast-exit listener (v0.2.7)
let mockProcessRunning = false; // is_process_running result (v0.2.7)
let mockSettings = { music_enabled: '1', music_volume: '30', sfx_enabled: '1', sfx_volume: '80', skip_boot: '0', ambient_music_path: '' };
let mockSearchResults = {}; // key: source|fallback -> array of candidates
let mockKhinsider = null;   // null = not found, string = cached path
let mockCachedMusic = null; // null = none
let mockArt = null;         // null = no art, object = get_game_art result
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
        case 'pick_cover_art': return Promise.resolve('C:\\art\\cover.jpg');
        case 'set_cover_art_appid': return Promise.resolve(null);
        case 'set_cover_art_url': return Promise.resolve(null);
        case 'retry_art': return Promise.resolve('Cover art found');
        case 'get_game_art': return Promise.resolve(mockArt);
        case 'list_exe_candidates': return Promise.resolve([{ exe_path: 'C:\\Games\\Fail\\fail.exe', source: 'name' }]);
        case 'is_process_running': return Promise.resolve(mockProcessRunning);
        case 'get_app_version': return Promise.resolve('0.2.1');
        default: return Promise.resolve(null);
      }
    },
    convertFileSrc: (p) => 'asset://localhost/' + p.replace(/\\/g, '/'),
  },
  event: {
    listen: (name, fn) => {
      if (name === 'launch-fast-exit') fastExitHandler = fn;
      return Promise.resolve(() => {});
    },
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
  'isBootVisible', 'tileFocusedHTML', 'tilePlainHTML', 'tileIconHTML', 'escHtml', 'showToast',
  'toggleGameMenu', 'buildGameMenu', 'hideGameMenu',
  'openSearch', 'closeSearch', 'searchGameMatches', 'renderSearchResults',
  'pickSearchResult', 'toggleThemePlayback', 'applyRealArt',
  'baseTileFields', 'placeholderArt', 'mapUnifiedGame',
  'saveAutoFetchArt', 'copySgdbLink',
  'pickCoverArt', 'setCoverArtAppid', 'pasteCoverArtUrl', 'retryArt',
  'listenFastExit', 'showFastExitNotice', 'hideFastExitNotice', 'fastExitChangeExe',
  'openChangeExe', 'startExePoll', 'stopRunningPoll',
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
  var toastTimer = null;
  var bgSlides = [];
  var exePoll = null, runningPoll = null, currentSessionId = null;
  // Stub: applyRealArt calls render() when a hero arrives; the harness
  // only records that a render was requested.
  var renderCalls = 0;
  function render(){ renderCalls++; }
  var SFX_STEMS = ['boot', 'nav', 'select', 'launch', 'back', 'cc_open', 'ambient'];
  var SFX_EXTS = ['mp3', 'wav', 'ogg', 'm4a'];
  var sfxCtx, sfxGain, sfxBuffers = {}, sfxEnabled = true, sfxVolume = 0.8;
  var ambientSrc, ambientGain, ambientPlaying = false;
  var bootToken = 0;
  var THEME_EXCLUDED = ['cover','remix','piano','1 hour','1hr','live','extended','loop','8d','slowed','reverb'];
  // Stub: pickSearchResult delegates selection to the real selectGame;
  // the harness only records which index was picked.
  var selectGameCalls = [];
  function selectGame(i){ selectGameCalls.push(i); }
  // Stub: cover-art actions refresh the game list; the harness only
  // records that a refresh was requested.
  var refreshGamesCalls = 0;
  function refreshGames(){ refreshGamesCalls++; }
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

  console.log('Game menu / search / music bar:');
  await check('••• menu opens for a demo game with Change Theme Music + Paste music URL', async () => {
    GAMES = [{ key: 'dreams', row: 'Dreams' }]; // no source field = demo
    sel = 0;
    const m = getEl('gameMenu');
    m.style.display = 'none';
    toggleGameMenu();
    assert.strictEqual(m.style.display, 'block', 'menu did not open');
    assert(m.innerHTML.includes('Change Theme Music'), 'missing Change Theme Music: ' + m.innerHTML);
    assert(m.innerHTML.includes('Paste music URL'), 'missing Paste music URL');
    assert(m.innerHTML.includes('Demo game'), 'missing demo notice');
    toggleGameMenu();
    assert.strictEqual(m.style.display, 'none', 'menu did not close on second toggle');
  });
  await check('••• menu for real games keeps Remove options', async () => {
    GAMES = [{ key: 'm1', row: 'Doom', source: 'manual', manualId: 'x' }];
    sel = 0;
    buildGameMenu();
    const m = getEl('gameMenu');
    assert(m.innerHTML.includes('Change Theme Music'), 'missing Change Theme Music');
    assert(m.innerHTML.includes('Remove from launcher'), 'missing Remove from launcher');
    GAMES = [{ key: 's1', row: 'Hollow Knight', source: 'scan', scanId: 's-1' }];
    buildGameMenu();
    assert(m.innerHTML.includes('Remove from Library'), 'missing Remove from Library');
    GAMES = [{ key: 's2', row: 'Hollow Knight', source: 'scan', scanId: 7, exePath: 'C:\\Games\\Hollow Knight\\Hollow Knight.exe', confidence: 'low', hasOverride: false }];
    buildGameMenu();
    assert(m.innerHTML.includes('Launch file:'), 'missing Launch file line');
    assert(m.innerHTML.includes('Change Executable'), 'missing Change Executable');
    assert(m.innerHTML.includes('Not sure this is the right file'), 'missing low-confidence warning');
    GAMES = [{ key: 's3', row: 'Cyberpunk 2077', source: 'scan', scanId: 8, exePath: 'C:\\Games\\Cyberpunk 2077\\bin\\x64\\Cyberpunk2077.exe', confidence: 'high', hasOverride: false }];
    buildGameMenu();
    assert(!m.innerHTML.includes('Not sure this is the right file'), 'high-confidence game should not warn');
  });
  await check('Search opens, filters by title, picks a result', async () => {
    GAMES = [{ key: 'dreams', row: 'Dreams' }, { key: 'astro', row: "Astro's Playroom", name: "ASTRO'S PLAYROOM" }, { key: 'blood', row: 'Bloodborne' }];
    sel = 0;
    const box = getEl('searchBox');
    box.style.display = 'none';
    openSearch();
    assert.strictEqual(box.style.display, 'block', 'search box did not open');
    assert.deepStrictEqual(searchGameMatches('dre'), [0], 'dre should match Dreams only');
    assert.deepStrictEqual(searchGameMatches(''), [0, 1, 2], 'empty query should match all');
    assert.deepStrictEqual(searchGameMatches('zzz'), [], 'zzz should match nothing');
    renderSearchResults('astro');
    const res = getEl('searchResults').innerHTML;
    assert(res.includes('Astro'), 'results missing Astro: ' + res);
    assert(!res.includes('Bloodborne'), 'results should exclude Bloodborne');
    selectGameCalls.length = 0;
    pickSearchResult(2);
    assert.deepStrictEqual(selectGameCalls, [2], 'pick did not select game 2');
    assert.strictEqual(box.style.display, 'none', 'search box did not close after pick');
  });
  await check('Music bar tap pauses and resumes the theme', async () => {
    themeAudio = makeEl('audio');
    themeAudio.volume = 0.3;
    let pausedCalled = false, playedCalled = false;
    themeAudio.paused = false;
    themeAudio.src = 'asset://localhost/music.m4a';
    themeAudio.pause = () => { pausedCalled = true; themeAudio.paused = true; };
    themeAudio.play = () => { playedCalled = true; themeAudio.paused = false; return Promise.resolve(); };
    toggleThemePlayback();
    assert(pausedCalled, 'first tap did not pause');
    toggleThemePlayback();
    assert(playedCalled, 'second tap did not resume');
  });
  await check('Music bar tap with nothing loaded starts the cached theme', async () => {
    themeAudio = makeEl('audio');
    themeAudio.volume = 0.3;
    themeAudio.paused = true;
    themeAudio.src = '';
    themeAudio.play = () => Promise.resolve();
    themeAudio.pause = () => {};
    invokeLog.length = 0;
    mockCachedMusic = { path: 'C:\\music\\theme.m4a' };
    GAMES = [{ key: 'g1', row: 'Hollow Knight', source: 'steam' }];
    sel = 0;
    toggleThemePlayback();
    await sleep(150);
    assert(invokeLog.some((c) => c.cmd === 'get_game_music'), 'expected get_game_music call, got ' + invokeLog.map((c) => c.cmd).join(','));
  });
  await check('escHtml escapes single quotes too', async () => {
    assert.strictEqual(escHtml("a'b\"c<d&e"), 'a&#39;b&quot;c&lt;d&amp;e');
  });

  console.log('Cover art (v0.2.5):');
  await check('Settings panel has the auto-fetch checkbox and the SGDB link', async () => {
    assert(html.includes('id="autoFetchArt"'), 'missing autoFetchArt checkbox');
    assert(html.includes('Auto-fetch cover art from Steam'), 'missing auto-fetch label');
    assert(html.includes('steamgriddb.com/profile/preferences/api'), 'missing SGDB link text');
    assert(html.includes('copySgdbLink()'), 'missing Copy link button');
    assert(html.includes('saveAutoFetchArt(this)'), 'missing auto-fetch wiring');
  });
  await check('saveAutoFetchArt writes the auto_fetch_art setting', async () => {
    delete mockSettings.auto_fetch_art;
    saveAutoFetchArt({ checked: false });
    assert.strictEqual(mockSettings.auto_fetch_art, '0', 'uncheck should write 0');
    saveAutoFetchArt({ checked: true });
    assert.strictEqual(mockSettings.auto_fetch_art, '1', 'check should write 1');
  });
  await check('copySgdbLink shows the key URL when clipboard is unavailable', async () => {
    const toast = getEl('toast');
    toast.textContent = '';
    copySgdbLink();
    assert.strictEqual(toast.textContent, 'https://www.steamgriddb.com/profile/preferences/api', 'toast should show the URL, got: ' + toast.textContent);
  });
  await check('mapUnifiedGame maps art_failed to artFailed', async () => {
    const u = { key: 'scan-7', name: 'Hollow Knight', source: 'scan', appid: null, exe_path: 'C:\\Games\\Hollow Knight\\Hollow Knight.exe', launch_uri: null, folder: 'C:\\Games\\Hollow Knight', confidence: 'high', pick_source: 'name', has_override: false, art_failed: true };
    const g = mapUnifiedGame(u);
    assert.strictEqual(g.artFailed, true, 'artFailed should be true');
    assert.strictEqual(g.scanId, 7, 'scanId should be 7');
    const u2 = Object.assign({}, u, { art_failed: false });
    assert.strictEqual(mapUnifiedGame(u2).artFailed, false, 'artFailed should be false');
  });
  await check('••• menu shows cover-art items for real games, Retry Art only when artFailed', async () => {
    const m = getEl('gameMenu');
    GAMES = [{ key: 'scan-7', row: 'Hollow Knight', source: 'scan', scanId: 7, exePath: 'C:\\Games\\Hollow Knight\\Hollow Knight.exe', confidence: 'high', hasOverride: false, artFailed: true }];
    sel = 0;
    buildGameMenu();
    assert(m.innerHTML.includes('Set Cover Art'), 'missing Set Cover Art: ' + m.innerHTML);
    assert(m.innerHTML.includes('Set Steam AppID'), 'missing Set Steam AppID');
    assert(m.innerHTML.includes('Paste Image URL'), 'missing Paste Image URL');
    assert(m.innerHTML.includes('Retry Art'), 'missing Retry Art when artFailed');
    GAMES = [{ key: 'scan-8', row: 'Celeste', source: 'scan', scanId: 8, artFailed: false }];
    buildGameMenu();
    assert(m.innerHTML.includes('Set Cover Art'), 'missing Set Cover Art (no-fail game)');
    assert(!m.innerHTML.includes('Retry Art'), 'Retry Art should not show when artFailed is false');
    GAMES = [{ key: 'dreams', row: 'Dreams' }];
    buildGameMenu();
    assert(!m.innerHTML.includes('Set Cover Art'), 'demo games should not get cover-art items');
  });
  await check('pickCoverArt invokes pick_cover_art and refreshes', async () => {
    GAMES = [{ key: 'scan-7', row: 'Hollow Knight', source: 'scan', scanId: 7 }];
    sel = 0;
    invokeLog.length = 0;
    refreshGamesCalls = 0;
    pickCoverArt();
    await sleep(50);
    const call = invokeLog.find((c) => c.cmd === 'pick_cover_art');
    assert(call, 'missing pick_cover_art call: ' + invokeLog.map((c) => c.cmd).join(','));
    assert.strictEqual(call.args.gameKey, 'scan-7', 'wrong gameKey');
    assert.strictEqual(refreshGamesCalls, 1, 'expected a refresh after picking');
  });
  await check('setCoverArtAppid prompts and invokes with the number; rejects junk', async () => {
    GAMES = [{ key: 'scan-7', row: 'Hollow Knight', source: 'scan', scanId: 7 }];
    sel = 0;
    global.prompt = () => '440';
    invokeLog.length = 0;
    refreshGamesCalls = 0;
    setCoverArtAppid();
    await sleep(50);
    let call = invokeLog.find((c) => c.cmd === 'set_cover_art_appid');
    assert(call, 'missing set_cover_art_appid call');
    assert.strictEqual(call.args.appid, 440, 'appid should be the number 440');
    assert.strictEqual(refreshGamesCalls, 1, 'expected a refresh after setting');
    global.prompt = () => 'abc';
    invokeLog.length = 0;
    setCoverArtAppid();
    await sleep(50);
    assert(!invokeLog.some((c) => c.cmd === 'set_cover_art_appid'), 'junk input must not invoke');
    global.prompt = () => '440x';
    invokeLog.length = 0;
    setCoverArtAppid();
    await sleep(50);
    assert(!invokeLog.some((c) => c.cmd === 'set_cover_art_appid'), 'trailing junk must not invoke');
    delete global.prompt;
  });
  await check('pasteCoverArtUrl prompts and invokes set_cover_art_url', async () => {
    GAMES = [{ key: 'scan-7', row: 'Hollow Knight', source: 'scan', scanId: 7 }];
    sel = 0;
    global.prompt = () => 'https://example.com/cover.jpg';
    invokeLog.length = 0;
    refreshGamesCalls = 0;
    pasteCoverArtUrl();
    await sleep(50);
    const call = invokeLog.find((c) => c.cmd === 'set_cover_art_url');
    assert(call, 'missing set_cover_art_url call');
    assert.strictEqual(call.args.url, 'https://example.com/cover.jpg', 'wrong URL');
    assert.strictEqual(refreshGamesCalls, 1, 'expected a refresh after setting');
    delete global.prompt;
  });
  await check('retryArt invokes retry_art with the game key and name', async () => {
    GAMES = [{ key: 'scan-7', row: 'Hollow Knight', source: 'scan', scanId: 7, artFailed: true }];
    sel = 0;
    invokeLog.length = 0;
    refreshGamesCalls = 0;
    retryArt();
    await sleep(50);
    const call = invokeLog.find((c) => c.cmd === 'retry_art');
    assert(call, 'missing retry_art call');
    assert.strictEqual(call.args.gameKey, 'scan-7', 'wrong gameKey');
    assert.strictEqual(call.args.name, 'Hollow Knight', 'wrong name');
    assert.strictEqual(refreshGamesCalls, 1, 'expected a refresh after retry');
    assert.strictEqual(getEl('toast').textContent, 'Cover art found', 'toast should show the retry result');
  });

  console.log('Exe icon fallback (v0.2.6):');
  await check('mapUnifiedGame maps art_is_icon to artIsIcon', async () => {
    const u = { key: 'scan-7', name: 'Hollow Knight', source: 'scan', appid: null, exe_path: 'C:\\Games\\Hollow Knight\\Hollow Knight.exe', launch_uri: null, folder: 'C:\\Games\\Hollow Knight', confidence: 'high', pick_source: 'name', has_override: false, art_failed: false, art_is_icon: true };
    assert.strictEqual(mapUnifiedGame(u).artIsIcon, true, 'artIsIcon should be true');
    const u2 = Object.assign({}, u, { art_is_icon: false });
    assert.strictEqual(mapUnifiedGame(u2).artIsIcon, false, 'artIsIcon should be false');
    const u3 = Object.assign({}, u);
    delete u3.art_is_icon;
    assert.strictEqual(mapUnifiedGame(u3).artIsIcon, false, 'missing art_is_icon should be false');
  });
  await check('tilePlainHTML centers the icon on the gradient; normal tiles stay full-bleed', async () => {
    const g = baseTileFields('Hollow Knight', 'scan', 'scan  \u2022  PC');
    g.img = 'data:image/png;base64,AAA';
    g.artIsIcon = true;
    const iconHtml = tilePlainHTML(g);
    assert(iconHtml.includes('linear-gradient(135deg,#8B20FF,#00C8FF,#F02DCD)'), 'icon tile should have the Sonic gradient: ' + iconHtml);
    assert(iconHtml.includes('object-fit:contain'), 'icon tile should use contain: ' + iconHtml);
    assert(iconHtml.includes('width:62%'), 'icon should be ~62% of the tile: ' + iconHtml);
    assert(!iconHtml.includes('object-fit:cover'), 'icon tile must not use cover');
    const g2 = baseTileFields('Hollow Knight', 'scan', 'scan  \u2022  PC');
    g2.img = 'data:image/jpeg;base64,BBB';
    const coverHtml = tilePlainHTML(g2);
    assert(coverHtml.includes('object-fit:cover'), 'normal tile should use cover');
    assert(!coverHtml.includes('linear-gradient(135deg,#8B20FF'), 'normal tile should not get the gradient');
  });
  await check('tileFocusedHTML keeps the focus ring for icon games', async () => {
    const g = baseTileFields('Hollow Knight', 'scan', 'scan  \u2022  PC');
    g.img = 'data:image/png;base64,AAA';
    g.artIsIcon = true;
    const html = tileFocusedHTML(g);
    assert(html.includes('ps5-focus-ring'), 'focused icon tile should keep the ring');
    assert(html.includes('object-fit:contain'), 'focused icon tile should use contain');
  });
  await check('applyRealArt sets artIsIcon, keeps the gradient hero, and passes exePath', async () => {
    const bgBefore = 'data:image/svg+xml;placeholder';
    GAMES = [{ key: 'scan-7', row: 'Hollow Knight', source: 'scan', scanId: 7, exePath: 'C:\\Games\\Hollow Knight\\Hollow Knight.exe', artIsIcon: false, img: 'data:image/svg+xml;tile', bg: bgBefore }];
    sel = 0;
    tileEls = [makeEl('store'), makeEl('tile0')];
    mockArt = { tile_data_uri: 'data:image/png;base64,AAA', tile_path: 'C:\\art\\scan-7-icon.png', hero_path: null, hero_data_uri: null, capsule_path: 'C:\\art\\scan-7-icon.png', is_icon: true };
    invokeLog.length = 0;
    renderCalls = 0;
    const ci = getEl('cardImg');
    ci.src = '';
    ci.style.objectFit = 'cover';
    ci.parentElement.style.background = '';
    applyRealArt();
    await sleep(50);
    const call = invokeLog.find((c) => c.cmd === 'get_game_art');
    assert(call, 'missing get_game_art call');
    assert.strictEqual(call.args.exePath, 'C:\\Games\\Hollow Knight\\Hollow Knight.exe', 'exePath should be passed, got: ' + JSON.stringify(call.args));
    assert.strictEqual(GAMES[0].artIsIcon, true, 'artIsIcon should be set');
    assert.strictEqual(GAMES[0].img, 'data:image/png;base64,AAA', 'tile should be the icon data URI');
    assert.strictEqual(GAMES[0].bg, bgBefore, 'hero must stay the gradient placeholder for icon games');
    assert(tileEls[1].innerHTML.includes('object-fit:contain'), 'tile should re-render centered: ' + tileEls[1].innerHTML);
    assert.strictEqual(ci.src, 'data:image/png;base64,AAA', 'card image should be the icon');
    assert.strictEqual(ci.style.objectFit, 'contain', 'card image should use contain');
    assert(ci.parentElement.style.background.includes('#8B20FF'), 'card should sit on the gradient: ' + ci.parentElement.style.background);
    assert.strictEqual(renderCalls, 0, 'no render for icon games (no hero to apply)');
  });
  await check('applyRealArt still sets the hero for real cover art', async () => {
    GAMES = [{ key: 'scan-7', row: 'Hollow Knight', source: 'scan', scanId: 7, exePath: 'C:\\Games\\Hollow Knight\\Hollow Knight.exe', artIsIcon: false, img: 'data:image/svg+xml;tile', bg: 'data:image/svg+xml;placeholder' }];
    sel = 0;
    tileEls = [makeEl('store'), makeEl('tile0')];
    bgSlides = [makeEl('slide0')];
    mockArt = { tile_data_uri: 'data:image/jpeg;base64,BBB', tile_path: 'C:\\art\\scan-7-header.jpg', hero_path: 'C:\\art\\scan-7-hero.jpg', hero_data_uri: null, capsule_path: 'C:\\art\\scan-7-capsule.jpg', is_icon: false };
    renderCalls = 0;
    applyRealArt();
    await sleep(50);
    assert.strictEqual(GAMES[0].artIsIcon, false, 'artIsIcon should stay false for real art');
    assert.strictEqual(GAMES[0].bg, 'asset://localhost/C:/art/scan-7-hero.jpg', 'hero should be set from hero_path, got: ' + GAMES[0].bg);
    assert.strictEqual(renderCalls, 1, 'render should run when the hero arrives');
    assert(tileEls[1].innerHTML.includes('object-fit:cover'), 'tile should stay full-bleed: ' + tileEls[1].innerHTML);
  });

  console.log('Fail-soft dialog (v0.2.7):');
  await check('no window.confirm remains anywhere in the page', async () => {
    assert(!html.includes('window.confirm'), 'window.confirm still present');
    assert(html.includes('id="fastExitNotice"'), 'missing fastExitNotice element');
    assert(html.includes('fastExitChangeExe()'), 'missing Change launch file button');
    assert(html.includes('hideFastExitNotice()'), 'missing Dismiss button');
  });
  await check('launch-fast-exit shows the DOM notice with the game name', async () => {
    GAMES = [{ key: 'scan-9', row: 'Fail', source: 'scan', scanId: 9, exePath: 'C:\\Games\\Fail\\fail.exe' }];
    sel = 0;
    const n = getEl('fastExitNotice');
    n.style.display = 'none';
    listenFastExit();
    assert(fastExitHandler, 'listener was not registered');
    fastExitHandler({ payload: { game_name: 'Fail', game_key: 'scan-9' } });
    assert.strictEqual(n.style.display, 'block', 'notice did not show');
    assert(getEl('fastExitText').textContent.includes('Fail'), 'notice should name the game: ' + getEl('fastExitText').textContent);
  });
  await check('missing payload name falls back to "The game"', async () => {
    const n = getEl('fastExitNotice');
    n.style.display = 'none';
    fastExitHandler({ payload: {} });
    assert.strictEqual(n.style.display, 'block', 'notice did not show for empty payload');
    assert(getEl('fastExitText').textContent.includes('The game'), 'fallback name missing: ' + getEl('fastExitText').textContent);
    hideFastExitNotice();
  });
  await check('Change launch file opens the picker for the right game', async () => {
    GAMES = [
      { key: 'scan-7', row: 'Hollow Knight', source: 'scan', scanId: 7, exePath: 'C:\\Games\\Hollow Knight\\Hollow Knight.exe' },
      { key: 'scan-9', row: 'Fail', source: 'scan', scanId: 9, exePath: 'C:\\Games\\Fail\\fail.exe' },
    ];
    sel = 0;
    selectGameCalls.length = 0;
    getEl('gameMenu').style.display = 'none';
    fastExitHandler({ payload: { game_name: 'Fail', game_key: 'scan-9' } });
    assert.strictEqual(getEl('fastExitNotice').style.display, 'block', 'notice should be showing');
    fastExitChangeExe();
    assert.strictEqual(sel, 1, 'sel should move to the failed game');
    assert.deepStrictEqual(selectGameCalls, [1], 'selectGame should be called for the failed game');
    assert.strictEqual(getEl('gameMenu').style.display, 'block', 'game menu should open');
    assert.strictEqual(getEl('fastExitNotice').style.display, 'none', 'notice should hide once the picker opens');
    await sleep(250);
    const box = getEl('exeResults').innerHTML;
    assert(box.includes('fail.exe'), 'picker should list the launch file: ' + box);
  });
  await check('Dismiss hides the notice and clears the stored game', async () => {
    GAMES = [{ key: 'scan-9', row: 'Fail', source: 'scan', scanId: 9, exePath: 'C:\\Games\\Fail\\fail.exe' }];
    sel = 0;
    selectGameCalls.length = 0;
    getEl('gameMenu').style.display = 'none';
    fastExitHandler({ payload: { game_name: 'Fail', game_key: 'scan-9' } });
    assert.strictEqual(getEl('fastExitNotice').style.display, 'block', 'notice should be showing');
    hideFastExitNotice();
    assert.strictEqual(getEl('fastExitNotice').style.display, 'none', 'notice should hide on Dismiss');
    fastExitChangeExe();
    assert.deepStrictEqual(selectGameCalls, [], 'Change after Dismiss must not select a game');
    assert.strictEqual(getEl('gameMenu').style.display, 'none', 'menu must not open after Dismiss');
  });
  await check('notice survives the Steam restore path (stopRunningPoll)', async () => {
    const n = getEl('fastExitNotice');
    document.body.classList.add('game-running');
    fastExitHandler({ payload: { game_name: 'Fail', game_key: 'scan-9' } });
    assert.strictEqual(n.style.display, 'block', 'notice should be showing');
    let shown = false;
    currentSessionId = null;
    stopRunningPoll({ show: () => { shown = true; return Promise.resolve(); } });
    assert(!document.body.classList.contains('game-running'), 'game-running class should be removed');
    assert(shown, 'window should be shown again');
    assert.strictEqual(n.style.display, 'block', 'notice must survive the restore');
    hideFastExitNotice();
  });
  await check('notice survives the exe restore path (startExePoll)', async () => {
    GAMES = [{ key: 'scan-9', row: 'Fail', source: 'scan', scanId: 9, exePath: 'C:\\Games\\Fail\\fail.exe' }];
    sel = 0;
    const n = getEl('fastExitNotice');
    document.body.classList.add('game-running');
    let shown = false;
    const win = { show: () => { shown = true; return Promise.resolve(); } };
    mockProcessRunning = true;
    startExePoll(GAMES[0], null, win);
    await sleep(2100); // check 1: running -> seen
    fastExitHandler({ payload: { game_name: 'Fail', game_key: 'scan-9' } });
    assert.strictEqual(n.style.display, 'block', 'notice should be showing');
    mockProcessRunning = false;
    await sleep(2100); // check 2: not running + seen -> restore
    assert(shown, 'window should be shown again after the exe exits');
    assert(!document.body.classList.contains('game-running'), 'game-running class should be removed');
    assert.strictEqual(n.style.display, 'block', 'notice must survive the exe restore');
    hideFastExitNotice();
  });

  console.log(`\n${passed} passed, ${failed} failed`);
  process.exit(failed ? 1 : 0);
}

main().catch((e) => { console.error(e); process.exit(1); });
