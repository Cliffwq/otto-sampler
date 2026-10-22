const { chromium } = require('playwright');
const fs=require('fs'), assert=require('assert/strict');
(async()=>{
 const browser=await chromium.launch({headless:true,channel:"msedge"});
 const page=await browser.newPage({viewport:{width:1200,height:800}});
 await page.route('https://otto.test/**', route=>{
  const path=new URL(route.request().url()).pathname;
  const file='src/'+(path==='/'?'index.html':path.slice(1));
  route.fulfill({body:fs.readFileSync(file),contentType:path.endsWith('.css')?'text/css':path.endsWith('.js')?'text/javascript':path.endsWith('.svg')?'image/svg+xml':'text/html'});
 });
 await page.addInitScript(()=>{
  window.calls=[]; window.listeners={}; window.pin={docked:false,alwaysOnTop:false};
  const win={isMaximized:async()=>false,onResized:async()=>{},minimize:async()=>calls.push('minimize'),toggleMaximize:async()=>calls.push('maximize'),close:async()=>calls.push('close')};
  window.__TAURI__={window:{getCurrentWindow:()=>win},event:{listen:async(name,fn)=>listeners[name]=fn},core:{Channel:class {},convertFileSrc:p=>p,invoke:async(name,args)=>{
   calls.push({name,args});
   if(name==='scan_favorite') return {added:0,updated:0,deleted:0,total:25};
   if(name==='get_favorites') return [{name:'测试素材',is_default:true,paths:[]}];
   if(name==='reset_file_index') { if(window.resetFailure) throw Error('扫描进行中'); window.indexCleared=true; return null; }
   if(name==='search_files') {
     if(window.indexCleared) return {records:[],hasMore:false};
     if(window.paginationTest) {
       if(args.query==='slow') await new Promise(resolve=>window.resolveSlow=resolve);
       if(window.failPage && args.offset===200) { window.failPage=false; throw Error('temporary failure'); }
       if(args.offset>0) await new Promise(resolve=>setTimeout(resolve,60));
       const total=args.query==='new'?1:450;
       return {records:Array.from({length:Math.min(200,Math.max(0,total-args.offset))},(_,i)=>({file_type:'audio',filename:args.query+' '+(args.offset+i)+'.mp3',path:args.query+':'+(args.offset+i),duration_secs:2})),hasMore:args.offset+200<total};
     }
     return {records:Array.from({length:25},(_,i)=>({file_type:'audio',filename:'清晰的测试声音 '+(i+1)+(i%2?'.wav':'.mp3'),path:'sample'+i,duration_secs:i+1})),hasMore:false};
   }
   if(name==='get_window_pin_state') return pin;
   if(name==='toggle_window_pin') {pin.alwaysOnTop=!pin.alwaysOnTop; return pin;}
   return [];
  }}};
  if (!localStorage.getItem('otto-sampler-sort')) localStorage.setItem('otto-sampler-sort', 'mtime:desc');
 });
 await page.goto('https://otto.test/');
 await page.waitForSelector('.sample-card');
 await page.locator('.sample-card').first().dispatchEvent('dragstart');
 await page.waitForFunction(()=>calls.some(x=>x.name==='plugin:drag|start_drag'));
 const drag=await page.evaluate(()=>calls.find(x=>x.name==='plugin:drag|start_drag').args);
 assert.equal(drag.image,'data:image/png;base64,'+fs.readFileSync('src-tauri/icons/32x32.png').toString('base64'));
 assert.deepEqual(drag.item,['sample0']);
 await page.waitForFunction(()=>calls.some(x=>x.name==='scan_favorite'));
 assert.equal(await page.evaluate(()=>calls.filter(x=>x.name==='scan_favorite').length),1);
 await page.locator('#settings-btn').click();
 assert.equal(await page.locator('#startup-scan').isChecked(),true);
 await page.locator('#startup-scan').uncheck();
 await page.reload(); await page.waitForSelector('.sample-card');
 await page.waitForFunction(()=>calls.some(x=>x.name==='get_window_pin_state'));
 await page.waitForTimeout(100);
 assert.equal(await page.evaluate(()=>calls.filter(x=>x.name==='scan_favorite').length),0);
 await page.locator('#settings-btn').click();
 assert.equal(await page.locator('#startup-scan').isChecked(),false);
 await page.locator('#startup-scan').check();
 await page.reload(); await page.waitForSelector('.sample-card');
 await page.waitForFunction(()=>calls.some(x=>x.name==='scan_favorite'));
 assert.equal(await page.evaluate(()=>calls.filter(x=>x.name==='scan_favorite').length),1);
 await page.locator('#scan-btn').hover();
 assert.equal(await page.locator('#control-hint').textContent(),'增量扫描');
 assert.equal(await page.locator('button[title]').count(),0);
 await page.waitForTimeout(1100);
 assert.equal(await page.locator('#control-hint').textContent(),'增量扫描');
 await page.locator('#settings-btn').hover();
 assert.equal(await page.locator('#control-hint').textContent(),'设置');
 await page.locator('.section-title').hover();
 assert.equal(await page.locator('#control-hint').textContent(),'');
 for (const id of ['volume-slider','progress-slider']) {
   const slider=page.locator('#'+id), box=await slider.boundingBox();
   assert(box.height>=28);
   await slider.click({position:{x:box.width*.7,y:3}});
   assert(Number(await slider.inputValue())>50);
 }
 await page.locator('.section-title').hover();
 assert.equal(await page.locator('#control-hint').textContent(),'');
 await page.screenshot({path:'tools/ui-hints.png',animations:'disabled'});
 assert.equal(await page.locator('#sort-select').inputValue(),'mtime:desc');
 assert.equal(await page.locator('html').getAttribute('data-palette'),'mpc60');
 const firstSearch=await page.evaluate(()=>calls.find(x=>x.name==='search_files'));
 assert.equal(firstSearch.args.sortField,'mtime'); assert.equal(firstSearch.args.sortDir,'desc');
 assert.equal(await page.locator('.card-drag-handle').count(),0);
 assert.equal(await page.locator('.badge').count(),0);
 assert.equal(await page.locator('.brand-logo').count(),0);
 assert.equal(await page.locator('.brand-wordmark').getAttribute('alt'),'OTTO Sampler');
 const aligned=await page.evaluate(()=>{const a=document.querySelector('.filters').getBoundingClientRect(), b=document.querySelector('.sort-control').getBoundingClientRect();return Math.abs(a.top+a.height/2-b.top-b.height/2)<1;});assert(aligned);
 assert.equal(await page.locator('.favorites-strip #favorites-btn').count(),1);
 assert.equal(await page.locator('.titlebar #favorites-btn').count(),0);
 assert.equal(await page.locator('.card-duration').first().textContent(),'00:01');
 const gap=await page.locator('.sample-card').evaluateAll(cards=>cards[1].getBoundingClientRect().top-cards[0].getBoundingClientRect().bottom); assert.equal(gap,0);
 await page.locator('#sort-select').selectOption('duration:desc');
 assert.equal(await page.evaluate(()=>localStorage.getItem('otto-sampler-sort')),'duration:desc');
 assert.equal(await page.locator('[data-tauri-drag-region]').count(),0);
 await page.locator('.brand').click();
 assert.equal(await page.evaluate(()=>calls.filter(x=>x.name==='begin_window_drag').length),1);
 await page.locator('#always-on-top-btn').click();
 assert.equal(await page.evaluate(()=>calls.filter(x=>x.name==='begin_window_drag').length),1);
 assert.equal(await page.locator('#always-on-top-btn').getAttribute('aria-pressed'),'true');
 await page.evaluate(()=>listeners['window-pin-state']({payload:{docked:true,alwaysOnTop:true}}));
 assert.equal(await page.locator('#always-on-top-btn').isDisabled(),true);
 await page.evaluate(()=>listeners['window-pin-state']({payload:{docked:false,alwaysOnTop:false}}));
 assert.equal(await page.locator('#always-on-top-btn').isEnabled(),true);
 for(const id of ['minimize','maximize','close']) await page.locator('#'+id+'-btn').click();
 const calls=await page.evaluate(()=>window.calls); for(const name of ['minimize','maximize','close']) assert(calls.includes(name));

 await page.locator('.sample-card').first().click({button:'right'});
 assert.equal(await page.locator('#file-context-menu').evaluate(el=>getComputedStyle(el).position),'fixed');
 await page.keyboard.press('Escape');
 assert.equal(await page.locator('#search-input').getAttribute('autocomplete'),'off');
 const cfg=JSON.parse(fs.readFileSync('src-tauri/tauri.conf.json','utf8'));
 assert.equal(cfg.app.windows[0].decorations,false);
 assert.equal(cfg.app.windows[0].generalAutofillEnabled,false);
 await page.locator('#settings-btn').click();
 await page.locator('input[name="theme"][value="light"]').check();
 assert.equal(await page.locator('html').getAttribute('data-theme'),'light');
 await page.locator('#settings-close-btn').click();
 await page.mouse.move(900,700);
 await page.screenshot({animations:'disabled',path:'tools/ui-light.png'});
 await page.locator('#settings-btn').click();
 await page.locator('input[name="theme"][value="dark"]').check();
 assert.equal(await page.evaluate(()=>localStorage.getItem('otto-sampler-theme')),'dark');
 await page.reload();
 await page.waitForSelector('.sample-card');
 assert.equal(await page.locator('#sort-select').inputValue(),'duration:desc');
 assert.equal(await page.locator('html').getAttribute('data-theme'),'dark');
 await page.emulateMedia({colorScheme:'light'});
 assert.equal(await page.locator('html').getAttribute('data-theme'),'dark');
 await page.screenshot({animations:'disabled',path:'tools/ui-dark.png'});
 await page.locator('#settings-btn').click();
 await page.locator('input[name="theme"][value="system"]').check();
 assert.equal(await page.locator('html').getAttribute('data-theme'),'light');
 await page.emulateMedia({colorScheme:'dark'});
 await page.waitForFunction(()=>document.documentElement.dataset.theme==='dark');
 await page.screenshot({animations:'disabled',path:'tools/ui-settings.png'});
 await page.setViewportSize({width:700,height:800});
 assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),true);
 await page.screenshot({animations:'disabled',path:'tools/ui-compact.png'});
 await page.keyboard.press('Escape');
 assert.equal(await page.locator('#settings-modal').isVisible(),false);


 await page.setViewportSize({width:1000,height:720});
 const paletteColors=[];
 for(const palette of ['mpc2000xl','mpc60','sp1200','sp404']) {
   for(const mode of ['light','dark']) {
     await page.locator('#settings-btn').click();
     await page.locator('#palette-select').selectOption(palette);
     await page.locator('input[name="theme"][value="'+mode+'"]').check();
     assert.equal(await page.locator('html').getAttribute('data-palette'),palette);
     assert.equal(await page.locator('html').getAttribute('data-theme'),mode);
     await page.locator('#settings-close-btn').click();
     paletteColors.push(await page.evaluate(()=>getComputedStyle(document.documentElement).getPropertyValue('--bg-base').trim()));
     await page.mouse.move(950,680);
     await page.screenshot({animations:'disabled',path:'tools/ui-'+palette+'-'+mode+'.png'});
   }
 }
 assert.equal(new Set(paletteColors).size,8);
 await page.reload(); await page.waitForSelector('.sample-card');
 assert.equal(await page.locator('html').getAttribute('data-palette'),'sp404');
 assert.equal(await page.locator('html').getAttribute('data-theme'),'dark');


 for(const id of ['scan-btn','favorites-btn','settings-btn']) {
   assert.equal((await page.locator('#'+id).textContent()).trim(),'');
   assert(await page.locator('#'+id).getAttribute('aria-label'));
 }
 const player=await page.evaluate(()=>{const a=document.querySelector('#play-pause-btn').getBoundingClientRect(),b=document.querySelector('#stop-btn').getBoundingClientRect(),c=document.querySelector('.volume-control').getBoundingClientRect();return {gap:b.left-a.right,volumeGap:c.left-b.right,icon:document.querySelector('#vol-icon svg').getBoundingClientRect().width};});
 assert(player.gap<=8);assert(player.volumeGap<=16);assert.equal(player.icon,24);
 await page.locator('#favorites-btn').click();
 await page.screenshot({animations:'disabled',path:'tools/ui-path-manager.png'});
 await page.locator('#modal-close-btn').click();
 await page.evaluate(()=>localStorage.setItem('unrelated-key','keep'));
 await page.locator('#settings-btn').click();
 await page.locator('#reset-preferences-btn').click();
 await page.screenshot({animations:'disabled',path:'tools/ui-reset-settings.png'});
 await page.locator('#settings-confirm-cancel').click();
 assert.equal(await page.locator('html').getAttribute('data-palette'),'sp404');
 await page.locator('#reset-preferences-btn').click();
 await page.locator('#settings-confirm-apply').click();
 await page.getByText('偏好设置已恢复默认。').waitFor();
 assert.equal(await page.locator('html').getAttribute('data-palette'),'mpc60');
 assert.equal(await page.locator('#sort-select').inputValue(),'filename:asc');
 assert.equal(await page.locator('#volume-slider').inputValue(),'80');
 assert.equal(await page.locator('input[name="theme"][value="system"]').isChecked(),true);
 assert.equal(await page.evaluate(()=>localStorage.getItem('otto-sampler-palette')),null);
 assert.equal(await page.evaluate(()=>localStorage.getItem('unrelated-key')),'keep');
 await page.locator('#reset-index-btn').click();
 await page.locator('#settings-confirm-cancel').click();
 assert.equal(await page.evaluate(()=>calls.filter(x=>x.name==='reset_file_index').length),0);
 await page.evaluate(()=>window.resetFailure=true);
 await page.locator('#reset-index-btn').click();
 await page.locator('#settings-confirm-apply').click();
 await page.getByText('重置失败：Error: 扫描进行中').waitFor();
 assert.equal(await page.locator('.sample-card').count(),25);
 await page.evaluate(()=>window.resetFailure=false);
 await page.locator('#settings-confirm-apply').click();
 await page.getByText('索引已清空，点击增量扫描重新建立。').waitFor();
 assert.equal(await page.locator('.sample-card').count(),0);
 assert.equal(await page.locator('#favorites-select').inputValue(),'测试素材');
 await page.locator('#settings-close-btn').click();
 await page.evaluate(()=>{window.indexCleared=false;window.paginationTest=true;});
 await page.locator('#search-input').fill('pages');
 await page.waitForFunction(()=>document.querySelector('#results-count').textContent==='前200');
 const bottom=async()=>page.locator('#results-list').evaluate(el=>{el.scrollTop=el.scrollHeight;for(let i=0;i<5;i++)el.dispatchEvent(new Event('scroll'));});
 await bottom();
 await page.waitForFunction(()=>document.querySelector('#results-count').textContent==='前400');
 assert.equal(await page.locator('.sample-card').count(),400);
 assert.equal(await page.evaluate(()=>calls.filter(x=>x.name==='search_files'&&x.args.query==='pages'&&x.args.offset===200).length),1);
 await bottom();
 await page.waitForFunction(()=>document.querySelector('#results-count').textContent==='共450');
 assert.equal(await page.locator('.sample-card').count(),450);
 assert.equal(await page.locator('.sample-card').evaluateAll(cards=>new Set(cards.map(c=>c.dataset.path)).size),450);
 await bottom();
 assert.equal(await page.evaluate(()=>calls.filter(x=>x.name==='search_files'&&x.args.query==='pages').length),3);
 await page.evaluate(()=>window.failPage=true);
 await page.locator('#search-input').fill('retry');
 await page.waitForFunction(()=>document.querySelector('#results-count').textContent==='前200');
 await bottom();
 await page.getByText('加载失败，点击重试').waitFor();
 assert.equal(await page.locator('.sample-card').count(),200);
 await page.getByText('加载失败，点击重试').click();
 await page.waitForFunction(()=>document.querySelector('#results-count').textContent==='前400');
 await page.locator('#search-input').fill('slow');
 await page.waitForFunction(()=>typeof window.resolveSlow==='function');
 await page.locator('#search-input').fill('new');
 await page.waitForFunction(()=>document.querySelector('#results-count').textContent==='共1');
 await page.evaluate(()=>window.resolveSlow());
 await page.waitForTimeout(100);
 assert.equal(await page.locator('.sample-card').count(),1);
 assert.equal(await page.locator('.sample-card').first().getAttribute('data-path'),'new:0');
 await page.setViewportSize({width:360,height:650});
 await page.locator('#scan-btn').hover();
 assert.equal(await page.locator('#control-hint').textContent(),'增量扫描');
 assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
 const hintBox=await page.locator('#control-hint').boundingBox(); assert(hintBox.width>=70);
 await page.evaluate(()=>{
 document.querySelector('#scan-stats-bar').classList.remove('hidden');
 document.querySelector('#stats-total').textContent='5343';
 document.querySelector('#stats-updated').textContent='8';
 });
 const numericFonts=await page.evaluate(()=>['.card-duration','#player-time','#stats-total'].map(selector=>{
 const css=getComputedStyle(document.querySelector(selector));
 return [css.fontFamily,css.fontSize,css.fontWeight,css.fontVariantNumeric,css.letterSpacing];
 }));
 assert.deepEqual(numericFonts[1],numericFonts[0]); assert.deepEqual(numericFonts[2],numericFonts[0]);
 assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
 await page.screenshot({path:'tools/ui-hints-narrow.png',animations:'disabled'});
 await page.locator('#scan-btn').click();
 await page.locator('#scan-stats-bar').waitFor({state:'visible'});
 await page.locator('#scan-stats-bar').waitFor({state:'hidden',timeout:6500});
 await browser.close(); console.log('PASS: sort, continuous rows, pin/window controls, context menu, theme persistence/system changes, compact layout, autofill config, paginated scroll/retry/stale replies');
})().catch(e=>{console.error(e);process.exit(1)});
