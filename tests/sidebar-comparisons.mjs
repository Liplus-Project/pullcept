import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { join } from 'node:path';

// Production screenshots only: #529's review-only card overrides are retired by #534.
export async function sidebarComparisons(page, output) {
  await mkdir(output,{recursive:true});
  await page.evaluate(()=>document.activeElement?.blur());
  const viewport=page.viewportSize();
  const panel=page.locator('#participants');
  const width=await panel.evaluate(e=>e.style.width);
  await page.setViewportSize({width:1280,height:1700});
  for(const theme of ['light','dark']) {
    await page.emulateMedia({colorScheme:theme});
    for(const size of [264,192]) {
      await panel.evaluate((e,size)=>e.style.width=`${size}px`,size);
      await panel.screenshot({path:join(output,`production-${theme}-${size}.png`)});
      assert.ok(await panel.evaluate(e=>e.scrollWidth<=e.clientWidth+1),'panel content fits narrow width');
      assert.equal(await page.locator('#terminal-facts').evaluate(e=>getComputedStyle(e).borderTopColor),theme==='light'?'rgb(227, 227, 226)':'rgb(51, 51, 58)');
    }
  }
  await panel.evaluate((e,width)=>e.style.width=width,width);
  await page.emulateMedia({colorScheme:'light'});
  await page.setViewportSize(viewport);
}
