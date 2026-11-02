// Generates the SWORTA logo and the animated wordmark as SVG, light and dark.
// Usage: node docs/brand.mjs docs
import { writeFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";

const out = process.argv[2] ?? ".";
mkdirSync(out, { recursive: true });

// Same 5x7 glyphs as src/dots.rs.
const GLYPHS = {
  S: [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
  W: [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
  O: [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
  R: [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
  T: [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
  A: [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
};

const THEMES = {
  dark: { on: "#ECECEC", dim: "#2A2A2A", red: "#D71921" },
  light: { on: "#111111", dim: "#E2E2E2", red: "#D71921" },
};

// Small seeded random so the files do not change on every run.
let seed = 7;
const rnd = () => ((seed = (seed * 16807) % 2147483647) - 1) / 2147483646;
const f = (n) => Math.round(n * 100) / 100;

/** The mark: a 5x5 grid whose lit dots stand as a sorted staircase, the last one red. */
function markDots(x0, y0, pitch) {
  const dots = [];
  for (let col = 0; col < 5; col++) {
    for (let row = 0; row < 5; row++) {
      const height = col + 1; // column heights 1..5, sorted
      const lit = row >= 5 - height;
      const red = col === 4 && row === 0;
      dots.push({ x: x0 + col * pitch + pitch / 2, y: y0 + row * pitch + pitch / 2, lit, red, col, row });
    }
  }
  return dots;
}

function textDots(text, x0, y0, pitch) {
  const dots = [];
  [...text].forEach((ch, ci) => {
    GLYPHS[ch].forEach((bits, row) => {
      for (let col = 0; col < 5; col++) {
        const lit = (bits & (1 << (4 - col))) !== 0;
        dots.push({ x: x0 + (ci * 6 + col) * pitch + pitch / 2, y: y0 + row * pitch + pitch / 2, lit, col: ci * 6 + col, row });
      }
    });
  });
  return dots;
}

const STYLE = `
.d{animation:fall .9s cubic-bezier(.2,.9,.25,1) both;animation-delay:var(--t)}
.r{animation:drop .8s cubic-bezier(.3,1.45,.5,1) both;animation-delay:var(--t)}
.p{animation:pulse 2.6s ease-in-out 3.4s infinite}
@keyframes fall{from{opacity:0;transform:translate(var(--x),var(--y))}to{opacity:1;transform:none}}
@keyframes drop{from{opacity:0;transform:translateY(-70px)}to{opacity:1;transform:none}}
@keyframes pulse{50%{opacity:.3}}
@media (prefers-reduced-motion:reduce){.d,.r,.p{animation:none}}`;

function wordmark(theme) {
  const t = THEMES[theme];
  seed = 7;
  const pitch = 12;
  const pad = 6;
  const r = pitch * 0.36;
  const mark = markDots(pad, pad + 2 * pitch, pitch); // rows 2..6, on the baseline of the text
  const word = textDots("SWORTA", pad + 7 * pitch, pad, pitch);
  const w = pad * 2 + (7 + 35) * pitch;
  const h = pad * 2 + 7 * pitch;

  let grid = "";
  let lit = "";
  for (const d of [...mark, ...word]) {
    grid += `<circle cx="${f(d.x)}" cy="${f(d.y)}" r="${f(r * 0.5)}"/>`;
  }
  // The staircase builds column by column, then the word sorts itself in.
  for (const d of mark.filter((d) => d.lit && !d.red)) {
    const tdel = 0.1 + d.col * 0.09 + (4 - d.row) * 0.03;
    lit += `<circle class="d" style="--x:0px;--y:-${f(30 + rnd() * 30)}px;--t:${f(tdel)}s" cx="${f(d.x)}" cy="${f(d.y)}" r="${f(r)}"/>`;
  }
  for (const d of word.filter((d) => d.lit)) {
    const tdel = 0.55 + d.col * 0.035 + rnd() * 0.25;
    const x = (rnd() - 0.5) * 90;
    const y = -(40 + rnd() * 70);
    lit += `<circle class="d" style="--x:${f(x)}px;--y:${f(y)}px;--t:${f(tdel)}s" cx="${f(d.x)}" cy="${f(d.y)}" r="${f(r)}"/>`;
  }
  const rd = mark.find((d) => d.red);
  const red = `<g class="p"><circle class="r" style="--t:2.4s" cx="${f(rd.x)}" cy="${f(rd.y)}" r="${f(r)}" fill="${t.red}"/></g>`;

  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${w} ${h}" width="${w}" height="${h}" role="img" aria-labelledby="t">
<title id="t">SWORTA</title>
<style>${STYLE}</style>
<g fill="${t.dim}">${grid}</g>
<g fill="${t.on}">${lit}</g>
${red}
</svg>
`;
}

function logo(theme, size) {
  const t = THEMES[theme];
  const pitch = size / 5;
  const r = pitch * 0.36;
  const dots = markDots(0, 0, pitch);
  const body = dots
    .map((d) => {
      const fill = d.red ? t.red : d.lit ? t.on : t.dim;
      return `<circle cx="${f(d.x)}" cy="${f(d.y)}" r="${f(d.lit ? r : r * 0.5)}" fill="${fill}"/>`;
    })
    .join("");
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${size} ${size}" width="${size}" height="${size}" role="img" aria-labelledby="t">
<title id="t">SWORTA</title>
${body}
</svg>
`;
}

for (const theme of Object.keys(THEMES)) {
  writeFileSync(join(out, `wordmark-${theme}.svg`), wordmark(theme));
  writeFileSync(join(out, `logo-${theme}.svg`), logo(theme, 120));
}
console.log("written to", out);
