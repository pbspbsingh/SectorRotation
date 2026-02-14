// ─────────────────────────────────────────────────────────────────────────────
// STATE
// ─────────────────────────────────────────────────────────────────────────────

const API = ''; // empty = same origin
let currentLayer = 'sector';
let currentTab = 'rrg';
let currentTimeframe = 'weekly';
let selectedTicker = null;
let drillSector = null; // when set, center shows only this sector's industry groups
let expandedSectors = new Set(); // tracks which sectors are expanded in the tree

let universe = [];
let rrgData = [];
let rankData = [];
let zData = [];

const QUAD = {
  Leading: { color: '#00E676', bg: '#004D26' },
  Weakening: { color: '#FF9800', bg: '#4A2900' },
  Lagging: { color: '#FF1744', bg: '#4A0012' },
  Improving: { color: '#2979FF', bg: '#00204A' },
};

const TICKER_COLORS = [
  '#00D4FF', '#00E676', '#FF9800', '#2979FF', '#FF1744',
  '#FFD600', '#E040FB', '#00B0FF', '#69F0AE', '#FF6D00',
  '#F50057', '#00BFA5', '#AA00FF', '#FFAB40', '#40C4FF',
  '#B2FF59', '#FF4081', '#18FFFF', '#EEFF41', '#FF6E40',
  '#EA80FC', '#84FFFF', '#CCFF90',
];

function tickerColor(ticker) {
  const hash = [...ticker].reduce((a, c) => a + c.charCodeAt(0), 0);
  return TICKER_COLORS[hash % TICKER_COLORS.length];
}

// ─────────────────────────────────────────────────────────────────────────────
// DRILLDOWN
// ─────────────────────────────────────────────────────────────────────────────

/** Set of child industry tickers for the drilled sector, or null. */
function drilledChildren() {
  if (!drillSector) return null;
  const sec = universe.find(s => s.ticker === drillSector);
  return sec ? new Set(sec.children.map(c => c.ticker)) : null;
}

/** Filter an array of {ticker,...} to only the drilled sector's children. */
function filterToDrill(arr) {
  const children = drilledChildren();
  return children ? arr.filter(e => children.has(e.ticker)) : arr;
}

// Active data for renders — filtered when drilled in
function visibleRRG() { return filterToDrill(rrgData); }
function visibleRank() { return filterToDrill(rankData); }
function visibleZ() { return filterToDrill(zData); }

/** API layer to fetch — always 'industry' while drilled in. */
function effectiveLayer() {
  return drillSector ? 'industry' : currentLayer;
}

function drillInto(sectorTicker) {
  const sec = universe.find(s => s.ticker === sectorTicker);
  if (!sec || sec.children.length === 0) {
    // No industry groups defined — just open detail
    selectTicker(sectorTicker);
    return;
  }
  drillSector = sectorTicker;
  selectedTicker = sectorTicker;
  updateBreadcrumb();
  loadAll();
  loadDetail(sectorTicker);
}

function drillBack() {
  drillSector = null;
  selectedTicker = null;
  updateBreadcrumb();
  loadAll();
}

function updateBreadcrumb() {
  const bar = document.getElementById('breadcrumb-bar');
  const btnS = document.getElementById('btn-sector');
  const btnI = document.getElementById('btn-industry');

  if (!drillSector) {
    bar.classList.remove('visible');
    btnS.disabled = false;
    btnI.disabled = false;
    return;
  }

  bar.classList.add('visible');
  const sec = universe.find(s => s.ticker === drillSector);
  const name = sec ? sec.name : drillSector;
  const count = sec ? sec.children.length : 0;
  document.getElementById('breadcrumb-label').textContent = `${drillSector}  —  ${name}`;
  document.getElementById('breadcrumb-count').textContent = `${count} INDUSTRY GROUP${count !== 1 ? 'S' : ''}`;

  // Disable layer toggle while drilled (forced to industry)
  btnS.disabled = true;
  btnI.disabled = true;
}

// ─────────────────────────────────────────────────────────────────────────────
// API
// ─────────────────────────────────────────────────────────────────────────────

async function apiFetch(path) {
  const r = await fetch(API + '/api' + path);
  if (!r.ok) throw new Error(`API ${path} returned ${r.status}`);
  return r.json();
}

async function apiPost(path) {
  const r = await fetch(API + '/api' + path, { method: 'POST' });
  if (!r.ok) throw new Error(`API POST ${path} returned ${r.status}`);
  return r.json();
}

async function refreshData() {
  setLoading(true, 'FETCHING MARKET DATA...');
  document.getElementById('refresh-btn').disabled = true;
  try {
    const resp = await apiPost('/refresh');
    if (!resp.ok) throw new Error(resp.error || 'Refresh failed');
    setLoading(true, 'COMPUTING SIGNALS...');
    await loadAll();
    setStatus(`${resp.data.tickers_fetched} tickers loaded`, true);
  } catch (e) {
    setStatus('Error: ' + e.message, false);
    console.error(e);
  }
  setLoading(false);
  document.getElementById('refresh-btn').disabled = false;
}

async function loadAll() {
  const layer = effectiveLayer();
  try {
    const tfParam = '&timeframe=' + currentTimeframe;
    const [univ, rrg, rank, z] = await Promise.all([
      apiFetch('/universe'),
      apiFetch('/rrg?layer=' + layer + tfParam),
      apiFetch('/rankings?layer=' + layer + tfParam),
      apiFetch('/zscore?layer=' + layer + tfParam),
    ]);
    if (univ.ok) universe = univ.data.sectors;
    if (rrg.ok) rrgData = rrg.data;
    if (rank.ok) rankData = rank.data;
    if (z.ok) zData = z.data;
  } catch (e) {
    console.error('loadAll failed:', e);
    setStatus('Error loading data: ' + e.message, false);
    return;
  }

  renderTree();
  renderRRG();
  renderHeatmap();
  renderZScore();
  if (selectedTicker) loadDetail(selectedTicker);
}

async function loadDetail(ticker) {
  selectedTicker = ticker;
  try {
    const resp = await apiFetch('/detail/' + ticker + '?timeframe=' + currentTimeframe);
    if (resp.ok) renderDetail(resp.data);
  } catch (e) {
    console.error('loadDetail failed:', e);
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// CONTROLS
// ─────────────────────────────────────────────────────────────────────────────

function setLayer(layer) {
  currentLayer = layer;
  drillSector = null;
  document.getElementById('btn-sector').classList.toggle('active', layer === 'sector');
  document.getElementById('btn-industry').classList.toggle('active', layer === 'industry');
  updateBreadcrumb();
  if (rrgData.length > 0 || rankData.length > 0) loadAll();
}

function setTimeframe(tf) {
  currentTimeframe = tf;
  document.getElementById('btn-daily').classList.toggle('active', tf === 'daily');
  document.getElementById('btn-weekly').classList.toggle('active', tf === 'weekly');
  if (rrgData.length > 0 || rankData.length > 0) loadAll();
}

function switchTab(tab) {
  currentTab = tab;
  document.querySelectorAll('.tab').forEach((el, i) => {
    el.classList.toggle('active', ['rrg', 'heatmap', 'zscore'][i] === tab);
  });
  document.querySelectorAll('.tab-content').forEach((el, i) => {
    el.classList.toggle('active', ['tab-rrg', 'tab-heatmap', 'tab-zscore'][i] === 'tab-' + tab);
  });
  if (tab === 'rrg' && (rrgData.length || drillSector)) renderRRG();
}

function setLoading(show, msg = '') {
  document.getElementById('loading-overlay').classList.toggle('show', show);
  if (msg) document.getElementById('loading-msg').textContent = msg;
}

function setStatus(msg, live) {
  document.getElementById('status-text').textContent = msg;
  document.getElementById('status-dot').className = 'status-dot' + (live ? ' live' : '');
}

// ─────────────────────────────────────────────────────────────────────────────
// LEFT TREE
// ─────────────────────────────────────────────────────────────────────────────

function quadrantForTicker(ticker) {
  const e = rrgData.find(e => e.ticker === ticker);
  return e ? e.quadrant : null;
}

function renderTree() {
  const container = document.getElementById('tree');

  if (currentLayer === 'sector') {
    // ── Sector view: flat list of sectors ──
    container.innerHTML = universe.map(sec => {
      const quad = quadrantForTicker(sec.ticker);
      const dot = quad ? `<div class="quad-dot" style="background:${QUAD[quad]?.color || '#555'}"></div>` : '';
      return `
        <div class="tree-sector">
          <div class="tree-sector-header ${selectedTicker === sec.ticker ? 'selected' : ''}"
               onclick="toggleAndSelect('${sec.ticker}')">
            <span class="tree-sector-ticker">${sec.ticker}</span>
            <span class="tree-sector-name">${sec.name}</span>
            ${dot}
          </div>
        </div>`;
    }).join('');

  } else {
    // ── Industry layer view: sectors as collapsible groups with filtering ──
    container.innerHTML = universe.map(sec => {
      if (sec.children.length === 0) return '';
      const isFiltered = drillSector === sec.ticker;
      const children = sec.children.map(ind => {
        const quad = quadrantForTicker(ind.ticker);
        const dot = quad ? `<div class="quad-dot" style="background:${QUAD[quad]?.color || '#555'};width:5px;height:5px"></div>` : '';
        return `
          <div class="tree-industry ${selectedTicker === ind.ticker ? 'selected' : ''}"
               onclick="selectTicker('${ind.ticker}')">
            <span style="color:var(--text-dim);font-size:10px">${ind.ticker}</span>
            <span style="flex:1;font-size:10px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${ind.name}</span>
            ${dot}
          </div>`;
      }).join('');
      return `
        <div class="tree-sector">
          <div class="tree-sector-header ${isFiltered ? 'selected' : ''}" onclick="toggleFilter('${sec.ticker}')">
            <span class="tree-arrow ${expandedSectors.has(sec.ticker) ? 'open' : ''}">▶</span>
            <span class="tree-sector-ticker">${sec.ticker}</span>
            <span class="tree-sector-name">${sec.name}</span>
          </div>
          <div class="tree-children ${expandedSectors.has(sec.ticker) ? 'open' : ''}">${children}</div>
        </div>`;
    }).join('');
  }
}

function toggleSector(header) {
  header.querySelector('.tree-arrow').classList.toggle('open');
  header.nextElementSibling.classList.toggle('open');
}

function toggleFilter(sectorTicker) {
  if (expandedSectors.has(sectorTicker)) {
    expandedSectors.delete(sectorTicker);
  } else {
    expandedSectors.clear();
    expandedSectors.add(sectorTicker);
  }
  // Toggle chart filter
  if (drillSector === sectorTicker) {
    drillSector = null;
  } else {
    drillSector = sectorTicker;
  }
  renderTree();
  renderRRG();
  renderHeatmap();
  renderZScore();
}

function toggleAndSelect(ticker) {
  if (selectedTicker === ticker) {
    selectedTicker = null;
    renderTree();
    document.getElementById('detail-panel').innerHTML =
      '<div class="detail-placeholder"><div style="font-size:24px;opacity:0.3">◎</div><div>Click any sector or industry group<br>to view signals</div></div>';
  } else {
    selectedTicker = ticker;
    renderTree();
    loadDetail(ticker);
  }
}

// Unified ticker selection — used by both sector and industry clicks (#14)
function selectTicker(ticker) {
  if (selectedTicker === ticker) {
    selectedTicker = null;
    renderTree();
    document.getElementById('detail-panel').innerHTML =
      '<div class="detail-placeholder"><div style="font-size:24px;opacity:0.3">◎</div><div>Click any sector or industry group<br>to view signals</div></div>';
  } else {
    selectedTicker = ticker;
    renderTree();
    loadDetail(ticker);
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// RRG (D3)
// ─────────────────────────────────────────────────────────────────────────────

function renderRRG() {
  const svg = d3.select('#rrg-svg');
  svg.selectAll('*').remove();

  const el = document.getElementById('rrg-container');
  const W = el.clientWidth || el.getBoundingClientRect().width;
  const H = el.clientHeight || el.getBoundingClientRect().height;

  if (W <= 0 || H <= 0) return;

  svg.attr('width', W).attr('height', H);

  const data = visibleRRG();

  if (!data.length) {
    svg.append('text').attr('x', '50%').attr('y', '50%')
      .attr('text-anchor', 'middle').attr('fill', '#2A3A4A')
      .attr('font-family', 'IBM Plex Mono').attr('font-size', '12')
      .text('No data — click REFRESH DATA');
    return;
  }

  const margin = { top: 30, right: 30, bottom: 50, left: 60 };
  const width = W - margin.left - margin.right;
  const height = H - margin.top - margin.bottom;

  const allRatios = data.flatMap(d => d.tail.map(t => t.rsRatio));
  const allMoms = data.flatMap(d => d.tail.map(t => t.rsMomentum));
  const pad = 1.5;
  const xDom = [Math.min(...allRatios) - pad, Math.max(...allRatios) + pad];
  const yDom = [Math.min(...allMoms) - pad, Math.max(...allMoms) + pad];
  xDom[0] = Math.min(xDom[0], 98.5); xDom[1] = Math.max(xDom[1], 101.5);
  yDom[0] = Math.min(yDom[0], 98.5); yDom[1] = Math.max(yDom[1], 101.5);

  const xScale = d3.scaleLinear().domain(xDom).range([0, width]);
  const yScale = d3.scaleLinear().domain(yDom).range([height, 0]);
  const g = svg.append('g').attr('transform', `translate(${margin.left},${margin.top})`);

  const cx = xScale(100), cy = yScale(100);

  // Quadrant backgrounds + labels
  [
    { x: 0, y: 0, w: cx, h: cy, color: QUAD.Improving.color, label: 'IMPROVING', lx: cx / 2, ly: cy / 2 },
    { x: cx, y: 0, w: width - cx, h: cy, color: QUAD.Leading.color, label: 'LEADING', lx: cx + (width - cx) / 2, ly: cy / 2 },
    { x: 0, y: cy, w: cx, h: height - cy, color: QUAD.Lagging.color, label: 'LAGGING', lx: cx / 2, ly: cy + (height - cy) / 2 },
    { x: cx, y: cy, w: width - cx, h: height - cy, color: QUAD.Weakening.color, label: 'WEAKENING', lx: cx + (width - cx) / 2, ly: cy + (height - cy) / 2 },
  ].forEach(q => {
    g.append('rect').attr('x', q.x).attr('y', q.y).attr('width', q.w).attr('height', q.h).attr('fill', q.color).attr('opacity', 0.06);
    g.append('text').attr('x', q.lx).attr('y', q.ly).attr('text-anchor', 'middle').attr('dominant-baseline', 'middle')
      .attr('fill', q.color).attr('opacity', 0.35).attr('font-size', 11)
      .attr('font-family', 'IBM Plex Mono').attr('font-weight', 600).attr('letter-spacing', '0.1em').text(q.label);
  });

  // Center lines
  g.append('line').attr('x1', 0).attr('x2', width).attr('y1', cy).attr('y2', cy).attr('stroke', '#1E2A38').attr('stroke-width', 1);
  g.append('line').attr('x1', cx).attr('x2', cx).attr('y1', 0).attr('y2', height).attr('stroke', '#1E2A38').attr('stroke-width', 1);

  // Axes
  g.append('g').attr('transform', `translate(0,${height})`).call(d3.axisBottom(xScale).ticks(6).tickFormat(d3.format('.1f')))
    .selectAll('text,line,path').attr('stroke', '#2A3A4A').attr('fill', '#4A6070').attr('font-family', 'IBM Plex Mono').attr('font-size', '9');
  g.append('g').call(d3.axisLeft(yScale).ticks(6).tickFormat(d3.format('.1f')))
    .selectAll('text,line,path').attr('stroke', '#2A3A4A').attr('fill', '#4A6070').attr('font-family', 'IBM Plex Mono').attr('font-size', '9');

  g.append('text').attr('x', width / 2).attr('y', height + 40).attr('text-anchor', 'middle').attr('fill', '#4A6070')
    .attr('font-family', 'IBM Plex Mono').attr('font-size', '10').attr('letter-spacing', '0.08em').text('RS-RATIO  →  relative strength vs SPY');
  g.append('text').attr('transform', 'rotate(-90)').attr('x', -height / 2).attr('y', -46)
    .attr('text-anchor', 'middle').attr('fill', '#4A6070').attr('font-family', 'IBM Plex Mono').attr('font-size', '10')
    .attr('letter-spacing', '0.08em').text('RS-MOMENTUM  →  acceleration');

  // Clicking any dot selects that ticker for the detail panel
  data.forEach(entry => {
    const color = tickerColor(entry.ticker);
    const tail = entry.tail;
    if (!tail || !tail.length) return;

    // Fading tail lines
    for (let j = 1; j < tail.length; j++) {
      const alpha = 0.12 + 0.55 * (j / tail.length);
      g.append('line')
        .attr('x1', xScale(tail[j - 1].rsRatio)).attr('y1', yScale(tail[j - 1].rsMomentum))
        .attr('x2', xScale(tail[j].rsRatio)).attr('y2', yScale(tail[j].rsMomentum))
        .attr('stroke', color).attr('stroke-width', 1.5).attr('opacity', alpha);
    }

    const ex = xScale(entry.current.rsRatio);
    const ey = yScale(entry.current.rsMomentum);

    const onClick = () => { selectTicker(entry.ticker); };

    g.append('circle').attr('cx', ex).attr('cy', ey).attr('r', 5)
      .attr('fill', color).attr('stroke', '#0D1117').attr('stroke-width', 1.5)
      .style('cursor', 'pointer')
      .on('click', onClick)
      .on('mouseenter', function () { d3.select(this).attr('r', 7); })
      .on('mouseleave', function () { d3.select(this).attr('r', 5); });

    const labelX = ex > width * 0.85 ? ex - 8 : ex + 8;
    const anchor = ex > width * 0.85 ? 'end' : 'start';
    g.append('text').attr('x', labelX).attr('y', ey + 1)
      .attr('dominant-baseline', 'middle').attr('text-anchor', anchor)
      .attr('fill', color).attr('font-size', 10).attr('font-family', 'IBM Plex Mono').attr('font-weight', 600)
      .style('cursor', 'pointer').on('click', onClick).text(entry.ticker);
  });
}

// ─────────────────────────────────────────────────────────────────────────────
// RANK HEATMAP
// ─────────────────────────────────────────────────────────────────────────────

let heatmapSortCol = null;  // current sort column key
let heatmapSortAsc = true;  // ascending?

function renderHeatmap() {
  const container = document.getElementById('heatmap-content');
  const data = visibleRank();
  if (!data.length) { container.innerHTML = ''; return; }

  const n = rankData.length; // use full population for color scaling, not filtered count

  // Sort data if a column is selected
  const sortedData = [...data];
  if (heatmapSortCol) {
    const key = heatmapSortCol;
    sortedData.sort((a, b) => {
      let va = a[key], vb = b[key];
      if (typeof va === 'string') {
        va = va.toLowerCase(); vb = vb.toLowerCase();
        return heatmapSortAsc ? va.localeCompare(vb) : vb.localeCompare(va);
      }
      return heatmapSortAsc ? va - vb : vb - va;
    });
  }

  function rankColor(rank) {
    const t = (rank - 1) / Math.max(n - 1, 1);
    if (t < 0.33) return { bg: `rgba(0,230,118,${0.15 + 0.5 * (1 - t * 3)})`, text: '#fff' };
    if (t < 0.67) return { bg: `rgba(255,214,0,0.15)`, text: '#FFD600' };
    return { bg: `rgba(255,23,68,${0.15 + 0.5 * ((t - 0.67) * 3)})`, text: '#fff' };
  }

  function trendArrow(t) {
    if (t === 'rising') return '<span class="trend-up">↑↑</span>';
    if (t === 'falling') return '<span class="trend-down">↓↓</span>';
    return '<span class="trend-flat">—</span>';
  }

  function sortIcon(col) {
    if (heatmapSortCol !== col) return '<span style="opacity:0.3;font-size:8px"> ⇅</span>';
    return heatmapSortAsc
      ? '<span style="color:var(--cyan);font-size:8px"> ▲</span>'
      : '<span style="color:var(--cyan);font-size:8px"> ▼</span>';
  }

  function thClick(col) {
    return `onclick="sortHeatmap('${col}')"`;
  }

  const rows = sortedData.map(r => {
    const c20 = rankColor(r.rank20d);
    const c63 = rankColor(r.rank63d);
    const c126 = rankColor(r.rank126d);
    return `
      <tr>
        <td class="heatmap-ticker" style="cursor:pointer" onclick="selectTicker('${r.ticker}')">${r.ticker}</td>
        <td class="heatmap-name" style="cursor:pointer" onclick="selectTicker('${r.ticker}')">${r.name}</td>
        <td><div class="heatmap-cell" style="background:${c20.bg};color:${c20.text}">${r.rank20d}</div></td>
        <td><div class="heatmap-cell" style="background:${c63.bg};color:${c63.text}">${r.rank63d}</div></td>
        <td><div class="heatmap-cell" style="background:${c126.bg};color:${c126.text}">${r.rank126d}</div></td>
        <td style="text-align:center;font-size:11px">${r.rankChange > 0 ? '+' : ''}${r.rankChange}</td>
        <td style="text-align:center">${trendArrow(r.trend)}</td>
        <td style="text-align:right;padding-right:14px;font-size:10px;color:${r.relRet20d >= 0 ? '#00E676' : '#FF1744'}">${r.relRet20d > 0 ? '+' : ''}${r.relRet20d.toFixed(1)}%</td>
      </tr>`;
  }).join('');

  container.innerHTML = `
    <table class="heatmap-table">
      <thead>
        <tr>
          <th>TICKER</th><th style="text-align:center;padding-left:14px">NAME</th>
          <th style="cursor:pointer" ${thClick('rank20d')}>1M RANK${sortIcon('rank20d')}</th>
          <th style="cursor:pointer" ${thClick('rank63d')}>3M RANK${sortIcon('rank63d')}</th>
          <th style="cursor:pointer" ${thClick('rank126d')}>6M RANK${sortIcon('rank126d')}</th>
          <th style="cursor:pointer" ${thClick('rankChange')}>ΔRANK${sortIcon('rankChange')}</th>
          <th style="cursor:pointer" ${thClick('trend')}>TREND${sortIcon('trend')}</th>
          <th style="cursor:pointer;text-align:right;padding-right:14px" ${thClick('relRet20d')}>REL RET 1M${sortIcon('relRet20d')}</th>
        </tr>
      </thead>
      <tbody>${rows}</tbody>
    </table>`;
}

function sortHeatmap(col) {
  if (heatmapSortCol === col) {
    heatmapSortAsc = !heatmapSortAsc;
  } else {
    heatmapSortCol = col;
    heatmapSortAsc = col === 'rank20d' || col === 'rank63d' || col === 'rank126d';
  }
  renderHeatmap();
}

// ─────────────────────────────────────────────────────────────────────────────
// Z-SCORE BARS
// ─────────────────────────────────────────────────────────────────────────────

function renderZScore() {
  const container = document.getElementById('zscore-content');
  const data = visibleZ();
  if (!data.length) { container.innerHTML = ''; return; }

  const ZMAX = 3;

  function barFill(z) {
    const pct = Math.min(Math.abs(z), ZMAX) / ZMAX * 50;
    return z >= 0
      ? { left: '50%', width: pct + '%', color: '#00E676' }
      : { left: (50 - pct) + '%', width: pct + '%', color: '#FF1744' };
  }

  function signalBadge(sig) {
    const styles = {
      leader: 'background:#004D26;color:#00E676',
      improving: 'background:#00204A;color:#2979FF',
      reverting: 'background:#4A2900;color:#FF9800',
      lagging: 'background:#4A0012;color:#FF1744',
      neutral: 'background:#1E2A38;color:#4A6070',
    };
    return `<span class="signal-badge" style="${styles[sig] || styles.neutral}">${sig.toUpperCase()}</span>`;
  }

  function bar(z, label) {
    const f = barFill(z);
    const t1 = 50 + (1.5 / ZMAX) * 50;
    const t2 = 50 - (1.5 / ZMAX) * 50;
    return `
      <div class="zscore-bar-row">
        <div class="zscore-bar-label">${label}</div>
        <div class="zscore-bar-track">
          <div class="zscore-center-line"></div>
          <div class="zscore-threshold" style="left:${t1}%"></div>
          <div class="zscore-threshold" style="left:${t2}%"></div>
          <div class="zscore-fill" style="left:${f.left};width:${f.width};background:${f.color};opacity:0.8"></div>
        </div>
        <div style="font-size:10px;width:36px;text-align:right;color:${z >= 0 ? '#00E676' : '#FF1744'}">${z > 0 ? '+' : ''}${z.toFixed(2)}</div>
      </div>`;
  }

  container.innerHTML = data.map(entry => `
    <div class="zscore-row" onclick="selectTicker('${entry.ticker}')">
      <div class="zscore-header">
        <div class="zscore-name">
          <span style="color:${tickerColor(entry.ticker)};font-weight:600">${entry.ticker}</span>
          <span style="color:var(--text-dim);margin-left:8px">${entry.name}</span>
        </div>
        ${signalBadge(entry.signal)}
      </div>
      <div class="zscore-bars">
        ${bar(entry.zShort, '1M')}
        ${bar(entry.zLong, '3M')}
      </div>
    </div>`).join('');
}

// ─────────────────────────────────────────────────────────────────────────────
// DETAIL PANEL
// ─────────────────────────────────────────────────────────────────────────────

function renderDetail(data) {
  const panel = document.getElementById('detail-panel');
  const { rrg, rank, zscore, convergence, priceHistory } = data;

  if (!rrg && !rank && !zscore) {
    panel.innerHTML = `<div class="detail-placeholder"><div>No signal data for ${data.ticker}</div></div>`;
    return;
  }

  function quadBadge(q) {
    const s = QUAD[q] || { color: '#555', bg: '#222' };
    return `<span class="quad-badge" style="background:${s.bg};color:${s.color}">${q}</span>`;
  }

  function confBar(conf) {
    const pcts = { high: 100, medium: 66, low: 33 };
    const colors = { high: '#00E676', medium: '#FFD600', low: '#FF9800' };
    return `
      <div class="confidence-bar">
        <div class="confidence-label">
          <span>CONVERGENCE CONFIDENCE</span>
          <span style="color:${colors[conf] || '#555'};font-weight:600">${(conf || 'low').toUpperCase()}</span>
        </div>
        <div class="confidence-track">
          <div class="confidence-fill" style="width:${pcts[conf] || 0}%;background:${colors[conf] || '#555'}"></div>
        </div>
      </div>`;
  }

  function miniChart(history) {
    if (!history || history.length < 2) return '';
    const ratios = history.map(p => p.ratio);
    const minR = Math.min(...ratios), maxR = Math.max(...ratios);
    const rangeR = maxR - minR || 0.001;
    const W = 240, H = 80;
    const pts = ratios.map((r, i) =>
      `${(i / (ratios.length - 1)) * W},${H - ((r - minR) / rangeR) * H}`
    ).join(' ');
    const color = ratios[ratios.length - 1] >= ratios[0] ? '#00E676' : '#FF1744';
    return `
      <div class="detail-card">
        <div class="detail-card-title">Relative Strength vs SPY (normalized)</div>
        <svg viewBox="0 0 ${W} ${H}" style="width:100%;height:80px;overflow:visible">
          <polyline points="${pts}" fill="none" stroke="${color}" stroke-width="1.5" opacity="0.8"/>
          <line x1="0" y1="${H / 2}" x2="${W}" y2="${H / 2}" stroke="#1E2A38" stroke-width="0.5" stroke-dasharray="3,3"/>
        </svg>
      </div>`;
  }

  let html = `
    <div class="detail-ticker-header">
      <div class="detail-ticker">${data.ticker}</div>
      <div class="detail-name">${data.name}</div>
    </div>`;

  if (rrg) html += `
    <div class="detail-card">
      <div class="detail-card-title">RRG Signal</div>
      <div class="kv-grid">
        <div class="kv"><div class="kv-key">QUADRANT</div><div class="kv-val">${quadBadge(rrg.quadrant)}</div></div>
        <div class="kv"><div class="kv-key">RS-RATIO</div><div class="kv-val" style="color:${rrg.current.rsRatio >= 100 ? '#00E676' : '#FF1744'}">${rrg.current.rsRatio.toFixed(2)}</div></div>
        <div class="kv"><div class="kv-key">RS-MOMENTUM</div><div class="kv-val" style="color:${rrg.current.rsMomentum >= 100 ? '#00E676' : '#FF1744'}">${rrg.current.rsMomentum.toFixed(2)}</div></div>
        <div class="kv"><div class="kv-key">TAIL</div><div class="kv-val" style="font-size:11px;color:var(--text-dim)">${rrg.tail.length} weeks</div></div>
      </div>
    </div>`;

  if (rank) {
    const cc = rank.rankChange > 0 ? '#00E676' : rank.rankChange < 0 ? '#FF1744' : '#4A6070';
    html += `
    <div class="detail-card">
      <div class="detail-card-title">RS Rankings</div>
      <div class="kv-grid">
        <div class="kv"><div class="kv-key">RANK 1M</div><div class="kv-val">#${rank.rank20d}</div></div>
        <div class="kv"><div class="kv-key">RANK 3M</div><div class="kv-val">#${rank.rank63d}</div></div>
        <div class="kv"><div class="kv-key">RANK 6M</div><div class="kv-val">#${rank.rank126d}</div></div>
        <div class="kv"><div class="kv-key">RANK CHANGE</div><div class="kv-val" style="color:${cc}">${rank.rankChange > 0 ? '+' : ''}${rank.rankChange}</div></div>
        <div class="kv"><div class="kv-key">REL RET 1M</div><div class="kv-val" style="color:${rank.relRet20d >= 0 ? '#00E676' : '#FF1744'};font-size:12px">${rank.relRet20d > 0 ? '+' : ''}${rank.relRet20d.toFixed(2)}%</div></div>
        <div class="kv"><div class="kv-key">REL RET 3M</div><div class="kv-val" style="color:${rank.relRet63d >= 0 ? '#00E676' : '#FF1744'};font-size:12px">${rank.relRet63d > 0 ? '+' : ''}${rank.relRet63d.toFixed(2)}%</div></div>
      </div>
    </div>`;
  }

  if (zscore) {
    const sigColors = { leader: '#00E676', improving: '#2979FF', reverting: '#FF9800', lagging: '#FF1744', neutral: '#4A6070' };
    html += `
    <div class="detail-card">
      <div class="detail-card-title">Z-Score Momentum</div>
      <div class="kv-grid">
        <div class="kv"><div class="kv-key">Z-SCORE 1M</div><div class="kv-val" style="color:${zscore.zShort >= 0 ? '#00E676' : '#FF1744'}">${zscore.zShort > 0 ? '+' : ''}${zscore.zShort.toFixed(2)}</div></div>
        <div class="kv"><div class="kv-key">Z-SCORE 3M</div><div class="kv-val" style="color:${zscore.zLong >= 0 ? '#00E676' : '#FF1744'}">${zscore.zLong > 0 ? '+' : ''}${zscore.zLong.toFixed(2)}</div></div>
        <div class="kv" style="grid-column:1/-1"><div class="kv-key">SIGNAL</div><div class="kv-val" style="color:${sigColors[zscore.signal] || '#4A6070'};font-size:12px">${zscore.signal.toUpperCase()}</div></div>
      </div>
    </div>`;
  }

  if (convergence) html += `
    <div class="detail-card">
      <div class="detail-card-title">Signal Convergence</div>
      <div class="convergence-checklist">
        <div class="check-row">
          <div class="check-icon ${convergence.rrgSignal ? 'pass' : 'fail'}">${convergence.rrgSignal ? '✓' : '✗'}</div>
          <span>RRG: Improving/Leading + moving right</span>
        </div>
        <div class="check-row">
          <div class="check-icon ${convergence.rankSignal ? 'pass' : 'fail'}">${convergence.rankSignal ? '✓' : '✗'}</div>
          <span>RS Rank: Rising (1M improving)</span>
        </div>
        <div class="check-row">
          <div class="check-icon ${convergence.zscoreSignal ? 'pass' : 'fail'}">${convergence.zscoreSignal ? '✓' : '✗'}</div>
          <span>Z-Score: Leader / Improving / Reverting</span>
        </div>
      </div>
      ${confBar(convergence.confidence)}
    </div>`;

  html += miniChart(priceHistory);
  panel.innerHTML = html;
}

// ─────────────────────────────────────────────────────────────────────────────
// INIT
// ─────────────────────────────────────────────────────────────────────────────

async function init() {
  updateBreadcrumb(); // ensure breadcrumb hidden at start
  try {
    const status = await apiFetch('/status');
    if (status.ok && status.data.loaded) {
      setStatus(`${status.data.ticker_count} tickers`, true);
      await loadAll();
    } else {
      setStatus('No data — click Refresh', false);
      const univ = await apiFetch('/universe');
      if (univ.ok) { universe = univ.data.sectors; renderTree(); }
    }
  } catch (e) {
    setStatus('Cannot reach server', false);
    console.error('Init error:', e);
  }
}

window.addEventListener('resize', () => {
  if (currentTab === 'rrg' && (rrgData.length || drillSector)) renderRRG();
});

init();
