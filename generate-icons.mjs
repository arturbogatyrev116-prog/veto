import { Resvg } from '@resvg/resvg-js';
import { readFileSync, writeFileSync, mkdirSync } from 'fs';
import { join } from 'path';

const svgData = readFileSync('./icon-source.svg', 'utf-8');

const sizes = [32, 128, 256, 512, 1024];

mkdirSync('./icon-build', { recursive: true });

for (const size of sizes) {
  const resvg = new Resvg(svgData, {
    fitTo: { mode: 'width', value: size },
  });
  const png = resvg.render().asPng();
  const out = join('./icon-build', `icon-${size}.png`);
  writeFileSync(out, png);
  console.log(`✓ ${out}`);
}

// favicon (только V, без фона)
const faviconSvg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32" width="32" height="32">
  <defs>
    <linearGradient id="vGrad32" x1="0%" y1="0%" x2="100%" y2="100%">
      <stop offset="0%" style="stop-color:#B87BFF;stop-opacity:1"/>
      <stop offset="100%" style="stop-color:#9D4EDD;stop-opacity:1"/>
    </linearGradient>
  </defs>
  <path d="M6 4 L10 24 L14 24 L18 4 L16 4 L12 20 L8 4 Z" fill="url(#vGrad32)"/>
</svg>`;

writeFileSync('./icon-build/favicon.svg', faviconSvg);
console.log('✓ ./icon-build/favicon.svg');

const faviconResvg32 = new Resvg(faviconSvg, { fitTo: { mode: 'width', value: 32 } });
writeFileSync('./icon-build/favicon-32.png', faviconResvg32.render().asPng());
console.log('✓ ./icon-build/favicon-32.png');

const faviconResvg16 = new Resvg(faviconSvg, { fitTo: { mode: 'width', value: 16 } });
writeFileSync('./icon-build/favicon-16.png', faviconResvg16.render().asPng());
console.log('✓ ./icon-build/favicon-16.png');

console.log('\nDone. Now run: npx @tauri-apps/cli icon ./icon-build/icon-1024.png');
