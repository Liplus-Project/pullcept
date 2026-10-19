import { mkdir, writeFile } from 'node:fs/promises';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

// Opt-in review artifacts only. The production document and stylesheet are never changed.
export async function sidebarComparisons(page, output) {
  await mkdir(output, {recursive:true});
  const source = await page.locator('#participants').evaluate(e=>({html:e.outerHTML,width:e.getBoundingClientRect().width}));
  const css = readFileSync('src/styles.css','utf8');
  const reviewCss = `
  html,body {margin:0; background:#eee; height:auto; overflow:visible;}
  body {display:block;padding:24px;}
  .comparison {display:flex;gap:24px;align-items:flex-start;width:max-content;}
  .variant {width:var(--review-width);}
  .review-label {height:50px;margin:0;font:13px/1.6 'Segoe UI',sans-serif;color:#333;}
  .variant #participants {width:var(--review-width);height:1500px;max-height:none;box-sizing:border-box;position:relative;flex:none;overflow:hidden;border:1px solid #ddd;}
  .variant .sidebar-grip {display:none;}
  .cards #terminal-facts,.cards #participant-details,.cards #terminal-diagnostics {border:1px solid #e3e3e2!important;border-radius:12px;padding:12px!important;margin:10px 10px 0!important;box-sizing:border-box;}
  .cards #terminal-facts {margin-top:12px!important;}
  .cards #terminal-facts > .review-state {font-size:.78rem;font-weight:650;margin:0 0 8px;}
  .cards #terminal-facts > .facts {padding:0;border:0;}
  .cards .todo-fold > summary {padding-top:0;padding-bottom:0;}
  .cards #terminal-diagnostics > summary {padding-top:0;padding-bottom:0;}
  .after .todo-fold > summary::before,.after #terminal-diagnostics > summary::before,.after .todo-history > summary::before {display:none!important;}
  .after .todo-title::after,.after #terminal-diagnostics > summary::after,.after .todo-history-title::after {content:'';display:inline-block;flex:none;width:.38em;height:.38em;border-right:1px solid currentColor;border-bottom:1px solid currentColor;transform:rotate(-45deg);margin-left:.55em;}
  .after details[open] > summary::after,.after details[open] > summary .todo-title::after,.after details[open] > summary .todo-history-title::after {transform:rotate(45deg)!important;}
  .after .todo-fold > summary::after {order:1;}
  .after .todo-fold > summary .todo-refresh {order:2;margin-left:auto;}
  .after #terminal-diagnostics > summary {display:flex;align-items:center;}
  .after .todo-history > summary::after {order:1;}
  `;
  const labels=['現状：背景色のみ調整','カード案 A：矢印を見出しの前','カード案 B：矢印を見出しの後'];
  const html=`<!doctype html><html lang="ja"><meta charset="utf-8"><style>${css}\n${reviewCss}</style><body><main class="comparison" style="--review-width:${source.width}px">${labels.map((label,i)=>`<section class="variant ${i?'cards':''} ${i===2?'after':''}"><h1 class="review-label">${label}<br>同一データ・幅 ${source.width}px／比較専用</h1>${source.html}</section>`).join('')}</main><script>document.querySelectorAll('.cards #terminal-facts').forEach(e=>{const h=document.createElement('h2');h.className='review-state';h.textContent='状態';e.prepend(h);});</script></body></html>`;
  await writeFile(join(output,'comparison.html'),html,'utf8');
  const review=await page.context().browser().newPage();
  await review.setViewportSize({width:Math.ceil(source.width*3+96),height:1600});
  await review.setContent(html);await review.evaluate(()=>document.fonts.ready);
  await review.locator('.comparison').screenshot({path:join(output,'sidebar-comparison.png')});
  for(let i=0;i<3;i++) await review.locator('.variant').nth(i).screenshot({path:join(output,['current','cards-before','cards-after'][i]+'.png')});
  await review.close();
}

