/*
 * eve-offline admin page: the cluster map and the pilots online.
 *
 * The map's view transform ({x, y, k}, world in 0..1), wheel-zoom-at-cursor
 * and drag-to-pan are a plain canvas renderer; no library. Data comes from
 * the server's /api/* with the X-Admin-Token header; nothing here holds
 * client data.
 */
(function () {
  'use strict';

  const POLL_MS = 3000;
  const TOKEN_KEY = 'eve-offline-admin-token';

  const $ = (id) => document.getElementById(id);
  let token = '';
  let map = null;          // {systems: [{id,name,region,sec,x,y}], byId, links}
  let pilots = [];
  let status = null;
  let selected = null;     // system id filter
  let sortKey = 'system';
  let timer = null;

  // ------------------------------------------------------------ storage
  function loadToken() {
    try { return sessionStorage.getItem(TOKEN_KEY) || localStorage.getItem(TOKEN_KEY) || ''; }
    catch (e) { return ''; }
  }
  function saveToken(t, remember) {
    try {
      sessionStorage.setItem(TOKEN_KEY, t);
      if (remember) localStorage.setItem(TOKEN_KEY, t); else localStorage.removeItem(TOKEN_KEY);
    } catch (e) { /* private mode: keep it in memory only */ }
  }
  function clearToken() {
    try { sessionStorage.removeItem(TOKEN_KEY); localStorage.removeItem(TOKEN_KEY); } catch (e) { /* */ }
  }

  // ---------------------------------------------------------------- api
  async function api(path) {
    const r = await fetch(path, { headers: { 'X-Admin-Token': token }, cache: 'no-store' });
    if (r.status === 401) throw new Error('token refused');
    if (!r.ok) throw new Error(path + ': HTTP ' + r.status);
    return r.json();
  }

  function setStatus(text, bad) {
    const el = $('status');
    el.textContent = text;
    el.classList.toggle('bad', !!bad);
  }

  async function connect() {
    try {
      const m = await api('/api/map');
      map = buildMap(m);
      renderer.fit();
      $('login').classList.add('hidden');
      $('logout').classList.remove('hidden');
      await poll();
    } catch (e) {
      setStatus(String(e.message || e), true);
      $('login').classList.remove('hidden');
    }
  }

  async function poll() {
    clearTimeout(timer);
    try {
      const [s, p] = await Promise.all([api('/api/status'), api('/api/pilots')]);
      status = s;
      pilots = p.pilots || [];
      showStatus();
      renderTable();
      renderer.dirty = true;
    } catch (e) {
      setStatus(String(e.message || e), true);
      if (String(e.message) === 'token refused') { $('login').classList.remove('hidden'); return; }
    }
    timer = setTimeout(poll, POLL_MS);
  }

  // ------------------------------------------------------------ actions
  // POST /api/action/<name>, JSON body, the same token header. The server
  // refuses any other Content-Type or a foreign Origin.
  async function act(name, body) {
    const out = $('answer');
    out.classList.remove('bad');
    out.textContent = name + ' ...';
    try {
      const r = await fetch('/api/action/' + name, {
        method: 'POST',
        headers: { 'X-Admin-Token': token, 'Content-Type': 'application/json' },
        body: JSON.stringify(body || {}),
        cache: 'no-store',
      });
      const j = await r.json().catch(() => ({}));
      if (r.ok) out.textContent = name + ': ' + (j.text || 'ok');
      else { out.classList.add('bad'); out.textContent = name + ': ' + (j.error || 'HTTP ' + r.status); }
    } catch (e) {
      out.classList.add('bad');
      out.textContent = name + ': ' + String(e.message || e);
    }
  }

  function setupActions() {
    const val = (f, n) => f.elements[n].value.trim();
    $('f-spawn').addEventListener('submit', (e) => {
      e.preventDefault();
      const f = e.target;
      // /bots spawn <group> <side> <role> <count> <skills> <ship> [system]
      const words = ['spawn', val(f, 'group'), val(f, 'side'), val(f, 'role'), val(f, 'count'),
        'role', val(f, 'ship')];
      if (val(f, 'system')) words.push(val(f, 'system'));
      act('bots', { words: words.join(' ') });
    });
    $('f-despawn').addEventListener('submit', (e) => {
      e.preventDefault();
      act('bots', { words: 'despawn ' + val(e.target, 'group') });
    });
    $('b-list').addEventListener('click', () => act('bots', { words: 'list' }));
    $('f-broadcast').addEventListener('submit', (e) => {
      e.preventDefault();
      act('broadcast', { text: val(e.target, 'text') });
    });
    $('b-save').addEventListener('click', () => act('save', {}));
  }

  function fmtUptime(s) {
    const d = Math.floor(s / 86400), h = Math.floor(s / 3600) % 24, m = Math.floor(s / 60) % 60;
    return (d ? d + 'd ' : '') + h + 'h ' + m + 'm';
  }

  function showStatus() {
    const s = status;
    if (!s) return;
    const parts = ['up ' + fmtUptime(s.uptime_s)];
    if (s.snapshot_age_s === null) parts.push('no snapshot yet');
    else parts.push(s.humans + ' humans, ' + s.bots + ' bots (' + s.in_space + ' in space, '
      + s.docked + ' docked) in ' + s.systems + ' systems, ' + s.npcs + ' NPCs');
    if (s.tick) parts.push('tick p95 ' + s.tick.p95_ms + ' ms, held max ' + s.tick.held_max_ms
      + ' ms' + (s.tick.skipped ? ', skipped ' + s.tick.skipped : ''));
    const stale = s.snapshot_age_s !== null && s.snapshot_age_s > 15;
    setStatus(parts.join(' | ') + (stale ? ' | snapshot ' + s.snapshot_age_s + ' s old' : ''), stale);
  }

  // ---------------------------------------------------------------- map
  function buildMap(m) {
    // Rows: [id, name, region, security, x_ly, z_ly]. Screen y is -z [A].
    const rows = m.systems || [];
    let x0 = Infinity, x1 = -Infinity, y0 = Infinity, y1 = -Infinity;
    for (const r of rows) {
      x0 = Math.min(x0, r[4]); x1 = Math.max(x1, r[4]);
      y0 = Math.min(y0, -r[5]); y1 = Math.max(y1, -r[5]);
    }
    const span = Math.max(x1 - x0, y1 - y0) || 1;
    const systems = rows.map(r => ({
      id: r[0], name: r[1], region: r[2], sec: r[3],
      x: (r[4] - x0) / span, y: (-r[5] - y0) / span,
    }));
    const byId = new Map(systems.map(s => [s.id, s]));
    return { systems, byId, links: m.links || [] };
  }

  // Security status to colour.
  function secColor(sec) {
    if (sec >= 0.5) return '#3fb950';
    if (sec >= 0.1) return '#c09840';
    return '#c05c5c';
  }

  const renderer = {
    canvas: null, ctx: null, t: { x: 0, y: 0, k: 1 }, dirty: true, hover: null,

    init() {
      this.canvas = $('map');
      this.ctx = this.canvas.getContext('2d');
      window.addEventListener('resize', () => this.resize());
      this.resize();
      this.interaction();
      const loop = () => { if (this.dirty) { this.draw(); this.dirty = false; } requestAnimationFrame(loop); };
      requestAnimationFrame(loop);
    },

    resize() {
      const p = this.canvas.parentElement;
      if (!p.clientWidth || !p.clientHeight) return;
      this.canvas.width = p.clientWidth;
      this.canvas.height = p.clientHeight;
      this.dirty = true;
    },

    fit() {
      const w = this.canvas.width, h = this.canvas.height;
      const k = Math.min(w, h) * 0.92;
      this.t = { x: (w - k) / 2, y: (h - k) / 2, k };
      this.dirty = true;
    },

    toScreen(wx, wy) { return [wx * this.t.k + this.t.x, wy * this.t.k + this.t.y]; },

    occupancy() {
      const occ = new Map();
      for (const p of pilots) {
        if (p.system_id == null) continue;
        const o = occ.get(p.system_id) || { humans: 0, bots: 0, space: 0, names: [] };
        if (p.bot) o.bots++; else o.humans++;
        if (p.state === 'space') o.space++;
        if (o.names.length < 6) o.names.push(p.name);
        occ.set(p.system_id, o);
      }
      return occ;
    },

    draw() {
      const ctx = this.ctx, W = this.canvas.width, H = this.canvas.height;
      ctx.fillStyle = '#0d1117';
      ctx.fillRect(0, 0, W, H);
      if (!map) return;
      // Gate links.
      ctx.strokeStyle = 'rgba(139,148,158,0.18)';
      ctx.lineWidth = 1;
      ctx.beginPath();
      for (const [a, b] of map.links) {
        const sa = map.byId.get(a), sb = map.byId.get(b);
        if (!sa || !sb) continue;
        const [ax, ay] = this.toScreen(sa.x, sa.y);
        const [bx, by] = this.toScreen(sb.x, sb.y);
        if ((ax < 0 && bx < 0) || (ax > W && bx > W) || (ay < 0 && by < 0) || (ay > H && by > H)) continue;
        ctx.moveTo(ax, ay); ctx.lineTo(bx, by);
      }
      ctx.stroke();
      // Systems, small, by security.
      const r0 = Math.max(1.5, Math.min(4, this.t.k / 1200));
      for (const s of map.systems) {
        const [sx, sy] = this.toScreen(s.x, s.y);
        if (sx < -4 || sy < -4 || sx > W + 4 || sy > H + 4) continue;
        ctx.fillStyle = secColor(s.sec);
        ctx.globalAlpha = 0.8;
        ctx.fillRect(sx - r0 / 2, sy - r0 / 2, r0, r0);
      }
      ctx.globalAlpha = 1;
      // Occupied systems: a dot sized by count, humans amber, bots cyan.
      const occ = this.occupancy();
      ctx.font = '11px system-ui, sans-serif';
      ctx.textBaseline = 'middle';
      for (const [id, o] of occ) {
        const s = map.byId.get(id);
        if (!s) continue;
        const [sx, sy] = this.toScreen(s.x, s.y);
        const n = o.humans + o.bots;
        const r = 4 + Math.min(10, Math.sqrt(n) * 2);
        ctx.beginPath();
        ctx.arc(sx, sy, r, 0, Math.PI * 2);
        ctx.fillStyle = o.humans ? 'rgba(255,180,0,0.85)' : 'rgba(0,212,255,0.75)';
        ctx.fill();
        if (selected === id) {
          ctx.strokeStyle = '#ffffff'; ctx.lineWidth = 2; ctx.stroke();
        }
        const label = s.name + ' ' + (o.humans ? o.humans + 'h ' : '') + (o.bots ? o.bots + 'b' : '');
        ctx.lineWidth = 3; ctx.strokeStyle = '#0d1117';
        ctx.strokeText(label, sx + r + 3, sy);
        ctx.fillStyle = '#e6edf3';
        ctx.fillText(label, sx + r + 3, sy);
      }
    },

    hit(px, py) {
      if (!map) return null;
      let best = null, bestD = 12 * 12;
      const occ = this.occupancy();
      for (const s of map.systems) {
        const [sx, sy] = this.toScreen(s.x, s.y);
        let d = (sx - px) ** 2 + (sy - py) ** 2;
        if (occ.has(s.id)) d /= 4; // occupied dots win ties
        if (d < bestD) { bestD = d; best = s; }
      }
      return best;
    },

    // Pan and zoom.
    interaction() {
      const el = this.canvas;
      let drag = null, moved = false;
      el.addEventListener('mousedown', e => {
        drag = { sx: e.clientX, sy: e.clientY, tx: this.t.x, ty: this.t.y };
        moved = false;
      });
      window.addEventListener('mousemove', e => {
        if (!drag) return;
        const dx = e.clientX - drag.sx, dy = e.clientY - drag.sy;
        if (Math.hypot(dx, dy) > 3) moved = true;
        this.t.x = drag.tx + dx; this.t.y = drag.ty + dy;
        this.dirty = true;
      });
      window.addEventListener('mouseup', () => { drag = null; });
      el.addEventListener('mousemove', e => {
        if (drag) return;
        const rect = el.getBoundingClientRect();
        const s = this.hit(e.clientX - rect.left, e.clientY - rect.top);
        const tip = $('tip');
        if (!s) { tip.classList.add('hidden'); return; }
        const o = this.occupancy().get(s.id);
        tip.textContent = s.name + ' (' + s.sec.toFixed(1) + ')'
          + (o ? ': ' + o.humans + ' humans, ' + o.bots + ' bots, ' + o.space + ' in space'
            + (o.names.length ? ' - ' + o.names.join(', ') + (o.humans + o.bots > o.names.length ? ', ...' : '') : '') : '');
        tip.style.left = (e.clientX - rect.left + 14) + 'px';
        tip.style.top = (e.clientY - rect.top + 10) + 'px';
        tip.classList.remove('hidden');
      });
      el.addEventListener('mouseleave', () => $('tip').classList.add('hidden'));
      el.addEventListener('wheel', e => {
        e.preventDefault();
        const f = e.deltaY > 0 ? 0.85 : 1.18;
        const rect = el.getBoundingClientRect();
        const mx = e.clientX - rect.left, my = e.clientY - rect.top;
        this.t.x = mx - (mx - this.t.x) * f;
        this.t.y = my - (my - this.t.y) * f;
        this.t.k *= f;
        this.dirty = true;
      }, { passive: false });
      el.addEventListener('click', e => {
        if (moved) return;
        const rect = el.getBoundingClientRect();
        const s = this.hit(e.clientX - rect.left, e.clientY - rect.top);
        selected = s && selected !== s.id ? s.id : null;
        renderTable();
        this.dirty = true;
      });
    },
  };

  // -------------------------------------------------------------- table
  function rowOf(p) {
    return {
      p,
      name: p.name,
      kind: p.bot ? 'bot' : 'human',
      group: p.group || '',
      side: p.side || '',
      role: p.role || '',
      system: p.system || (p.system_id != null ? String(p.system_id) : ''),
      state: p.state,
      ship: p.ship || String(p.ship_type_id),
    };
  }

  function renderTable() {
    const q = $('filter').value.trim().toLowerCase();
    const showH = $('show-humans').checked, showB = $('show-bots').checked;
    const rows = pilots.map(rowOf).filter(r =>
      (r.p.bot ? showB : showH)
      && (selected === null || r.p.system_id === selected)
      && (!q || (r.name + ' ' + r.system + ' ' + r.ship + ' ' + r.state + ' ' + r.group + ' ' + r.side + ' ' + r.role).toLowerCase().includes(q)));
    rows.sort((a, b) => String(a[sortKey]).localeCompare(String(b[sortKey])) || a.name.localeCompare(b.name));
    const tbody = $('pilots').querySelector('tbody');
    const frag = document.createDocumentFragment();
    for (const r of rows) {
      const tr = document.createElement('tr');
      const where = r.state === 'space' && r.p.position
        ? 'space' : r.state + (r.p.station_id ? ' ' + r.p.station_id : '');
      const sideCls = r.side ? 'side-' + r.side.toLowerCase().replace(/[^a-z0-9]/g, '') : '';
      for (const [text, cls] of [[r.name, ''], [r.kind, r.kind], [r.group, ''], [r.side, sideCls],
        [r.role, ''], [r.system, ''], [where, ''], [r.ship, '']]) {
        const td = document.createElement('td');
        td.textContent = text;
        if (cls) td.className = cls;
        tr.appendChild(td);
      }
      if (r.p.position) tr.title = 'x ' + r.p.position[0] + ', y ' + r.p.position[1] + ', z ' + r.p.position[2] + ' m';
      tr.addEventListener('click', () => {
        selected = r.p.system_id;
        renderer.dirty = true;
        renderTable();
      });
      frag.appendChild(tr);
    }
    tbody.replaceChildren(frag);
  }

  // --------------------------------------------------------------- init
  function init() {
    renderer.init();
    setupActions();
    $('login').addEventListener('submit', e => {
      e.preventDefault();
      token = $('token').value.trim();
      saveToken(token, $('remember').checked);
      $('token').value = '';
      connect();
    });
    $('logout').addEventListener('click', () => {
      clearToken(); token = ''; clearTimeout(timer);
      $('logout').classList.add('hidden'); $('login').classList.remove('hidden');
      setStatus('not connected', false);
    });
    for (const id of ['filter', 'show-humans', 'show-bots']) $(id).addEventListener('input', renderTable);
    for (const th of document.querySelectorAll('#pilots th')) {
      th.addEventListener('click', () => { sortKey = th.dataset.key; renderTable(); });
    }
    token = loadToken();
    if (token) connect();
  }

  init();
})();
