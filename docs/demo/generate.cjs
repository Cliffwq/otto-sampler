// Render the current frontend with isolated example data, then encode a GIF.
// From any directory: node docs/demo/generate.cjs
// Requires npm ci, Microsoft Edge and ffmpeg on PATH. No app database is opened.
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const http = require('node:http');
const { spawnSync } = require('node:child_process');
const { chromium } = require('playwright');
const root = path.resolve(__dirname, '../..');
const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'otto-demo-'));
const frames = [];
function encode(args) {
  const run = spawnSync('ffmpeg', ['-hide_banner', '-loglevel', 'error', '-y', ...args], { encoding: 'utf8' });
  if (run.error || run.status !== 0) throw run.error || new Error(run.stderr);
}
(async () => {
  const server = http.createServer((req, res) => {
    const url = new URL(req.url, 'http://localhost');
    const relative = url.pathname === '/' ? 'docs/demo/index.html' : url.pathname === '/main.js' ? 'src/main.js' : url.pathname.startsWith('/assets/') ? 'src' + url.pathname : decodeURIComponent(url.pathname.slice(1));
    const file = path.resolve(root, relative);
    if (!file.startsWith(root + path.sep)) { res.writeHead(403); res.end(); return; }
    try {
      res.setHeader('Content-Type', ({ '.html': 'text/html; charset=utf-8', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml' })[path.extname(file)] || 'application/octet-stream');
      res.end(fs.readFileSync(file));
    } catch { res.writeHead(404); res.end(); }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  let browser;
  try {
    browser = await chromium.launch({ headless: true, channel: 'msedge' });
    const page = await browser.newPage({ viewport: { width: 1000, height: 750 }, deviceScaleFactor: 1 });
    await page.addInitScript(() => {
      if (window === window.top) return;
      localStorage.setItem('otto-sampler-startup-scan', 'false');
      localStorage.setItem('otto-sampler-theme', 'light');
      localStorage.setItem('otto-sampler-palette', 'mpc60');
      localStorage.setItem('otto-sampler-sort', 'filename:asc');
      const originals = ['大鼓Kick.wav', '军鼓Snare.wav', '踩镲HiHat.wav', '雨声Rain.wav', '街道Street.wav', '脚步Footsteps.wav'];
      window.demo = { indexed: false, added: false, scannedAddition: false };
      window.__TAURI__ = {
        window: { getCurrentWindow: () => ({ isMaximized: async () => false, onResized: async () => {} }) },
        event: { listen: async () => {} },
        core: { Channel: class {}, convertFileSrc: p => p, invoke: async (name, args) => {
          if (name === 'get_favorites') return [{ name: '素材库', paths: ['E:\\Drums', 'E:\\Textures'], is_default: true }];
          if (name === 'get_window_pin_state') return { docked: false, alwaysOnTop: false };
          if (name === 'scan_favorite') {
            const added = !demo.indexed ? 6 : demo.added && !demo.scannedAddition ? 1 : 0;
            demo.indexed = true; demo.scannedAddition = demo.added;
            return { added, updated: 0, deleted: 0, total: demo.scannedAddition ? 7 : 6 };
          }
          if (name === 'search_files') {
            let names = demo.indexed ? [...originals] : [];
            if (demo.scannedAddition) names.push('大鼓Kick Dry.wav');
            // Explicit fixture outcomes, not a second implementation of the search engine.
            if (args.query) names = names.filter(n => n.includes('大鼓Kick'));
            return { records: names.map(filename => ({ filename, file_type: 'audio', duration_secs: filename === '大鼓Kick Dry.wav' ? 3 : [2, 1, 1, 32, 25, 8][originals.indexOf(filename)], path: 'E:\\Samples\\' + filename })), hasMore: false };
          }
          return [];
        } }
      };
    });
    await page.goto('http://127.0.0.1:' + server.address().port + '/');
    const app = page.frames().find(f => f.url().endsWith('/src/index.html'));
    await app.waitForSelector('#search-input');
    // Disable transitions for consistent frames; capture only settled UI states.
    await app.addStyleTag({ content: '* { transition: none !important; animation: none !important; }' });
    let emphasis = [];
    const highlight = async () => {
      const boxes = [];
      for (const [scope, selector, label] of emphasis) {
        const locator = (scope === 'app' ? app : page).locator(selector);
        const bounds = await Promise.all((await locator.all()).map(el => el.boundingBox()));
        const visible = bounds.filter(Boolean);
        if (!visible.length) throw new Error('Missing highlight target: ' + selector);
        const x = Math.min(...visible.map(b => b.x)), y = Math.min(...visible.map(b => b.y));
        boxes.push({ x: x - 4, y: y - 4, width: Math.max(...visible.map(b => b.x + b.width)) - x + 8, height: Math.max(...visible.map(b => b.y + b.height)) - y + 8, label });
      }
      await page.evaluate(boxes => {
        document.querySelector('#demo-focus')?.remove();
        document.querySelector('#demo-caption')?.remove();
        const ns = 'http://www.w3.org/2000/svg';
        const svg = document.createElementNS(ns, 'svg');
        svg.id = 'demo-focus'; svg.setAttribute('viewBox', '0 0 1000 750');
        svg.style.cssText = 'position:absolute;inset:0;width:1000px;height:750px;pointer-events:none;z-index:10';
        const add = (parent, name, attrs) => { const el = document.createElementNS(ns, name); for (const [k,v] of Object.entries(attrs)) el.setAttribute(k,v); parent.appendChild(el); return el; };
        const defs = add(svg, 'defs', {}), mask = add(defs, 'mask', { id: 'focus-holes' });
        add(mask, 'rect', { width:1000, height:750, fill:'white' });
        for (const b of boxes) add(mask, 'rect', { x:b.x-3,y:b.y-3,width:b.width+6,height:b.height+6,rx:6,fill:'black' });
        add(svg, 'rect', { x:26,y:128,width:948,height:602,rx:7,fill:'#13282f',opacity:0.34,mask:'url(#focus-holes)' });
        for (const b of boxes) {
          add(svg, 'rect', { x:b.x,y:b.y,width:b.width,height:b.height,rx:5,fill:'#ffbd4515',stroke:'#fff9e9','stroke-width':7 });
          add(svg, 'rect', { x:b.x,y:b.y,width:b.width,height:b.height,rx:5,fill:'none',stroke:'#c24b16','stroke-width':3.5 });
          if (b.label && b !== boxes[0]) {
            const width = b.label.length * 9 + 24;
            const x = Math.min(b.x, 974-width), y = Math.max(130,b.y-29);
            add(svg, 'rect', { x,y,width,height:25,rx:4,fill:'#a7390c' });
            const text=add(svg,'text',{x:x+10,y:y+17,fill:'white','font-size':13,'font-family':'Segoe UI, Microsoft YaHei UI, sans-serif','font-weight':600});text.textContent=b.label;
          }
        }
        document.body.appendChild(svg);
        const sourceTarget = window.demoCaption.progress === 35;
        const target = window.demoCaption.progress === 3 ? boxes[1] : boxes[0];
        const bubble = document.createElement('div');
        bubble.id = 'demo-caption';
        const left = sourceTarget ? 410 : 44;
        const top = window.demoCaption.progress === 3 ? 555 : Math.max(160, Math.min(540, target.y + (sourceTarget ? 30 : -10)));
        bubble.style.cssText = 'position:absolute;left:'+left+'px;top:'+top+'px;width:300px;padding:16px 18px;background:#fff8e8;border:3px solid #b54110;border-radius:10px;box-shadow:0 5px 18px #13282f40;z-index:12;color:#293b40';
        const cn = document.createElement('div'), en = document.createElement('div');
        cn.textContent = window.demoCaption.cn; en.textContent = window.demoCaption.en;
        cn.style.cssText = 'font-size:19px;font-weight:650;line-height:1.5';
        en.style.cssText = 'font-size:13px;line-height:1.5;margin-top:8px;color:#566666';
        bubble.append(cn,en); document.body.appendChild(bubble);
        const fromX = sourceTarget ? left : left+300, fromY = top+45;
        const toX = sourceTarget ? target.x+target.width+4 : target.x-5, toY = target.y+target.height/2;
        const marker = add(defs,'marker',{id:'caption-arrow',viewBox:'0 0 10 10',refX:9,refY:5,markerWidth:8,markerHeight:8,orient:'auto'});
        add(marker,'path',{d:'M0 0 L10 5 L0 10Z',fill:'#b54110'});
        const d = 'M'+fromX+' '+fromY+' H362 V'+toY+' H'+toX;
        add(svg,'path',{d,fill:'none',stroke:'#fff8e8','stroke-width':7,'stroke-linejoin':'round'});
        add(svg,'path',{d,fill:'none',stroke:'#b54110','stroke-width':3.5,'stroke-linejoin':'round','marker-end':'url(#caption-arrow)'});
      }, boxes);
    };
    const capture = async seconds => {
      await highlight();
      const file = path.join(temp, String(frames.length).padStart(3, '0') + '.png');
      await page.screenshot({ path: file }); frames.push({ file, seconds });
    };
    const scene = async (step, cn, en, progress, focus = '') => {
      await page.mouse.move(0, 0);
      await page.evaluate(v => window.scene(...v), [step, cn, en, progress, focus]);
      await page.evaluate(() => { document.querySelector('.callout').style.display='none'; document.querySelector('.cursor').style.display='none'; });
      emphasis = progress === 3 ? [['page','.folder',''],['app','#favorites-select','共同搜索 / One library']]
        : focus === 'scan' ? [['app','#scan-btn','点击扫描 / Click scan']]
        : progress === 24 ? [['app','#scan-stats-bar','索引已建立 / Indexed'],['app','.sample-card','6 个素材 / 6 files']]
        : progress === 35 ? [['page','#new-file','新文件 / New file']]
        : progress === 54 ? [['app','#scan-stats-bar','新增 1 / Added 1'],['app','.sample-card:last-child','已加入索引 / Now indexed']]
        : [['app','#search-input','输入关键词 / Type query'],['app','.sample-card','相同的 2 个结果 / Same 2 matches']];
      if (focus === 'scan') {
        const button = await app.locator('#scan-btn').boundingBox();
        const panel = await page.locator('.app').boundingBox();
        await page.evaluate(({ button, panel }) => {
          Object.assign(document.querySelector('.callout').style, { left: button.x - panel.x - 3 + 'px', top: button.y - panel.y - 3 + 'px', width: button.width + 4 + 'px', height: button.height + 4 + 'px' });
          Object.assign(document.querySelector('.cursor').style, { left: button.x - panel.x + 17 + 'px', top: button.y - panel.y + 25 + 'px' });
        }, { button, panel });
      }
    };
    await scene(0, '两个文件夹，一个搜索范围。先建立索引。', 'Two folders, one library. Start by building the index.', 3);
    await capture(2.8);
    await scene(0, '扫描文件名和位置，记成一份本地目录。', 'Scan filenames and locations into a local catalog.', 12, 'scan');
    await capture(1.5);
    await app.locator('#scan-btn').click();
    await app.waitForFunction(() => document.querySelectorAll('.sample-card').length === 6);
    await scene(0, '索引建好了：6 个素材，现在能一起搜索。', 'Index ready: all 6 files can now be searched together.', 24);
    await capture(3.2);
    await page.locator('#new-file').evaluate(el => el.hidden = false);
    await app.evaluate(() => demo.added = true);
    await app.locator('#scan-stats-bar').evaluate(el => el.classList.add('hidden'));
    await scene(1, '文件夹新增了音效，搜索列表还没有它。', 'A new sound is in the folder, but not yet in the index.', 35);
    await capture(3);
    await scene(1, '再点一次增量扫描，检查文件夹里的变化。', 'Run an incremental scan to check what has changed.', 44, 'scan');
    await capture(1.5);
    await app.locator('#scan-btn').click();
    await app.waitForFunction(() => document.querySelectorAll('.sample-card').length === 7);
    await scene(1, '这次只新增 1 条记录，原有 6 条继续使用。', 'Add just 1 record; keep the 6 existing records.', 54);
    await capture(3.4);
    await app.locator('#scan-stats-bar').evaluate(el => el.classList.add('hidden'));
    await scene(2, '搜“鼓 kick”：汉字和英文一起用。', 'Search “鼓 kick”: Chinese and English together.', 65);
    await app.locator('#search-input').fill('鼓 kick');
    await app.waitForFunction(() => document.querySelectorAll('.sample-card').length === 2);
    await capture(3.2);
    await scene(2, '搜“gukick”：拼音也能找到“大鼓Kick”。', 'Search “gukick”: pinyin finds the same Chinese filename.', 78);
    await app.locator('#search-input').fill('gukick');
    await page.waitForTimeout(350);
    await capture(3.2);
    await scene(2, '“大gu kick”也可以，不必统一文件名的语言。', '“大gu kick” works too. No need to rename your files.', 91);
    await app.locator('#search-input').fill('大gu kick');
    await page.waitForTimeout(350);
    await capture(3.4);
    await scene(2, '索引把素材集中起来，混合搜索帮你找到它。', 'One index across folders. Search using the text you remember.', 100);
    await capture(3.5);
    const list = frames.map(f => "file '" + f.file.replace(/\\/g, '/') + "'\nduration " + f.seconds).join('\n') + "\nfile '" + frames.at(-1).file.replace(/\\/g, '/') + "'\n";
    const manifest = path.join(temp, 'frames.txt'); fs.writeFileSync(manifest, list);
    const output = path.join(root, 'docs/images/index-search-demo.gif');
    encode(['-f', 'concat', '-safe', '0', '-i', manifest, '-filter_complex', '[0:v]split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=3:diff_mode=rectangle', '-loop', '0', '-fps_mode', 'vfr', output]);
    console.log(JSON.stringify({ output, frames: frames.length, seconds: frames.reduce((n, f) => n + f.seconds, 0), bytes: fs.statSync(output).size, previewFrames: temp }));
  } finally {
    if (browser) await browser.close();
    server.close();
  }
})().catch(e => { console.error(e); process.exitCode = 1; });
