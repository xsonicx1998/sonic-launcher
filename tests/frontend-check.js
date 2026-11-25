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
    decode(){ return Promise.resolve(); },
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
  hidden: false,
  documentElement: { clientWidth: 1920 },
  addEventListener(){},
};
// Image constructor (buildBgSlides pre-decodes tile images).
global.Image = class {
  constructor(){ this.src = ''; }
  decode(){ return Promise.resolve(); }
};
// localStorage (the v0.2.9 recreate flag survives window destruction).
global.localStorage = {
  _s: {},
  getItem(k){ return Object.prototype.hasOwnProperty.call(this._s, k) ? this._s[k] : null; },
  setItem(k, v){ this._s[k] = String(v); },
  removeItem(k){ delete this._s[k]; },
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
let mockPlaytimeMinutes = 0; // get_playtime result (v0.2.11)
let mockLastPlayed = 0; // get_last_played result, unix seconds (v0.2.11)
let mockYoutubeBlocked = false; // is_youtube_blocked result (v0.2.11)
let mockSearchResults = {}; // key: source|fallback -> array of candidates
let mockKhinsider = null;   // null = not found, string = cached path
let mockCachedMusic = null; // null = none
let mockArt = null;         // null = no art, object = get_game_art result
let mockGames = [];         // list_all_games result (v0.2.9)
let mockPendingFastExit = null; // get_pending_fast_exit result (v0.2.9)
global.window.__TAURI__ = {
  core: {
    invoke: (cmd, args) => {
      invokeLog.push({ cmd, args });
      switch (cmd) {
        case 'get_setting': return Promise.resolve(mockSettings[args.key] ?? null);
        case 'set_setting': mockSettings[args.key] = args.value; return Promise.resolve(null);
        case 'list_all_games': return Promise.resolve(mockGames);
        case 'get_pending_fast_exit': return Promise.resolve(mockPendingFastExit);
        case 'begin_managed_launch': return Promise.resolve(null);
        case 'start_playtime_session': return Promise.resolve(1);
        case 'end_playtime_session': return Promise.resolve(null);
        case 'get_playtime': return Promise.resolve(mockPlaytimeMinutes);
        case 'get_last_played': return Promise.resolve(mockLastPlayed);
        case 'is_youtube_blocked': return Promise.resolve(mockYoutubeBlocked);
        case 'cancel_theme_music': return Promise.resolve(null);
        case 'quit_launcher': return Promise.resolve(null);
        case 'clear_art_music_cache': return Promise.resolve('Cleared 3 art files and 1 music files');
        case 'scan_folders': return Promise.resolve([]);
        case 'list_scan_folders': return Promise.resolve([]);
        case 'launch_game': return Promise.resolve(null);
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
// fetch mock: bundled sfx files exist as .mp3 for the 7 shipped stems only;
// app-data files exist only when flagged. v0.2.11: the lazy loader hardcodes
// .mp3, so any other extension must 404.
let appDataSfx = new Set();
const fetchLog = [];
const SFX_KNOWN_STEMS = ['boot', 'nav', 'select', 'launch', 'back', 'cc_open', 'ambient'];
global.fetch = (url) => {
  fetchLog.push(url);
  const isAppData = url.includes('com.sonic.launcher');
  const stem = (url.match(/\/([^\/]+)\.(mp3|wav|ogg|m4a)$/) || [])[1];
  if (isAppData) {
    return Promise.resolve({ ok: appDataSfx.has(stem), arrayBuffer: () => Promise.resolve(new ArrayBuffer(8)) });
  }
  // bundled dist/sfx/<stem>.mp3 exists only for the shipped stems
  return Promise.resolve({ ok: url.endsWith('.mp3') && SFX_KNOWN_STEMS.includes(stem), arrayBuffer: () => Promise.resolve(new ArrayBuffer(8)) });
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
  'tauriCore', 'getSetting', 'themeAudioEl', 'themeOk', 'fmtDur', 'pickThemeResult',
  'changeThemeMusic', 'tryThemeSearch', 'tryKhinsider', 'playCachedMusic', 'playThemeSrc',
  'stopThemeMusic', 'playAmbientIfSet', 'scheduleThemeMusic',
  'sfxAudioContext', 'sfxSetVolume', 'sfxPreload', 'sfxTryUrls', 'sfxDir', 'ensureSfx', 'playSfx',
  'playSfxBuffer', 'playBootSfx', 'fadeAudioParam', 'startPs5Ambient', 'startPs5AmbientBuffer', 'stopPs5Ambient',
  'resumeHomeAudio', 'enterHome', 'spawnParticles', 'bootSeq', 'skipBoot',
  'isBootVisible', 'tileFocusedHTML', 'tilePlainHTML', 'tileIconHTML', 'escHtml', 'showToast',
  'toggleGameMenu', 'buildGameMenu', 'hideGameMenu',
  'openSearch', 'closeSearch', 'searchGameMatches', 'renderSearchResults',
  'showLbl', 'hideLbl', 'openSettings',
  'pickSearchResult', 'toggleThemePlayback', 'applyRealArt', 'updateGameLogo',
  'baseTileFields', 'placeholderArt', 'mapUnifiedGame', 'titleCaseName', 'renderGameStats',
  'exitLauncher', 'openLibraryGrid', 'closeLibraryGrid', 'pickLibraryGridGame',
  'saveReduceAnimations', 'applyReduceAnimations', 'clearArtMusicCache', 'tickClock',
  'saveAutoFetchArt', 'copySgdbLink',
  'pickCoverArt', 'setCoverArtAppid', 'pasteCoverArtUrl', 'retryArt',
  'listenFastExit', 'showFastExitNotice', 'hideFastExitNotice', 'fastExitChangeExe',
  'openChangeExe', 'startExePoll', 'stopRunningPoll', 'startRunningPoll',
  'launchSelected', 'loadRealPlaytime',
  // v0.2.9: empty library, visibility pause, managed launch.
  'refreshGames', 'buildBgSlides', 'buildRow', 'selectGame', 'render',
  'setHeroBg', 'anchorRow', 'tileSmPx', 'updateEmptyState', 'emptyStateScan',
  'addScanFolderFlow', 'checkPendingFastExit', 'applyVisibilityPause', 'bootSeq',
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
  var themeAudio, themeDebounce, themeAutoTried, themeGen = 0, GAMES, sel, tileEls;
  var toastTimer = null;
  var bgSlides = [];
  var bgStack = getEl('bgStack'), currentBgEl = null, bgZ = 1; // buildBgSlides / setHeroBg
  var exePoll = null, runningPoll = null, currentSessionId = null;
  // Stubs: render/selectGame/refreshGames are replaced below by the
  // real extracted functions wrapped with call counters. Existing
  // tests keep the stub behaviour; v0.2.9 tests opt into the real
  // behaviour with the useReal* flags.
  var renderCalls = 0;
  function render(){ renderCalls++; }
  var SFX_STEMS = ['boot', 'nav', 'select', 'launch', 'back', 'cc_open', 'ambient'];
  var sfxCtx, sfxGain, sfxBuffers = {}, sfxTried = {}, sfxEnabled = true, sfxVolume = 0.8;
  var sfxDirPromise = null;
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
  // Stub: openSettings populates the settings panel; the harness only
  // checks that opening it hides the header tooltip labels (v0.2.14).
  function populateLauncherSettings(){}
  // Evaluate the extracted code in this context (globals above are in scope).
  eval(code);
  // Wrap the real functions (the eval'd declarations replaced the
  // stubs above): keep the call counters, delegate to the real
  // implementation only when a v0.2.9 test opts in.
  const _realSelectGame = selectGame;
  let useRealSelectGame = false;
  selectGame = function(i){
    selectGameCalls.push(i);
    if(useRealSelectGame){
      const need = Math.max(i, sel) + 1;
      while(tileEls.length <= need) tileEls.push(makeEl('tile' + tileEls.length));
      return _realSelectGame(i);
    }
  };
  const _realRender = render;
  let useRealRender = false;
  render = function(){ renderCalls++; if(useRealRender){ return _realRender(); } };
  const _realRefreshGames = refreshGames;
  let useRealRefreshGames = false;
  refreshGames = function(){ refreshGamesCalls++; if(useRealRefreshGames){ return _realRefreshGames(); } };

  // Seed the game list the chain needs.
  GAMES = [{ key: 'g1', row: 'Hollow Knight', source: 'steam', appid: '367520', exePath: '' }];
  sel = 0;
  themeAudio = makeEl('audio');
  themeAudio.volume = 0.3;
  tileEls = [makeEl('tile0'), makeEl('tile1')];

  console.log('Music chain order:');
  await check('YouTube theme -> OST -> KHInsider -> SoundCloud -> ambient', async () => {
    invokeLog.length = 0;
    mockCachedMusic = null;
    mockSearchResults = {};
    mockKhinsider = null;
    mockSettings.ambient_music_path = '';
    themeAutoTried = {};
    scheduleThemeMusic();
    await sleep(1200);
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
    await sleep(1200);
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
  await check('sfxPreload loads settings only — no audio fetched at startup (v0.2.11)', async () => {
    sfxBuffers = {};
    sfxTried = {};
    fetchLog.length = 0;
    sfxPreload();
    await sleep(200);
    assert.strictEqual(Object.keys(sfxBuffers).length, 0, 'no stem should be decoded at startup, got: ' + Object.keys(sfxBuffers).join(','));
    assert.strictEqual(fetchLog.length, 0, 'no audio should be fetched at startup, got: ' + fetchLog.join(', '));
    assert.strictEqual(sfxEnabled, true, 'sfx should stay enabled');
  });
  await check('playSfx lazy-loads on first use: app-data override wins, then bundled .mp3 only (v0.2.11)', async () => {
    sfxBuffers = {};
    sfxTried = {};
    appDataSfx = new Set(['nav']); // user overrode nav in app-data
    fetchLog.length = 0;
    playSfx('nav');
    await sleep(200);
    assert(sfxBuffers['nav'], 'nav not loaded on first use');
    assert(!fetchLog.some((u) => u === 'sfx/nav.mp3'), 'app-data hit must not fall through to bundled: ' + fetchLog.join(', '));
    playSfx('boot');
    await sleep(200);
    assert(sfxBuffers['boot'], 'boot (bundled) not loaded on first use');
    assert(fetchLog.some((u) => u === 'sfx/boot.mp3'), 'bundled boot.mp3 should be fetched: ' + fetchLog.join(', '));
    assert(fetchLog.every((u) => u.endsWith('.mp3')), 'only .mp3 may be fetched, got: ' + fetchLog.join(', '));
    // Second use of the same stem must not fetch again.
    fetchLog.length = 0;
    playSfx('nav');
    playSfx('boot');
    await sleep(100);
    assert.strictEqual(fetchLog.length, 0, 'cached stems must not re-fetch, got: ' + fetchLog.join(', '));
  });
  await check('playSfx is silent for a missing stem (no throw, no buffer)', async () => {
    sfxBuffers = {};
    sfxTried = {};
    playSfx('nonexistent');
    await sleep(200);
    assert(!sfxBuffers['nonexistent'], 'missing stem must not produce a buffer');
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
  await check('••• menu opens for a real game with Change Theme Music + Paste music URL', async () => {
    GAMES = [{ key: 'scan-7', row: 'Hollow Knight', source: 'scan', scanId: 7, exePath: 'C:\\Games\\Hollow Knight\\Hollow Knight.exe', confidence: 'high', hasOverride: false }];
    sel = 0;
    const m = getEl('gameMenu');
    m.style.display = 'none';
    toggleGameMenu();
    assert.strictEqual(m.style.display, 'block', 'menu did not open');
    assert(m.innerHTML.includes('Change Theme Music'), 'missing Change Theme Music: ' + m.innerHTML);
    assert(m.innerHTML.includes('Paste music URL'), 'missing Paste music URL');
    assert(!m.innerHTML.includes('Demo game'), 'no demo notice should remain: ' + m.innerHTML);
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
  await check('Change Theme Music searches YouTube with all three args (gameName, fallback, source)', async () => {
    GAMES = [{ key: 'g1', row: 'Hollow Knight', source: 'steam', appid: '367520', exePath: '' }];
    sel = 0;
    mockSearchResults = { 'youtube|0': [
      { id: 'abc123', title: 'Hollow Knight Theme', duration_secs: 200, source: 'youtube' },
      { id: 'def456', title: 'Hollow Knight OST', duration_secs: 180, source: 'youtube' },
    ] };
    invokeLog.length = 0;
    const box = getEl('themeResults');
    box.innerHTML = '';
    changeThemeMusic();
    await sleep(150);
    const searches = invokeLog.filter((c) => c.cmd === 'search_theme_music');
    assert.strictEqual(searches.length, 1, 'expected 1 search call, got ' + searches.length + ' :: ' + invokeLog.map((c) => c.cmd).join(','));
    assert.strictEqual(searches[0].args.gameName, 'Hollow Knight', 'gameName wrong: ' + JSON.stringify(searches[0].args));
    assert.strictEqual(searches[0].args.fallback, false, 'fallback must be false (standard theme query): ' + JSON.stringify(searches[0].args));
    assert.strictEqual(searches[0].args.source, 'youtube', 'source must be youtube: ' + JSON.stringify(searches[0].args));
    // The top-5 results are listed for the user to pick from.
    assert.deepStrictEqual(window.__themeResults, mockSearchResults['youtube|0'], 'results not stored for pickThemeResult');
    assert(box.innerHTML.includes('Hollow Knight Theme'), 'results not rendered: ' + box.innerHTML);
    assert(!box.innerHTML.includes('Search failed'), 'search must not fail: ' + box.innerHTML);
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
  await check('v0.2.14: header tooltip hides on mouseleave and when search/settings open (no stuck label)', async () => {
    // Live: showLbl shows one label, hideLbl clears both.
    showLbl('lblSearch');
    assert.strictEqual(getEl('lblSearch').style.display, 'block', 'Search label did not show');
    assert.strictEqual(getEl('lblSettings').style.display, 'none', 'Settings label should be hidden while Search shows');
    hideLbl();
    assert.strictEqual(getEl('lblSearch').style.display, 'none', 'hideLbl did not clear the Search label');
    showLbl('lblSettings');
    assert.strictEqual(getEl('lblSettings').style.display, 'block', 'Settings label did not show');
    // Live: opening search hides the label (the old code had no hide
    // path at all — the label stuck until the window was recreated).
    const box = getEl('searchBox');
    box.style.display = 'none';
    openSearch();
    assert.strictEqual(getEl('lblSettings').style.display, 'none', 'opening search did not hide the Settings label');
    assert.strictEqual(box.style.display, 'block', 'search box did not open');
    closeSearch();
    // Static: the icons show on hover and hide on mouseleave, and
    // opening settings hides the label too.
    assert(html.includes('onmouseenter="showLbl(\'lblSearch\')" onmouseleave="hideLbl()"'), 'search icon has no hover show/hide');
    assert(html.includes('onmouseenter="showLbl(\'lblSettings\')" onmouseleave="hideLbl()"'), 'settings icon has no hover show/hide');
    assert(extractFn(script, 'openSettings').includes('hideLbl()'), 'openSettings does not hide the labels');
  });
  await check('v0.2.14: search results scroll on wheel (explicit handler, deltaMode-aware)', async () => {
    // Static: the results div carries an inline wheel handler that
    // scrolls manually (belt-and-braces — native overflow scrolling
    // did not respond to the wheel in the Round 2 RDP session).
    const m = html.match(/<div id="searchResults"[^>]*>/);
    assert(m, 'searchResults div not found');
    assert(m[0].includes('onwheel='), 'searchResults has no wheel handler');
    assert(m[0].includes('scrollTop+=event.deltaY'), 'wheel handler does not scroll manually');
    assert(m[0].includes('deltaMode'), 'wheel handler is not deltaMode-aware');
    assert(m[0].includes('preventDefault()'), 'wheel handler does not preventDefault (double scroll)');
  });
  await check('v0.2.14: hero crossfade hides the old slide immediately (no 450ms linger)', async () => {
    // Live: the previous slide must be at opacity 0 synchronously
    // after setHeroBg — the old 450ms setTimeout left the previous
    // game's art fully visible right after a click.
    const s0 = makeEl('bg0'), s1 = makeEl('bg1');
    bgSlides = [s0, s1];
    currentBgEl = s0;
    bgZ = 1;
    s0.style.opacity = '1';
    s0.classList.add('is-active');
    s1.style.opacity = '0';
    setHeroBg(1);
    assert.strictEqual(s0.style.opacity, '0', 'old slide still visible immediately after selection change');
    assert(!s0.classList.contains('is-active'), 'old slide still has is-active (kenburns keeps running)');
    assert.strictEqual(s1.style.opacity, '1', 'new slide did not fade in');
    assert.strictEqual(currentBgEl, s1, 'currentBgEl did not advance');
    // Static: no delayed hide left in setHeroBg (the word may still
    // appear in the comment explaining the old behaviour — only a
    // real setTimeout( call counts).
    const fn = extractFn(script, 'setHeroBg');
    assert(!fn.includes('setTimeout('), 'setHeroBg still defers hiding the old slide');
    assert(fn.includes('opacity 420ms ease'), 'the 420ms fade-in is gone');
  });
  await check('v0.2.14: Settings panel is opaque (no bleed-through of the Games heading)', async () => {
    // Static: the old semi-transparent radial gradient let the main
    // view's "Games" heading show through at top-left, overlapping
    // the "Settings" title. The panel must be a solid colour now.
    const m = html.match(/<div id="settings"[^>]*>/);
    assert(m, 'settings div not found');
    assert(m[0].includes('background:#0B0F24'), 'settings panel is not opaque: ' + m[0]);
    assert(!html.includes('rgba(200,190,170,.28)'), 'old translucent settings gradient still present');
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
    assert(!m.innerHTML.includes('Set Cover Art'), 'games without a source should not get cover-art items');
  });
  await check('••• menu shows Retry Art for an icon-fallback game with a failed lookup (v0.2.12)', async () => {
    const m = getEl('gameMenu');
    // GTA V case: the tile is the exe icon (artIsIcon) AND the lookup
    // failed (artFailed) — Retry Art must still show; the icon
    // fallback must not hide the retry.
    GAMES = [{ key: 'scan-9', row: 'Grand Theft Auto V', source: 'scan', scanId: 9, artFailed: true, artIsIcon: true }];
    sel = 0;
    buildGameMenu();
    assert(m.innerHTML.includes('Retry Art'), 'missing Retry Art for icon-fallback game with artFailed: ' + m.innerHTML);
    GAMES = [{ key: 'scan-10', row: 'Some Game', source: 'scan', scanId: 10, artFailed: false, artIsIcon: true }];
    buildGameMenu();
    assert(!m.innerHTML.includes('Retry Art'), 'Retry Art should not show for icon game without a failed lookup');
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
    tileEls = [makeEl('tile0')];
    mockArt = { tile_data_uri: 'data:image/png;base64,AAA', tile_path: 'C:\\art\\scan-7-icon.png', hero_path: null, hero_data_uri: null, capsule_data_uri: 'data:image/png;base64,AAA', capsule_path: 'C:\\art\\scan-7-icon.png', logo_path: null, logo_data_uri: null, is_icon: true };
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
    assert(tileEls[0].innerHTML.includes('object-fit:contain'), 'tile should re-render centered: ' + tileEls[0].innerHTML);
    assert.strictEqual(ci.src, 'data:image/png;base64,AAA', 'card image should be the icon');
    assert.strictEqual(ci.style.objectFit, 'contain', 'card image should use contain');
    assert(ci.parentElement.style.background.includes('#8B20FF'), 'card should sit on the gradient: ' + ci.parentElement.style.background);
    assert.strictEqual(renderCalls, 0, 'no render for icon games (no hero to apply)');
  });
  await check('applyRealArt still sets the hero for real cover art', async () => {
    GAMES = [{ key: 'scan-7', row: 'Hollow Knight', source: 'scan', scanId: 7, exePath: 'C:\\Games\\Hollow Knight\\Hollow Knight.exe', artIsIcon: false, img: 'data:image/svg+xml;tile', bg: 'data:image/svg+xml;placeholder' }];
    sel = 0;
    tileEls = [makeEl('tile0')];
    bgSlides = [makeEl('slide0')];
    mockArt = { tile_data_uri: 'data:image/jpeg;base64,BBB', tile_path: 'C:\\art\\scan-7-header.jpg', hero_path: 'C:\\art\\scan-7-hero.jpg', hero_data_uri: null, capsule_data_uri: 'data:image/jpeg;base64,CCC', capsule_path: 'C:\\art\\scan-7-capsule.jpg', logo_path: null, logo_data_uri: null, is_icon: false };
    renderCalls = 0;
    applyRealArt();
    await sleep(50);
    assert.strictEqual(GAMES[0].artIsIcon, false, 'artIsIcon should stay false for real art');
    assert.strictEqual(GAMES[0].bg, 'asset://localhost/C:/art/scan-7-hero.jpg', 'hero should be set from hero_path, got: ' + GAMES[0].bg);
    assert.strictEqual(renderCalls, 1, 'render should run when the hero arrives');
    assert(tileEls[0].innerHTML.includes('object-fit:cover'), 'tile should stay full-bleed: ' + tileEls[0].innerHTML);
  });

  console.log('Three-asset Steam art (v0.2.8):');
  await check('tile uses the capsule first, 16:9 card keeps the header, hero unchanged', async () => {
    GAMES = [{ key: 'scan-7', row: 'Hollow Knight', source: 'scan', scanId: 7, exePath: 'C:\\Games\\Hollow Knight\\Hollow Knight.exe', artIsIcon: false, img: 'data:image/svg+xml;tile', bg: 'data:image/svg+xml;placeholder' }];
    sel = 0;
    tileEls = [makeEl('tile0')];
    bgSlides = [makeEl('slide0')];
    mockArt = { tile_data_uri: 'data:image/jpeg;base64,HEADER', tile_path: 'C:\\art\\scan-7-header.jpg', hero_path: 'C:\\art\\scan-7-hero.jpg', hero_data_uri: null, capsule_data_uri: 'data:image/jpeg;base64,CAPSULE', capsule_path: 'C:\\art\\scan-7-capsule.jpg', logo_path: null, logo_data_uri: null, is_icon: false };
    const ci = getEl('cardImg');
    ci.src = '';
    renderCalls = 0;
    applyRealArt();
    await sleep(50);
    assert.strictEqual(GAMES[0].img, 'data:image/jpeg;base64,CAPSULE', 'tile should be the capsule, got: ' + GAMES[0].img);
    assert.strictEqual(GAMES[0].cardImg, 'data:image/jpeg;base64,HEADER', 'card should keep the header, got: ' + GAMES[0].cardImg);
    assert.strictEqual(ci.src, 'data:image/jpeg;base64,HEADER', 'card image should be the header, got: ' + ci.src);
    assert.strictEqual(GAMES[0].bg, 'asset://localhost/C:/art/scan-7-hero.jpg', 'hero must still be library_hero.jpg, got: ' + GAMES[0].bg);
    assert.strictEqual(renderCalls, 1, 'render should run when the hero arrives');
  });
  await check('tile falls back to the header when the capsule is missing', async () => {
    GAMES = [{ key: 'scan-7', row: 'Hollow Knight', source: 'scan', scanId: 7, exePath: 'C:\\Games\\Hollow Knight\\Hollow Knight.exe', artIsIcon: false, img: 'data:image/svg+xml;tile', bg: 'data:image/svg+xml;placeholder' }];
    sel = 0;
    tileEls = [makeEl('tile0')];
    mockArt = { tile_data_uri: 'data:image/jpeg;base64,HEADER', tile_path: 'C:\\art\\scan-7-header.jpg', hero_path: null, hero_data_uri: null, capsule_data_uri: null, capsule_path: null, logo_path: null, logo_data_uri: null, is_icon: false };
    applyRealArt();
    await sleep(50);
    assert.strictEqual(GAMES[0].img, 'data:image/jpeg;base64,HEADER', 'tile should fall back to the header, got: ' + GAMES[0].img);
    assert.strictEqual(GAMES[0].cardImg, 'data:image/jpeg;base64,HEADER', 'card should be the header too');
  });
  await check('logo shows above Play when present; text title hides on load', async () => {
    GAMES = [{ key: 'steam-367520', row: 'Hollow Knight', source: 'steam', appid: '367520', img: 'data:image/svg+xml;tile', bg: 'data:image/svg+xml;placeholder' }];
    sel = 0;
    tileEls = [makeEl('tile0')];
    mockArt = { tile_data_uri: 'data:image/jpeg;base64,HEADER', tile_path: 'C:\\art\\steam-367520-header.jpg', hero_path: null, hero_data_uri: null, capsule_data_uri: 'data:image/jpeg;base64,CAPSULE', capsule_path: 'C:\\art\\steam-367520-capsule.jpg', logo_data_uri: 'data:image/png;base64,LOGO', logo_path: 'C:\\art\\steam-367520-logo.png', is_icon: false };
    const img = getEl('gameLogoImg');
    const title = getEl('gameLogo');
    img.style.display = 'none';
    title.style.display = '';
    applyRealArt();
    await sleep(50);
    assert.strictEqual(GAMES[0].logoUrl, 'data:image/png;base64,LOGO', 'logoUrl should be set from logo_data_uri');
    assert.strictEqual(title.style.display, '', 'title should stay visible while the logo loads');
    assert.strictEqual(img.style.display, 'none', 'logo should stay hidden while loading');
    assert.strictEqual(typeof img.onload, 'function', 'logo should have a load handler');
    img.onload();
    assert.strictEqual(img.style.display, 'block', 'logo should show after load');
    assert.strictEqual(title.style.display, 'none', 'title should hide once the logo loads');
  });
  await check('text title stays when the game has no logo', async () => {
    GAMES = [{ key: 'scan-7', row: 'Hollow Knight', source: 'scan', scanId: 7, img: 'data:image/svg+xml;tile', bg: 'data:image/svg+xml;placeholder' }];
    sel = 0;
    tileEls = [makeEl('tile0')];
    mockArt = { tile_data_uri: 'data:image/jpeg;base64,HEADER', tile_path: 'C:\\art\\scan-7-header.jpg', hero_path: null, hero_data_uri: null, capsule_data_uri: 'data:image/jpeg;base64,CAPSULE', capsule_path: 'C:\\art\\scan-7-capsule.jpg', logo_path: null, logo_data_uri: null, is_icon: false };
    const img = getEl('gameLogoImg');
    const title = getEl('gameLogo');
    img.style.display = 'block';
    title.style.display = 'none';
    applyRealArt();
    await sleep(50);
    assert.strictEqual(GAMES[0].logoUrl, undefined, 'logoUrl must stay unset without a logo');
    assert.strictEqual(img.style.display, 'none', 'logo img should hide without a logo');
    assert.strictEqual(title.style.display, '', 'text title should show without a logo');
  });
  await check('logo load error falls back to the text title', async () => {
    GAMES = [{ key: 'steam-367520', row: 'Hollow Knight', source: 'steam', appid: '367520', img: 'data:image/svg+xml;tile', bg: 'data:image/svg+xml;placeholder' }];
    sel = 0;
    tileEls = [makeEl('tile0')];
    mockArt = { tile_data_uri: 'data:image/jpeg;base64,HEADER', tile_path: 'C:\\art\\steam-367520-header.jpg', hero_path: null, hero_data_uri: null, capsule_data_uri: 'data:image/jpeg;base64,CAPSULE', capsule_path: 'C:\\art\\steam-367520-capsule.jpg', logo_data_uri: 'data:image/png;base64,LOGO', logo_path: 'C:\\art\\steam-367520-logo.png', is_icon: false };
    const img = getEl('gameLogoImg');
    const title = getEl('gameLogo');
    applyRealArt();
    await sleep(50);
    assert.strictEqual(typeof img.onerror, 'function', 'logo should have an error handler');
    img.onerror();
    assert.strictEqual(img.style.display, 'none', 'logo should hide on load error');
    assert.strictEqual(title.style.display, '', 'text title should show on load error');
  });
  await check('logo element exists with the PS5 sizing and render() wires the card + logo', async () => {
    assert(html.includes('id="gameLogoImg"'), 'missing gameLogoImg element');
    assert(html.includes('max-height:120px'), 'logo should cap at 120px height');
    assert(html.includes('max-width:420px'), 'logo should cap at 420px width');
    assert(html.includes('drop-shadow'), 'logo should have a drop-shadow');
    assert(html.includes('cardImgEl.src = g.cardImg || g.img'), 'render should prefer the header card image');
    assert(html.includes('updateGameLogo()'), 'render should call updateGameLogo');
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

  console.log('Memory trim (v0.2.9):');
  await check('demo games are gone: GAMES starts empty, no base64 art in the page', async () => {
    assert(html.includes('var GAMES = [];'), 'GAMES should start as an empty array');
    assert(!html.includes('var GAMES = [{"key"'), 'demo literal still present');
    assert(!html.includes('/9j/'), 'base64 JPEG art still in the page');
    assert(html.length < 300000, 'page should be ~100KB without demo art, got ' + html.length);
    assert(html.includes('id="emptyState"'), 'missing emptyState element');
    assert(html.includes('Scan for games'), 'missing Scan for games button');
    assert(html.includes('Add Game'), 'missing Add Game button');
  });
  await check('empty library: refreshGames with no games shows the empty state', async () => {
    useRealSelectGame = true; useRealRender = true; useRealRefreshGames = true;
    mockGames = [];
    mockPendingFastExit = null;
    mockCachedMusic = null;
    GAMES = [{ key: 'seed', row: 'Seed', source: 'scan' }];
    sel = 0;
    tileEls = [makeEl('tile0')];
    refreshGames();
    await sleep(50);
    assert.strictEqual(GAMES.length, 0, 'library should be empty, got ' + GAMES.length);
    assert.strictEqual(sel, -1, 'nothing should be selected');
    assert.strictEqual(getEl('emptyState').style.display, 'block', 'empty state should show');
    assert.strictEqual(getEl('heroBlock').style.display, 'none', 'hero should hide when empty');
    assert.strictEqual(getEl('heroCard').style.display, 'none', 'hero card should hide when empty');
    useRealSelectGame = false; useRealRender = false; useRealRefreshGames = false;
  });
  await check('library with games hides the empty state and selects the first game', async () => {
    useRealSelectGame = true; useRealRender = true; useRealRefreshGames = true;
    mockGames = [{ key: 'scan-7', name: 'Hollow Knight', source: 'scan', appid: null, exe_path: 'C:\\Games\\Hollow Knight\\Hollow Knight.exe', launch_uri: null, folder: 'C:\\Games\\Hollow Knight', confidence: 'high', pick_source: 'name', has_override: false, art_failed: false, art_is_icon: false }];
    mockPendingFastExit = null;
    mockCachedMusic = null;
    GAMES = [];
    sel = -1;
    tileEls = [];
    refreshGames();
    await sleep(50);
    assert.strictEqual(GAMES.length, 1, 'library should have one game');
    assert.strictEqual(sel, 0, 'first game should be selected');
    assert.strictEqual(getEl('emptyState').style.display, 'none', 'empty state should hide');
    assert.strictEqual(getEl('heroBlock').style.display, '', 'hero should show');
    assert.strictEqual(GAMES[0].row, 'Hollow Knight', 'wrong game mapped');
    useRealSelectGame = false; useRealRender = false; useRealRefreshGames = false;
  });
  await check('selectGame guards an empty library (no crash, empty state)', async () => {
    useRealSelectGame = true;
    GAMES = [];
    sel = 0;
    tileEls = [makeEl('tile0')];
    selectGame(0);
    assert.strictEqual(sel, -1, 'sel should be -1 for an empty library');
    assert.strictEqual(getEl('emptyState').style.display, 'block', 'empty state should show');
    selectGame(-1);
    assert.strictEqual(sel, -1, 'negative index should stay unselected');
    useRealSelectGame = false;
  });
  await check('visibility pause toggles the page-hidden class', async () => {
    global.document.hidden = true;
    applyVisibilityPause();
    assert(document.body.classList.contains('page-hidden'), 'page-hidden should be set while hidden');
    global.document.hidden = false;
    applyVisibilityPause();
    assert(!document.body.classList.contains('page-hidden'), 'page-hidden should clear when visible');
  });
  await check('boot skips when the window was recreated after a game', async () => {
    GAMES = [{ key: 'scan-7', row: 'Hollow Knight', source: 'scan', img: 'x', bg: 'y' }];
    sel = 0;
    tileEls = [makeEl('tile0')];
    global.localStorage.setItem('sonic-launcher-recreated', '1');
    const b = getEl('boot');
    b.style.display = 'flex';
    bootSeq();
    assert.strictEqual(b.style.display, 'none', 'boot should be skipped on recreate');
    assert.strictEqual(global.localStorage.getItem('sonic-launcher-recreated'), null, 'recreate flag should be consumed');
    await sleep(1250); // enterHome timers settle
  });
  await check('pending fast-exit notice shows on the startup check', async () => {
    mockPendingFastExit = { game_name: 'Fail', game_key: 'scan-9' };
    const n = getEl('fastExitNotice');
    n.style.display = 'none';
    invokeLog.length = 0;
    checkPendingFastExit();
    await sleep(50);
    assert(invokeLog.some((c) => c.cmd === 'get_pending_fast_exit'), 'should ask the backend for the pending notice');
    assert.strictEqual(n.style.display, 'block', 'notice should show');
    assert(getEl('fastExitText').textContent.includes('Fail'), 'notice should name the game');
    hideFastExitNotice();
    mockPendingFastExit = null;
    n.style.display = 'none';
    checkPendingFastExit();
    await sleep(50);
    assert.strictEqual(n.style.display, 'none', 'no notice when none is pending');
  });
  await check('empty-state Scan rescans configured folders and refreshes', async () => {
    mockGames = [];
    invokeLog.length = 0;
    refreshGamesCalls = 0;
    emptyStateScan();
    await sleep(50);
    assert(invokeLog.some((c) => c.cmd === 'list_scan_folders'), 'should list scan folders');
    assert.strictEqual(refreshGamesCalls, 0, 'no refresh with no folders configured (add-folder flow is a UI dialog)');
  });
  await check('launchSelected with minimize ON invokes begin_managed_launch', async () => {
    GAMES = [{ key: 'scan-9', row: 'Fail', source: 'scan', scanId: 9, exePath: 'C:\\Games\\Fail\\fail.exe' }];
    sel = 0;
    mockSettings.minimize_on_launch = '1';
    mockCachedMusic = null;
    invokeLog.length = 0;
    global.localStorage.removeItem('sonic-launcher-recreated');
    launchSelected();
    await sleep(100);
    const launched = invokeLog.find((c) => c.cmd === 'launch_game');
    assert(launched, 'launch_game should be invoked');
    const managed = invokeLog.find((c) => c.cmd === 'begin_managed_launch');
    assert(managed, 'begin_managed_launch should be invoked, got: ' + invokeLog.map((c) => c.cmd).join(','));
    assert.strictEqual(managed.args.gameKey, 'scan-9', 'wrong gameKey');
    assert.strictEqual(managed.args.source, 'scan', 'wrong source');
    assert.strictEqual(managed.args.exePath, 'C:\\Games\\Fail\\fail.exe', 'wrong exePath');
    assert.strictEqual(global.localStorage.getItem('sonic-launcher-recreated'), '1', 'recreate flag should be set before destroy');
    assert(!invokeLog.some((c) => c.cmd === 'start_playtime_session'), 'frontend must not start the session in managed mode');
  });
  await check('launchSelected with minimize OFF starts the frontend session + exe poll', async () => {
    GAMES = [{ key: 'scan-9', row: 'Fail', source: 'scan', scanId: 9, exePath: 'C:\\Games\\Fail\\fail.exe' }];
    sel = 0;
    mockSettings.minimize_on_launch = '0';
    mockCachedMusic = null;
    invokeLog.length = 0;
    launchSelected();
    await sleep(150);
    assert(invokeLog.some((c) => c.cmd === 'start_playtime_session'), 'frontend should start the session, got: ' + invokeLog.map((c) => c.cmd).join(','));
    assert(!invokeLog.some((c) => c.cmd === 'begin_managed_launch'), 'managed launch must not run when OFF');
    assert(exePoll !== null, 'exe poll should be running');
    if(exePoll){ clearInterval(exePoll); exePoll = null; }
    delete mockSettings.minimize_on_launch;
  });

  console.log('Host stays alive (v0.2.10):');
  await check('refreshGames fetches the pending fast-exit (notice surfaces after a window recreate)', async () => {
    useRealSelectGame = true; useRealRender = true; useRealRefreshGames = true;
    mockGames = [{ key: 'scan-9', name: 'Fail', source: 'scan', appid: null, exe_path: 'C:\\Games\\Fail\\fail.exe', launch_uri: null, folder: 'C:\\Games\\Fail', confidence: 'high', pick_source: 'name', has_override: false, art_failed: false, art_is_icon: false }];
    mockPendingFastExit = { game_name: 'Fail', game_key: 'scan-9' };
    mockCachedMusic = null;
    GAMES = [];
    sel = -1;
    tileEls = [];
    const n = getEl('fastExitNotice');
    n.style.display = 'none';
    invokeLog.length = 0;
    refreshGames();
    await sleep(50);
    assert(invokeLog.some((c) => c.cmd === 'get_pending_fast_exit'), 'recreated window must ask for the pending notice on load');
    assert.strictEqual(n.style.display, 'block', 'notice should show after the recreate');
    assert(getEl('fastExitText').textContent.includes('Fail'), 'notice should name the game');
    hideFastExitNotice();
    mockPendingFastExit = null;
    useRealSelectGame = false; useRealRender = false; useRealRefreshGames = false;
  });
  await check('refreshGames with no pending notice shows nothing (consume-on-read)', async () => {
    useRealSelectGame = true; useRealRender = true; useRealRefreshGames = true;
    mockGames = [{ key: 'scan-9', name: 'Fail', source: 'scan', appid: null, exe_path: 'C:\\Games\\Fail\\fail.exe', launch_uri: null, folder: 'C:\\Games\\Fail', confidence: 'high', pick_source: 'name', has_override: false, art_failed: false, art_is_icon: false }];
    mockPendingFastExit = null;
    mockCachedMusic = null;
    GAMES = [];
    sel = -1;
    tileEls = [];
    const n = getEl('fastExitNotice');
    n.style.display = 'none';
    refreshGames();
    await sleep(50);
    assert.strictEqual(n.style.display, 'none', 'no notice when none is pending');
    useRealSelectGame = false; useRealRender = false; useRealRefreshGames = false;
  });

  console.log('v0.2.11: fonts, exit, zero dead clicks, logo, performance:');
  await check('static page: system fonts only, no Google Fonts / Inter / Noto, weights 300/400/600 only', async () => {
    assert(!html.includes('fonts.googleapis'), 'Google Fonts link still present');
    assert(!html.includes("'Inter'"), 'Inter still in the font stack');
    assert(!html.includes('Noto Sans Arabic'), 'Noto Sans Arabic still in the font stack');
    assert(html.includes("font-family:'Segoe UI Variable Text','Segoe UI Variable','Segoe UI',sans-serif"), 'missing UI text stack');
    assert(html.includes("font-family:'Segoe UI Variable Display','Segoe UI Light','Segoe UI',sans-serif"), 'missing title stack');
    assert(!html.includes('font-weight:500'), 'font-weight 500 still present (Win10 Segoe UI has no 500)');
    assert(!html.includes('font-weight:800'), 'font-weight 800 still present (only 300/400/600 allowed)');
  });
  await check('static page: no PS Store tile, no Media tab, Exit in the top bar, clock ticks', async () => {
    assert(!html.includes('PS<br>Store'), 'PS Store tile still in buildRow');
    assert(!html.includes('>Media<'), 'Media tab still in the top bar');
    assert(html.includes('exitLauncher()'), 'missing exitLauncher wiring');
    assert(html.includes('setInterval(tickClock'), 'clock should tick on an interval');
    assert(!html.includes('tileEls[sel+1]'), 'stale tileEls[sel+1] offset (Store tile is gone)');
    assert(!html.includes('tileEls[i+1]'), 'stale tileEls[i+1] offset (Store tile is gone)');
    assert(html.includes('tileEls[sel]'), 'selectGame should use tileEls[sel]');
    assert(html.includes('tileEls[i]'), 'applyRealArt should use tileEls[i]');
  });
  await check('static page: Power menu is a working Exit only; no dead Sleep/Shut Down/Restart', async () => {
    assert(!html.includes('selPower('), 'selPower still present');
    assert(!html.includes('>Sleep<'), 'dead Sleep item still present');
    assert(!html.includes('>Shut Down<'), 'dead Shut Down item still present');
    assert(!html.includes('>Restart<'), 'dead Restart item still present');
    assert(html.includes('Exit Sonic Launcher'), 'missing Exit Sonic Launcher item');
  });
  await check('static page: no dead Control-Centre cards or dock icons (Home + Power only)', async () => {
    assert(!html.includes('newsCard'), 'fake news card still present');
    assert(!html.includes('clipCard'), 'fake clip card still present');
    assert(!html.includes('Customise'), 'dead Customise label still present');
    assert(html.includes('data-label="Home"'), 'Home dock item missing');
    assert(html.includes('data-label="Power"'), 'Power dock item missing');
    assert(!html.includes('data-label="Library"'), 'dead Library dock item still present');
    assert(!html.includes('data-label="Music"'), 'dead Music dock item still present');
    assert(!html.includes('data-label="Notifications"'), 'dead Notifications dock item still present');
    assert(!html.includes('data-label="Game Base"'), 'dead Game Base dock item still present');
  });
  await check('static page: Settings has four working sections, no decorative rows, no Open SFX Folder', async () => {
    assert(!html.includes("User's Guide"), 'decorative User\'s Guide row still present');
    assert(!html.includes('Accessibility'), 'decorative Accessibility row still present');
    assert(!html.includes('System Software'), 'decorative System Software row still present');
    assert(!html.includes('Open SFX Folder'), 'Open SFX Folder button still present');
    assert(!html.includes('openSfxFolder'), 'openSfxFolder still referenced');
    assert(!html.includes('Minimize launcher when'), 'old Minimize label still present');
    assert(html.includes('GENERAL'), 'missing GENERAL section');
    assert(html.includes('LIBRARY &amp; ARTWORK'), 'missing LIBRARY & ARTWORK section');
    assert(html.includes('AUDIO'), 'missing AUDIO section');
    assert(html.includes('ABOUT'), 'missing ABOUT section');
    assert(html.includes('id="reduceAnimations"'), 'missing Reduce Animations toggle');
    assert(html.includes('Close UI to tray while gaming'), 'missing renamed tray setting');
    assert(html.includes('Frees ~150 MB RAM'), 'missing tray subtext');
    assert(html.includes('Optional (for non-Steam exclusives)'), 'missing SGDB optional label');
    assert(html.includes('Clear Art / Music Cache'), 'missing Clear Art / Music Cache button');
    assert(html.includes('Exit Launcher'), 'missing Exit Launcher row');
    assert(html.includes('id="musicEnabled"'), 'missing theme music toggle');
  });
  await check('static page: hero logo is left-bottom, Title Case 42-52px weight 300, scrim present', async () => {
    assert(html.includes('object-position:left bottom'), 'logo should be object-position left bottom');
    assert(html.includes('margin-bottom:24px'), 'logo should sit 24px above Play');
    assert(html.includes('font-size:clamp(42px,3.2vw,52px)'), 'title should be 42-52px');
    assert(html.includes('id="heroScrim"'), 'missing bottom-left scrim');
    assert(html.includes('linear-gradient(to top right,rgba(0,0,0,.75),transparent 60%)'), 'scrim gradient wrong');
    assert(!baseTileFields.toString().includes('.toUpperCase()'), 'baseTileFields must not uppercase the title');
  });
  await check('static page: SFX lazy-loads a single .mp3; music chain has cancel + breaker + 700ms', async () => {
    assert(!html.includes('SFX_EXTS'), 'SFX_EXTS probe list still present');
    assert(html.includes('ensureSfx'), 'missing lazy SFX loader');
    assert(html.includes("'sfx/' + name + '.mp3'"), 'SFX should hardcode the single shipped .mp3');
    assert(html.includes('cancel_theme_music'), 'missing kill-on-change invoke');
    assert(html.includes('is_youtube_blocked'), 'missing circuit-breaker invoke');
    assert(html.includes('themeGen'), 'missing music generation token');
    assert(html.includes("getSetting('music_enabled', '0')"), 'theme music should default OFF');
    assert(html.includes('}, 700);'), 'debounce should be 700ms');
    assert(html.includes("invoke('quit_launcher')"), 'missing quit_launcher invoke');
    assert(html.includes("invoke('get_last_played'"), 'missing get_last_played invoke');
    assert(html.includes("invoke('clear_art_music_cache')"), 'missing clear_art_music_cache invoke');
    assert(html.includes('reduce-animations'), 'missing reduce-animations rules');
    assert(html.includes('id="libraryGrid"'), 'missing Library grid overlay');
    assert(html.includes('function renderGameStats'), 'missing renderGameStats');
    assert(!html.includes("g.hours + '  •  Last played ' + g.last"), 'old unconditional stats line still present');
  });
  await check('titleCaseName keeps acronyms, title-cases ALL-CAPS, passes natural case through', async () => {
    assert.strictEqual(titleCaseName('CYBERPUNK 2077'), 'Cyberpunk 2077');
    assert.strictEqual(titleCaseName('GRAND THEFT AUTO V'), 'Grand Theft Auto V');
    assert.strictEqual(titleCaseName('GTA V'), 'GTA V', 'GTA V must never become Gta V');
    assert.strictEqual(titleCaseName('RDR2'), 'RDR2');
    assert.strictEqual(titleCaseName("BALDUR'S GATE 3"), "Baldur's Gate 3");
    assert.strictEqual(titleCaseName('HADES II'), 'Hades II');
    assert.strictEqual(titleCaseName('KINGDOM COME: DELIVERANCE II'), 'Kingdom Come: Deliverance II');
    assert.strictEqual(titleCaseName('Hollow Knight'), 'Hollow Knight', 'natural case must pass through');
    assert.strictEqual(titleCaseName('Grand Theft Auto V'), 'Grand Theft Auto V');
    assert.strictEqual(titleCaseName(''), 'Unknown');
  });
  await check('baseTileFields: Title Case logo, no source label in tag/meta', async () => {
    const g = baseTileFields('Cyberpunk 2077', '', '');
    assert.strictEqual(g.name, 'Cyberpunk 2077', 'name should stay as-is');
    assert(g.logo.includes('Cyberpunk 2077'), 'logo should show the title: ' + g.logo);
    assert(!g.logo.includes('CYBERPUNK'), 'logo must not be ALL CAPS: ' + g.logo);
    assert(g.logo.includes('font-weight:300'), 'logo should be weight 300: ' + g.logo);
    assert(g.logo.includes('letter-spacing:-0.02em'), 'logo should use -0.02em tracking: ' + g.logo);
    assert.strictEqual(g.tag, '', 'tag should be empty (no source label)');
    assert.strictEqual(g.meta, '', 'meta should be empty (no source label)');
    const u = { key: 'scan-7', name: 'Hollow Knight', source: 'scan', appid: null, exe_path: 'C:\\Games\\Hollow Knight\\Hollow Knight.exe', launch_uri: null, folder: 'C:\\Games\\Hollow Knight', confidence: 'high', pick_source: 'name', has_override: false, art_failed: false, art_is_icon: false };
    const gu = mapUnifiedGame(u);
    assert.strictEqual(gu.tag, '', 'scan games must not show a "scan" tag');
    assert.strictEqual(gu.meta, '', 'scan games must not show a "scan" meta');
    assert(gu.logo.includes('Hollow Knight'), 'unified logo should be Title Case: ' + gu.logo);
  });
  await check('renderGameStats shows only real history (no dashes)', async () => {
    const el = getEl('gameStats');
    renderGameStats({ hours: '\u2014', last: '\u2014' });
    assert.strictEqual(el.textContent, '', 'no history should mean an empty line, got: ' + el.textContent);
    renderGameStats({ hours: '2 hours', last: '\u2014' });
    assert.strictEqual(el.textContent, '2 hours', 'hours only, got: ' + el.textContent);
    renderGameStats({ hours: '\u2014', last: 'Nov 24, 2026' });
    assert.strictEqual(el.textContent, 'Last played Nov 24, 2026', 'last only, got: ' + el.textContent);
    renderGameStats({ hours: '2 hours', last: 'Nov 24, 2026' });
    assert.strictEqual(el.textContent, '2 hours  \u2022  Last played Nov 24, 2026', 'both, got: ' + el.textContent);
  });
  await check('loadRealPlaytime fills hours and the real last-played date', async () => {
    mockPlaytimeMinutes = 125;
    mockLastPlayed = 1795560000; // ~Nov 24, 2026
    GAMES = [{ key: 'g1', row: 'Hollow Knight', source: 'steam' }];
    sel = 0;
    invokeLog.length = 0;
    loadRealPlaytime();
    await sleep(150);
    assert(invokeLog.some((c) => c.cmd === 'get_playtime'), 'should ask for playtime');
    assert(invokeLog.some((c) => c.cmd === 'get_last_played'), 'should ask for the last-played date');
    assert.strictEqual(GAMES[0].hours, '2 hours', 'hours should be "2 hours", got: ' + GAMES[0].hours);
    assert(GAMES[0].last.includes('2026'), 'last should carry the year, got: ' + GAMES[0].last);
    const txt = getEl('gameStats').textContent;
    assert(txt.includes('2 hours') && txt.includes('Last played '), 'stats line should show both, got: ' + txt);
    mockPlaytimeMinutes = 0;
    mockLastPlayed = 0;
  });
  await check('exitLauncher invokes quit_launcher', async () => {
    invokeLog.length = 0;
    exitLauncher();
    await sleep(50);
    assert(invokeLog.some((c) => c.cmd === 'quit_launcher'), 'quit_launcher should be invoked, got: ' + invokeLog.map((c) => c.cmd).join(','));
  });
  await check('Library tile grid opens, lists all games, picks one and closes', async () => {
    GAMES = [
      { key: 'g1', row: 'Hollow Knight', img: 'data:image/jpeg;base64,A', artIsIcon: false },
      { key: 'g2', row: 'Celeste', img: 'data:image/png;base64,B', artIsIcon: true },
    ];
    sel = 0;
    const grid = getEl('libraryGrid');
    const items = getEl('libraryGridItems');
    grid.style.display = 'none';
    openLibraryGrid();
    assert.strictEqual(grid.style.display, 'block', 'grid did not open');
    assert(items.innerHTML.includes('Hollow Knight'), 'grid should list Hollow Knight: ' + items.innerHTML);
    assert(items.innerHTML.includes('Celeste'), 'grid should list Celeste');
    assert(items.innerHTML.includes('linear-gradient(135deg,#8B20FF'), 'icon games should keep the gradient in the grid');
    closeLibraryGrid();
    assert.strictEqual(grid.style.display, 'none', 'grid did not close');
    openLibraryGrid();
    selectGameCalls.length = 0;
    pickLibraryGridGame(1);
    assert.strictEqual(grid.style.display, 'none', 'grid should close after a pick');
    assert.deepStrictEqual(selectGameCalls, [1], 'pick should select game 1');
  });
  await check('Reduce Animations toggle writes the setting and toggles the body class', async () => {
    delete mockSettings.reduce_animations;
    applyReduceAnimations(true);
    assert(document.body.classList.contains('reduce-animations'), 'class should be set');
    applyReduceAnimations(false);
    assert(!document.body.classList.contains('reduce-animations'), 'class should clear');
    saveReduceAnimations({ checked: true });
    assert.strictEqual(mockSettings.reduce_animations, '1', 'check should write 1');
    assert(document.body.classList.contains('reduce-animations'), 'class should be set by the toggle');
    saveReduceAnimations({ checked: false });
    assert.strictEqual(mockSettings.reduce_animations, '0', 'uncheck should write 0');
    assert(!document.body.classList.contains('reduce-animations'), 'class should clear on uncheck');
  });
  await check('Clear Art / Music Cache invokes the backend and refreshes', async () => {
    invokeLog.length = 0;
    refreshGamesCalls = 0;
    getEl('toast').textContent = '';
    clearArtMusicCache();
    await sleep(50);
    assert(invokeLog.some((c) => c.cmd === 'clear_art_music_cache'), 'clear_art_music_cache should be invoked');
    assert.strictEqual(refreshGamesCalls, 1, 'library should refresh after clearing');
    assert(getEl('toast').textContent.includes('Cleared'), 'toast should confirm the clear, got: ' + getEl('toast').textContent);
  });
  await check('theme music is OFF by default: unset setting runs no chain', async () => {
    GAMES = [{ key: 'g1', row: 'Hollow Knight', source: 'steam', appid: '367520' }];
    sel = 0;
    invokeLog.length = 0;
    mockCachedMusic = null;
    mockSearchResults = {};
    mockKhinsider = null;
    themeAutoTried = {};
    delete mockSettings.music_enabled; // never set -> fallback '0'
    scheduleThemeMusic();
    await sleep(1200);
    assert(invokeLog.some((c) => c.cmd === 'cancel_theme_music'), 'kill-on-change should still fire');
    assert(!invokeLog.some((c) => c.cmd === 'ensure_ytdlp'), 'no yt-dlp should spawn while theme music is off, got: ' + invokeLog.map((c) => c.cmd).join(','));
    assert(!invokeLog.some((c) => c.cmd === 'search_theme_music'), 'no search should run while theme music is off');
    mockSettings.music_enabled = '1';
  });
  await check('selection change kills the running yt-dlp child (cancel_theme_music)', async () => {
    GAMES = [{ key: 'g1', row: 'Hollow Knight', source: 'steam', appid: '367520' }];
    sel = 0;
    mockSettings.music_enabled = '0'; // chain stops after the debounce; only the cancel matters here
    invokeLog.length = 0;
    scheduleThemeMusic();
    scheduleThemeMusic();
    const cancels = invokeLog.filter((c) => c.cmd === 'cancel_theme_music');
    assert.strictEqual(cancels.length, 2, 'each schedule should cancel the previous child, got ' + cancels.length);
    await sleep(900);
    assert(!invokeLog.some((c) => c.cmd === 'search_theme_music'), 'disabled chain must not search');
    mockSettings.music_enabled = '1';
  });
  await check('circuit-breaker: blocked YouTube skips both YouTube searches', async () => {
    GAMES = [{ key: 'g1', row: 'Hollow Knight', source: 'steam', appid: '367520' }];
    sel = 0;
    invokeLog.length = 0;
    mockCachedMusic = null;
    mockSearchResults = {};
    mockKhinsider = null;
    mockYoutubeBlocked = true;
    themeAutoTried = {};
    scheduleThemeMusic();
    await sleep(1200);
    assert(invokeLog.some((c) => c.cmd === 'is_youtube_blocked'), 'should ask the breaker');
    const searches = invokeLog.filter((c) => c.cmd === 'search_theme_music');
    assert.strictEqual(searches.length, 1, 'only the SoundCloud search should run, got ' + searches.length + ' :: ' + invokeLog.map((c) => c.cmd).join(','));
    assert.strictEqual(searches[0].args.source, 'soundcloud', 'the one search should be SoundCloud');
    assert(invokeLog.some((c) => c.cmd === 'download_khinsider_theme'), 'KHInsider should still run');
    mockYoutubeBlocked = false;
  });
  await check('buildRow has no Store tile: game 0 is tileEls[0], Library tile is wired', async () => {
    const g1 = baseTileFields('Hollow Knight', '', ''); g1.key = 'scan-7'; g1.source = 'scan';
    const g2 = baseTileFields('Celeste', '', ''); g2.key = 'scan-8'; g2.source = 'scan';
    GAMES = [g1, g2];
    buildRow();
    assert.strictEqual(tileEls.length, 4, 'expected 2 games + plus + Library, got ' + tileEls.length);
    assert(tileEls[0].innerHTML.includes('data:image/svg+xml'), 'tileEls[0] should be the first game tile: ' + tileEls[0].innerHTML.slice(0, 120));
    assert(!tileEls.some((t) => t.innerHTML.includes('PS<br>Store')), 'no tile should be the PS Store');
    assert.strictEqual(typeof tileEls[3].onclick, 'function', 'Library tile should be clickable');
    assert.strictEqual(typeof tileEls[2].onclick, 'function', 'plus tile should be clickable');
  });
  await check('selectGame (real) focuses tileEls[sel] with the Store tile gone', async () => {
    useRealSelectGame = true; useRealRender = true;
    const g1 = baseTileFields('Hollow Knight', '', ''); g1.key = 'scan-7'; g1.source = 'scan';
    GAMES = [g1];
    sel = -1;
    tileEls = [makeEl('tile0')];
    renderCalls = 0;
    mockSettings.music_enabled = '0'; // keep the background chain quiet
    selectGame(0);
    assert.strictEqual(sel, 0, 'sel should be 0');
    assert(tileEls[0].classList.contains('is-focused'), 'game tile should be focused');
    assert(tileEls[0].innerHTML.includes('ps5-focus-ring'), 'focused tile should have the ring');
    assert(renderCalls >= 1, 'render should run on select');
    useRealSelectGame = false; useRealRender = false;
    mockSettings.music_enabled = '1';
  });
  await check('static page: gamepad polling survives skipping the boot screen (no dead gamepad)', async () => {
    const fn = extractFn(script, 'pollGamepad');
    // The boot branch must re-schedule the next frame before returning:
    // a bare `skipBoot(); return;` used to exit before the final
    // requestAnimationFrame, killing gamepad polling for the run.
    assert(fn.includes('skipBoot(); requestAnimationFrame(pollGamepad); return;'), 'boot branch must re-schedule pollGamepad before returning');
    assert(!/skipBoot\(\); return;/.test(fn), 'bare skipBoot(); return; still present — gamepad dies on boot button press');
    assert.strictEqual((fn.match(/requestAnimationFrame\(pollGamepad\)/g) || []).length, 2, 'pollGamepad should re-schedule twice: boot branch + end of loop');
  });

  console.log(`\n${passed} passed, ${failed} failed`);
  process.exit(failed ? 1 : 0);
}

main().catch((e) => { console.error(e); process.exit(1); });
