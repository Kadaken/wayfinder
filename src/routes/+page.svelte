<script>
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount, tick } from "svelte";

  /** @type {Array<[string, string]>} */
  let places = $state([]);
  /** @type {Array<any>} */
  let drives = $state([]);
  /** @type {string} */
  let cwd = $state("");
  /** @type {Array<any>} */
  let entries = $state([]);
  /** @type {Set<string>} */
  let selectedSet = $state(new Set());
  /** @type {any} */
  let selected = $state(null);      // last-clicked, drives the preview
  let lastIndex = $state(-1);
  /** @type {string} */
  let previewSrc = $state("");
  /** @type {string} */
  let previewError = $state("");
  /** @type {string} */
  let previewText = $state("");
  /** @type {string} */
  let view = $state("details");
  /** @type {{multiple:boolean,directory:boolean,starting_dir?:string|null,filter?:string|null} | null} */
  let pickerMode = $state(null);
  /** @type {boolean} */
  let showHidden = $state(false);
  /** @type {string} */
  let addr = $state("");
  /** @type {Array<string>} */
  let history = $state([]);
  let hidx = $state(-1);
  /** @type {boolean} */
  let loading = $state(false);
  /** @type {string} */
  let search = $state("");
  let iconZoom = $state(1);
  let previewZoom = $state(1);
  const clamp = (v,a,b) => Math.max(a, Math.min(b, v));
  const AUDIO_EXTS = new Set(['mp3','flac','wav','ogg','m4a','aac','opus','wma','aiff','alac']);
  function fmtTime(s) { if (!s || isNaN(s)) return '0:00'; return `${Math.floor(s/60)}:${String(Math.floor(s%60)).padStart(2,'0')}`; }
  function toggleAudio() { if (audioEl) audioPlaying ? audioEl.pause() : audioEl.play(); }
  function pickerEligiblePaths() {
    if (!pickerMode) return [];
    const selectedPaths = selectedSet.size ? [...selectedSet] : (selected ? [selected.path] : []);
    const eligible = rows
      .filter((entry) => selectedPaths.includes(entry.path))
      .filter((entry) => pickerMode.directory ? entry.is_dir : !entry.is_dir)
      .map((entry) => entry.path);
    if (!pickerMode.multiple) {
      if (selected && eligible.includes(selected.path)) return [selected.path];
      return eligible.slice(0, 1);
    }
    return eligible;
  }
  async function pickerSubmit(paths = pickerEligiblePaths()) {
    if (!pickerMode || !paths.length) return;
    await invoke("picker_confirm", { paths });
  }
  async function pickerAbort() {
    if (!pickerMode) return;
    await invoke("picker_cancel");
  }
  function lassoStart(ev) {
    if (ev.button !== 0) return;
    if (ev.target.closest('tr, button.cell, thead, .createbar, .empty, input, button')) return;
    if (!ev.ctrlKey) { selectedSet = new Set(); selected = null; }
    lasso = { x1: ev.clientX, y1: ev.clientY, x2: ev.clientX, y2: ev.clientY };
  }
  function lassoMove(ev) {
    if (!lasso) return;
    lasso = { ...lasso, x2: ev.clientX, y2: ev.clientY };
    if (!filesEl) return;
    const rl = Math.min(lasso.x1, lasso.x2), rr = Math.max(lasso.x1, lasso.x2);
    const rt = Math.min(lasso.y1, lasso.y2), rb = Math.max(lasso.y1, lasso.y2);
    const items = filesEl.querySelectorAll('[data-path]');
    const s = new Set();
    for (const item of items) {
      const r = item.getBoundingClientRect();
      if (r.left < rr && r.right > rl && r.top < rb && r.bottom > rt) s.add(item.dataset.path);
    }
    selectedSet = s;
  }
  function lassoEnd() { lasso = null; }
  function wheelFiles(ev){
    if (!ev.ctrlKey) return;
    ev.preventDefault();
    iconZoom = clamp(+(iconZoom * (ev.deltaY < 0 ? 1.12 : 0.9)).toFixed(3), 0.5, 4);
    try { localStorage.setItem("zf_zoom", String(iconZoom)); } catch {}
  }
  function wheelPreview(ev){
    if (!ev.ctrlKey) return;
    ev.preventDefault();
    previewZoom = clamp(+(previewZoom * (ev.deltaY < 0 ? 1.15 : 0.87)).toFixed(3), 0.25, 10);
  }
  /** @type {any} */
  let clipboard = $state(null);     // { mode:"copy"|"cut", paths:[] }
  /** @type {any} */
  let menu = $state(null);          // { x, y, onEntry }
  /** @type {HTMLDivElement | null} */
  let menuEl = $state(null);        // bound to the context-menu div, for edge-clamping
  /** @type {any} */
  let renaming = $state(null);      // path being renamed
  /** @type {string} */
  let renameVal = $state("");
  /** @type {boolean} */
  let creating = $state(false);
  /** @type {string} */
  let createVal = $state("");
  /** @type {string} */
  let toast = $state("");
  /** @type {Array<{name: string, path: string}>} */
  let favorites = $state([]);
  /** @type {Array<{name: string, path: string}>} */
  let recents = $state([]);
  let justNowMins = $state(2);
  let showSettings = $state(false);
  let nowTick = $state(Date.now());
  let colSize = $state(true);
  let colType = $state(true);
  let colDate = $state(true);
  let thumbsEnabled = $state(true);
  let thumbCache = $state({});
  let audioSrc = $state("");
  let audioPlaying = $state(false);
  let audioTime = $state(0);
  let audioDuration = $state(0);
  /** @type {HTMLAudioElement|null} */
  let audioEl = $state(null);
  /** @type {{x1:number,y1:number,x2:number,y2:number}|null} */
  let lasso = $state(null);
  /** @type {HTMLElement|null} */
  let filesEl = $state(null);

  const basename = (p) => p === "/" ? "/" : (p.replace(/\/+$/,"").split("/").pop() || p);
  const PLACE_ICONS = { Home:'home', Desktop:'desktop', Documents:'documents', Downloads:'downloads', Pictures:'pictures', Music:'music', Videos:'video', Trash:'trash' };
  const placeIcon = (name) => PLACE_ICONS[name] || null;
  function loadStore(){
    try { favorites = JSON.parse(localStorage.getItem("zf_favorites") || "[]"); } catch { favorites = []; }
    try { recents = JSON.parse(localStorage.getItem("zf_recents") || "[]"); } catch { recents = []; }
    try { const z = parseFloat(localStorage.getItem("zf_zoom")); if (z) iconZoom = z; } catch {}
    try { const p = JSON.parse(localStorage.getItem("zf_panes") || "null"); if (p){ sidebarW = p.sidebarW ?? sidebarW; previewW = p.previewW ?? previewW; showPreview = p.showPreview ?? true; } } catch {}
    try { const jn = parseInt(localStorage.getItem("zf_justnow") ?? "2"); justNowMins = isNaN(jn) ? 2 : jn; } catch {}
    try { colSize = localStorage.getItem("zf_colSize") !== "false"; } catch {}
    try { colType = localStorage.getItem("zf_colType") !== "false"; } catch {}
    try { colDate = localStorage.getItem("zf_colDate") !== "false"; } catch {}
    try { thumbsEnabled = localStorage.getItem("zf_thumbs") !== "false"; } catch {}
  }
  const saveFav = () => { try { localStorage.setItem("zf_favorites", JSON.stringify(favorites)); } catch {} };
  const saveRec = () => { try { localStorage.setItem("zf_recents", JSON.stringify(recents)); } catch {} };
  const saveSettings = () => { try { localStorage.setItem("zf_justnow", String(justNowMins)); } catch {} };
  const saveColVis = () => {
    try {
      localStorage.setItem("zf_colSize", String(colSize));
      localStorage.setItem("zf_colType", String(colType));
      localStorage.setItem("zf_colDate", String(colDate));
    } catch {}
  };
  const isFav = (path) => favorites.some(f => f.path === path);
  function addFavorite(path){ if (!isFav(path)){ favorites = [...favorites, { name: basename(path), path }]; saveFav(); flash("★ Added to Favorites"); } menu = null; }
  function removeFavorite(path){ favorites = favorites.filter(f => f.path !== path); saveFav(); menu = null; }
  function pushRecent(path){ if (recents.some(r => r.path === path)) return; recents = [{ name: basename(path), path }, ...recents].slice(0, 12); saveRec(); }
  /** @type {any} */
  let propsData = $state(null);
  /** @type {boolean} */
  let confirmDel = $state(false);
  // tabs
  /** @type {Array<{path: string, label: string, history: Array<string>, hidx: number}>} */
  let tabs = $state([{ path:"", label:"", history:[], hidx:-1 }]);
  let activeIdx = $state(0);
  // drag & drop
  /** @type {Array<string>} */
  let dragPaths = $state([]);
  /** @type {string} */
  let dropTarget = $state("");
  /** @type {HTMLDivElement | null} */
  let dragGhostEl = null;
  /** @type {{dir: string, at: number} | null} */
  let pendingExternalDrop = null;
  /** @type {{key: string, at: number} | null} */
  let lastNativeDrop = null;
  // open-with apps (loaded when right-clicking a file)
  /** @type {any} */
  let owApps = $state(null);
  /** @type {boolean} */
  let owOpen = $state(false);
  /** @type {boolean} */
  let owFlip = $state(false);       // submenu opens to the left instead of right (near screen edge)
  /** @type {HTMLDivElement | null} */
  let owFlyoutEl = $state(null);
  /** @type {ReturnType<typeof setTimeout> | null} */
  let owCloseTimer = null;
  // resizable panes + breadcrumb + preview toggle
  let sidebarW = $state(185);
  let previewW = $state(320);
  /** @type {boolean} */
  let showPreview = $state(true);
  /** @type {boolean} */
  let editingAddr = $state(false);
  /** @type {null | "sidebar" | "preview"} */
  let dragging = null;
  /** @type {number} */
  let navSeq = 0;

  const crumbs = $derived.by(() => {
    const parts = (cwd || "").split("/").filter(Boolean);
    const out = [{ name: "Computer", path: "/" }];
    let acc = "";
    for (const p of parts){ acc += "/" + p; out.push({ name: p, path: acc }); }
    return out;
  });
  function startResize(which, ev){ dragging = which; ev.preventDefault(); }
  function onResizeMove(ev){
    if (!dragging) return;
    if (dragging === "sidebar") sidebarW = clamp(ev.clientX, 120, 420);
    else if (dragging === "preview") previewW = clamp(window.innerWidth - ev.clientX, 180, 640);
  }
  const savePanes = () => { try { localStorage.setItem("zf_panes", JSON.stringify({ sidebarW, previewW, showPreview })); } catch {} };
  function endResize(){ if (dragging){ dragging = null; savePanes(); } }
  function togglePreview(){ showPreview = !showPreview; savePanes(); }
  function editAddr(){ editingAddr = true; queueMicrotask(()=>{ const el=document.getElementById("addr"); el?.focus(); el?.select(); }); }
  function crumbKey(ev, path){
    if (ev.key === "Enter" || ev.key === " "){
      ev.preventDefault();
      navigate(path);
    }
  }

  // folder tree
  /** @type {Array<any>} */
  let treeRoots = $state([]);
  /** @type {Record<string, Array<any>>} */
  let treeChildren = $state({});
  /** @type {Set<string>} */
  let treeExpanded = $state(new Set());
  /** @type {boolean} */
  let creatingFile = $state(false);
  let typeBuf = "";
  /** @type {ReturnType<typeof setTimeout> | null} */
  let typeTimer = null;

  const isArchive = (name) => /\.(zip|tar|tgz|7z)$|\.tar\.(gz|bz2|xz)$/i.test(name);

  async function loadTreeRoots(){
    const home = places[0]?.[1] || "/";
    try {
      const hk = await invoke("list_subdirs", { path: home, showHidden });
      treeRoots = [{ name: "Home", path: home, has_children: true }];
      treeChildren = { ...treeChildren, [home]: hk };
    } catch {}
  }
  async function toggleTree(node){
    const s = new Set(treeExpanded);
    if (s.has(node.path)) s.delete(node.path);
    else { s.add(node.path); if (!treeChildren[node.path]){ try { const kids = await invoke("list_subdirs", { path: node.path, showHidden }); treeChildren = { ...treeChildren, [node.path]: kids }; } catch {} } }
    treeExpanded = s;
  }
  const treeFlat = $derived.by(() => {
    const out = [];
    const walk = (node, depth) => { out.push({ ...node, depth }); if (treeExpanded.has(node.path)) for (const c of (treeChildren[node.path]||[])) walk(c, depth+1); };
    for (const r of treeRoots) walk(r, 0);
    return out;
  });

  async function compressSel(){ menu=null; if(!selectedSet.size) return; try { await invoke("compress_zip", { paths:[...selectedSet], destDir: cwd }); flash("Compressed to ZIP"); navigate(cwd,false); } catch(e){ flash("⚠ "+e); } }
  async function extractSel(path){ menu=null; try { await invoke("extract_archive", { path, destDir: cwd }); flash("Extracted"); navigate(cwd,false); } catch(e){ flash("⚠ "+e); } }
  function startCreateFile(){ creating=false; creatingFile=true; createVal="new.txt"; menu=null; }
  async function commitCreateFile(){ if (creatingFile && createVal.trim()){ try { await invoke("new_file", { parent: cwd, name: createVal.trim() }); } catch(e){ flash("⚠ "+e); } } creatingFile=false; navigate(cwd,false); }
  async function doEmptyTrash(){ menu=null; try { await invoke("empty_trash"); flash("Trash emptied"); navigate(cwd,false); } catch(e){ flash("⚠ "+e); } }

  function syncTab(){ tabs[activeIdx] = { path: cwd, label: basename(cwd) || "/", history: [...history], hidx }; tabs = tabs; }
  function newTab(path){ tabs = [...tabs, { path: path||cwd, label:"", history:[], hidx:-1 }]; activeIdx = tabs.length-1; history = []; hidx = -1; menu = null; navigate(path || cwd); }
  function switchTab(i){ if (i===activeIdx) return; syncTab(); activeIdx = i; const t = tabs[i]; history = [...t.history]; hidx = t.hidx; navigate(t.path, false); }
  function closeTab(i, ev){ if (ev) ev.stopPropagation(); if (tabs.length===1) return; const wasActive = i===activeIdx; tabs = tabs.filter((_,j)=>j!==i); if (activeIdx > i || activeIdx >= tabs.length) activeIdx = Math.max(0, activeIdx-1); if (wasActive){ const t = tabs[activeIdx]; history=[...t.history]; hidx=t.hidx; navigate(t.path,false); } }

  function hasExternalFiles(ev){ return Array.from(ev.dataTransfer?.types || []).includes("Files"); }
  function pathToFileUri(path){
    return "file://" + path.split("/").map(encodeURIComponent).join("/");
  }
  function clearCompactDragImage(){
    dragGhostEl?.remove();
    dragGhostEl = null;
  }
  function setCompactDragImage(ev, entry){
    if (!ev.dataTransfer?.setDragImage) return;
    clearCompactDragImage();
    const ghost = document.createElement("div");
    const count = dragPaths.length;
    const label = document.createElement("span");
    label.textContent = count === 1 ? basename(dragPaths[0]) : `${count} items`;
    if (count === 1 && entry && iconCache[entry.icon]) {
      const img = document.createElement("img");
      img.src = iconCache[entry.icon];
      Object.assign(img.style, {
        width: "18px",
        height: "18px",
        flex: "0 0 auto",
        objectFit: "contain"
      });
      ghost.appendChild(img);
    }
    ghost.appendChild(label);
    Object.assign(ghost.style, {
      position: "fixed",
      left: "-1000px",
      top: "-1000px",
      display: "flex",
      alignItems: "center",
      gap: "8px",
      maxWidth: "220px",
      overflow: "hidden",
      textOverflow: "ellipsis",
      whiteSpace: "nowrap",
      padding: "7px 10px",
      borderRadius: "8px",
      border: "1px solid rgba(255,255,255,.28)",
      background: "rgba(24,28,36,.94)",
      color: "#f5f7fb",
      font: "12px system-ui, sans-serif",
      boxShadow: "0 8px 22px rgba(0,0,0,.35)",
      pointerEvents: "none",
      zIndex: "2147483647"
    });
    document.body.appendChild(ghost);
    ev.dataTransfer.setDragImage(ghost, 12, 12);
    dragGhostEl = ghost;
  }
  function onDragStart(ev, e){
    if (!selectedSet.has(e.path)){ selectedSet = new Set([e.path]); selected = e; }
    dragPaths = [...selectedSet];
    ev.dataTransfer.effectAllowed = "copyMove";
    const uriList = dragPaths.map(pathToFileUri).join("\r\n");
    ev.dataTransfer.setData("text/plain", dragPaths.join("\n"));
    ev.dataTransfer.setData("text/uri-list", uriList);
    if (dragPaths.length === 1) {
      ev.dataTransfer.setData("DownloadURL", `application/octet-stream:${basename(dragPaths[0])}:${pathToFileUri(dragPaths[0])}`);
    }
    setCompactDragImage(ev, e);
  }
  function onDragEnd(){
    clearCompactDragImage();
    dragPaths = [];
    dropTarget = "";
  }
  function allowDrop(ev, isDir, path){
    if (dragPaths.length || hasExternalFiles(ev)){
      ev.preventDefault();
      ev.stopPropagation();
      ev.dataTransfer.dropEffect = dragPaths.length ? (ev.ctrlKey ? "copy" : "move") : "copy";
      dropTarget = isDir ? path : cwd;
    }
  }
  async function copyExternalPaths(paths, destDir){
    if (!paths.length || !destDir) return;
    try {
      await invoke("copy_paths", { srcs: paths, destDir });
      flash(`Copied ${paths.length}`);
      navigate(cwd, false);
    } catch(e) {
      flash("⚠ " + e);
    }
  }
  function alreadyHandledNativeDrop(paths, destDir){
    const key = `${destDir}\0${paths.slice().sort().join("\0")}`;
    const now = Date.now();
    const previous = lastNativeDrop;
    lastNativeDrop = { key, at: now };
    return !!previous && previous.key === key && now - previous.at < 1800;
  }
  function onDropFolder(ev, destDir){
    ev.preventDefault();
    ev.stopPropagation();
    destDir = destDir || cwd;
    dropTarget="";
    if (!dragPaths.length){
      pendingExternalDrop = { dir: destDir, at: Date.now() };
      return;
    }
    const srcs = dragPaths.filter(p => p !== destDir);
    dragPaths=[];
    if (!srcs.length) return;
    const op = ev.ctrlKey ? "copy_paths" : "move_paths";
    invoke(op, { srcs, destDir }).then(()=>{ flash(ev.ctrlKey?"Copied":"Moved"); navigate(cwd,false); }).catch(e=>flash("⚠ "+e));
  }
  function runOpenWith(id, path){ invoke("open_with", { appId:id, path }); menu=null; owApps=null; owOpen=false; }
  async function owEnter(ev){
    if (owCloseTimer) clearTimeout(owCloseTimer);
    const r = ev.currentTarget.getBoundingClientRect();
    // open to the right by default; flip left if the flyout would run off the right edge
    owFlip = (r.right + 212) > window.innerWidth - 8;
    owOpen = true;
    await tick();
    if (owFlyoutEl){                       // refine using the real rendered width
      const f = owFlyoutEl.getBoundingClientRect();
      if (!owFlip && f.right > window.innerWidth - 6) owFlip = true;
      else if (owFlip && f.left < 6) owFlip = false;
    }
  }
  function owLeave(){ owCloseTimer = setTimeout(() => { owOpen = false; }, 220); }

  function placeMenu(ev, path, isFav){ ev.preventDefault(); ev.stopPropagation(); owApps=null; menu = { x: ev.clientX, y: ev.clientY, place: { path, isFav } }; clampMenu(); }
  // Keep the context menu fully on-screen: if it would spill past the right/bottom edge, slide it back in.
  async function clampMenu(){
    await tick();
    if (!menu || !menuEl) return;
    const pad = 6, r = menuEl.getBoundingClientRect();
    let x = menu.x, y = menu.y;
    if (x + r.width  > window.innerWidth  - pad) x = Math.max(pad, window.innerWidth  - r.width  - pad);
    if (y + r.height > window.innerHeight - pad) y = Math.max(pad, window.innerHeight - r.height - pad);
    if (x !== menu.x || y !== menu.y) menu = { ...menu, x, y };
  }
  function newWindow(path){ invoke("new_window", { path }); menu = null; }
  async function openProps(path){ menu = null; try { propsData = await invoke("properties", { path }); } catch(e){ flash("⚠ " + e); } }
  function askDelete(){ menu = null; if (selectedSet.size) confirmDel = true; }
  async function delPerm(){ confirmDel = false; try { await invoke("delete_permanent", { paths: [...selectedSet] }); flash("Deleted"); navigate(cwd, false); } catch(e){ flash("⚠ " + e); } }

  const fmtSize = (n) => {
    const u = ["B","KB","MB","GB","TB"]; let i=0;
    while (n >= 1024 && i < u.length-1){ n/=1024; i++; }
    return i===0 ? `${n} B` : `${n.toFixed(1)} ${u[i]}`;
  };
  function fmtDate(s) {
    if (!s) return "";
    void nowTick; // reactive — re-runs each minute tick
    const date = new Date(s * 1000);
    const now = new Date();
    const diffMs = now - date;
    const diffMins = diffMs / 60000;
    if (justNowMins > 0 && diffMins >= 0 && diffMins < justNowMins) return "Just now";
    const timeStr = date.toLocaleString('en-US', { hour: 'numeric', minute: '2-digit', hour12: true });
    const todayStart = new Date(now.getFullYear(), now.getMonth(), now.getDate());
    const yesterdayStart = new Date(todayStart.getTime() - 86400000);
    const fileDay = new Date(date.getFullYear(), date.getMonth(), date.getDate());
    if (fileDay.getTime() === todayStart.getTime()) {
      if (diffMins < 60) return `${Math.max(1, Math.round(diffMins))} min ago`;
      const h = Math.floor(diffMins / 60);
      if (h < 12) return `${h} hr ago`;
      return `Today, ${timeStr}`;
    }
    if (fileDay.getTime() === yesterdayStart.getTime()) return `Yesterday, ${timeStr}`;
    if (diffMs < 7 * 86400000) {
      const day = date.toLocaleDateString('en-US', { weekday: 'short' });
      return `${day}, ${timeStr}`;
    }
    if (date.getFullYear() === now.getFullYear()) {
      const d = date.toLocaleDateString('en-US', { month: 'short', day: 'numeric' });
      return `${timeStr} · ${d}`;
    }
    const d = date.toLocaleDateString('en-US', { month: 'short', day: 'numeric', year: 'numeric' });
    return `${timeStr} · ${d}`;
  }
  const flash = (m) => { toast = m; setTimeout(() => toast = "", 2600); };
  function copyAddr(){
    const t = cwd;
    (navigator.clipboard?.writeText(t) ?? Promise.reject())
      .then(() => flash("📋 Copied path"))
      .catch(() => flash("⚠ copy failed"));
  }

  /** @type {string} */
  let sortKey = $state("name");
  let sortDir = $state(1);
  /** @type {Record<string, string>} */
  let iconCache = $state({});

  const filtered = $derived(
    search.trim()
      ? entries.filter(e => e.name.toLowerCase().includes(search.toLowerCase()))
      : entries
  );

  function typeLabel(e){
    if (e.is_dir) return "Folder";
    const ext = e.name.includes('.') ? e.name.split('.').pop().toUpperCase() : "";
    const cat = {
      MP3:"Audio",WAV:"Audio",FLAC:"Audio",OGG:"Audio",M4A:"Audio",AAC:"Audio",OPUS:"Audio",WMA:"Audio",
      MP4:"Video",MKV:"Video",WEBM:"Video",MOV:"Video",AVI:"Video",M4V:"Video",
      PNG:"Image",JPG:"Image",JPEG:"Image",GIF:"Image",WEBP:"Image",SVG:"Image",BMP:"Image",
      PDF:"Document",DOC:"Document",DOCX:"Document",ODT:"Document",
      XLS:"Spreadsheet",XLSX:"Spreadsheet",ODS:"Spreadsheet",CSV:"Spreadsheet",
      ZIP:"Archive",TAR:"Archive",GZ:"Archive","7Z":"Archive",RAR:"Archive",
      TXT:"Text",MD:"Text",LOG:"Text"
    }[ext];
    return ext ? `${ext}${cat ? ' '+cat : ' File'}` : "File";
  }

  const rows = $derived.by(() => {
    const r = [...filtered];
    const k = sortKey, d = sortDir;
    r.sort((a,b) => {
      if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
      let av, bv;
      if (k==="size"){ av=a.size; bv=b.size; }
      else if (k==="modified"){ av=a.modified; bv=b.modified; }
      else if (k==="type"){ av=typeLabel(a).toLowerCase(); bv=typeLabel(b).toLowerCase(); }
      else { av=a.name.toLowerCase(); bv=b.name.toLowerCase(); }
      return (av<bv?-1:av>bv?1:0)*d;
    });
    return r;
  });

  const selSize = $derived(
    entries.filter(e=>selectedSet.has(e.path) && !e.is_dir).reduce((s,e)=>s+e.size,0)
  );

  function setSort(k){ if (sortKey===k) sortDir=-sortDir; else { sortKey=k; sortDir=1; } }
  const arrow = (k) => sortKey===k ? (sortDir>0 ? ' ▲' : ' ▼') : '';

  async function ensureIcon(name){
    if (!name || name in iconCache) return;
    iconCache = { ...iconCache, [name]: "" };
    try { const d = await invoke("icon_svg", { name }); iconCache = { ...iconCache, [name]: d }; }
    catch {}
  }

  async function refreshDrives() { try { drives = await invoke("list_drives"); } catch {} }

  async function navigate(path, record = true) {
    if (!path) return;
    const seq = ++navSeq;
    loading = true; menu = null; renaming = null; creating = false;
    try {
      const nextEntries = await invoke("list_dir", { path, showHidden });
      if (seq !== navSeq) return;
      entries = nextEntries;
      for (const nm of new Set(entries.map(e=>e.icon))) ensureIcon(nm);
      cwd = path; addr = path; search = "";
      selectedSet = new Set(); selected = null; lastIndex = -1;
      previewSrc = ""; previewError = "";
      if (record) { history = [...history.slice(0, hidx+1), path]; hidx = history.length-1; pushRecent(path); }
      refreshDrives();
      syncTab();
      if (thumbsEnabled) {
        const imgEntries = entries.filter(e => e.is_image).slice(0, 60);
        thumbCache = {};
        for (const e of imgEntries) {
          invoke("read_data_url", { path: e.path })
            .then(url => { thumbCache = { ...thumbCache, [e.path]: url }; })
            .catch(() => {});
        }
      }
    } catch (e) { previewError = String(e); flash("⚠ " + e); }
    finally { if (seq === navSeq) loading = false; }
  }

  async function up() { navigate(await invoke("parent_dir", { path: cwd })); }
  function back(){ if (hidx>0){ hidx--; navigate(history[hidx], false); } }
  function fwd(){ if (hidx<history.length-1){ hidx++; navigate(history[hidx], false); } }

  async function openDrive(d) {
    if (d.mountpoint) { navigate(d.mountpoint); return; }
    flash("Mounting " + d.name + "…");
    try { const mp = await invoke("mount_drive", { device: d.path }); await refreshDrives(); navigate(mp || d.path); }
    catch (e) { flash("⚠ mount failed: " + e); }
  }
  function driveCtx(ev, d){ ev.preventDefault(); ev.stopPropagation(); owApps=null; owOpen=false; menu = { x: ev.clientX, y: ev.clientY, drive: d }; clampMenu(); }
  const canUnmount = (d) => d?.mountpoint && (d.removable || d.mountpoint.startsWith("/run/media"));
  async function unmountDrive(d){
    menu=null; flash("Unmounting " + d.name + "…");
    try { await invoke("unmount_drive", { device: d.path }); await refreshDrives(); flash("Unmounted " + d.name); }
    catch (e) { flash("⚠ unmount failed: " + e); }
  }
  async function ejectDrive(d){
    menu=null; flash("Ejecting " + d.name + "…");
    try { await invoke("eject_drive", { device: d.path }); await refreshDrives(); flash("✅ " + d.name + " — safe to remove"); }
    catch (e) { flash("⚠ eject failed: " + e); }
  }

  async function select(e, idx, ev) {
    if (ev?.ctrlKey) {
      const s = new Set(selectedSet);
      s.has(e.path) ? s.delete(e.path) : s.add(e.path);
      selectedSet = s;
    } else if (ev?.shiftKey && lastIndex >= 0) {
      const [a,b] = [Math.min(lastIndex, idx), Math.max(lastIndex, idx)];
      const s = new Set(selectedSet);
      for (let i=a;i<=b;i++) s.add(rows[i].path);
      selectedSet = s;
    } else {
      selectedSet = new Set([e.path]);
    }
    lastIndex = idx; selected = e;
    previewSrc = ""; previewError = ""; previewText = ""; audioSrc = ""; audioPlaying = false; audioTime = 0; audioDuration = 0; previewZoom = 1;
    const ext = e.name.split('.').pop()?.toLowerCase() ?? '';
    if (e.is_image) {
      try { previewSrc = await invoke("read_data_url", { path: e.path }); }
      catch (err) { previewError = String(err); }
    } else if (AUDIO_EXTS.has(ext)) {
      try { audioSrc = await invoke("read_data_url", { path: e.path }); }
      catch (err) { previewError = String(err); }
    } else if (!e.is_dir) {
      // try a text preview; binary/unreadable falls back to the file icon
      try { const t = await invoke("read_text_preview", { path: e.path }); if (selected?.path === e.path) previewText = t; }
      catch { /* not text — leave icon placeholder */ }
    }
  }

  function activate(e) {
    if (pickerMode) {
      if (pickerMode.directory && e.is_dir) {
        pickerSubmit([e.path]);
        return;
      }
      if (!pickerMode.directory && !e.is_dir) {
        pickerSubmit([e.path]);
        return;
      }
      if (e.is_dir) {
        navigate(e.path);
      }
      return;
    }
    if (e.is_dir) navigate(e.path);
    else invoke("open_path", { path: e.path });
  }

  // ---- file ops ----
  function doCopy(){ if (selectedSet.size) { clipboard = { mode:"copy", paths:[...selectedSet] }; flash(`Copied ${selectedSet.size}`); } }
  function doCut(){ if (selectedSet.size) { clipboard = { mode:"cut", paths:[...selectedSet] }; flash(`Cut ${selectedSet.size}`); } }
  async function paste(){
    if (!clipboard) return;
    try {
      if (clipboard.mode === "copy") await invoke("copy_paths", { srcs: clipboard.paths, destDir: cwd });
      else { await invoke("move_paths", { srcs: clipboard.paths, destDir: cwd }); clipboard = null; }
      navigate(cwd, false);
    } catch (e) { flash("⚠ " + e); }
  }
  async function del(){
    if (!selectedSet.size) return;
    try { await invoke("delete_paths", { paths:[...selectedSet] }); flash("Moved to Trash"); navigate(cwd, false); }
    catch (e) { flash("⚠ " + e); }
  }
  function startRename(e){ renaming = e.path; renameVal = e.name; menu = null; }
  async function commitRename(){
    if (renaming && renameVal.trim()) {
      try { await invoke("rename_path", { path: renaming, newName: renameVal.trim() }); }
      catch (e) { flash("⚠ " + e); }
    }
    renaming = null; navigate(cwd, false);
  }
  function startCreate(){ creatingFile=false; creating = true; createVal = "New Folder"; menu = null; }
  async function commitCreate(){
    if (creating && createVal.trim()) {
      try { await invoke("make_dir", { parent: cwd, name: createVal.trim() }); }
      catch (e) { flash("⚠ " + e); }
    }
    creating = false; navigate(cwd, false);
  }
  function termHere(){ invoke("open_terminal", { path: cwd }); menu = null; }

  function ctx(ev, e){
    ev.preventDefault();
    ev.stopPropagation();
    if (e && !selectedSet.has(e.path)) { selectedSet = new Set([e.path]); selected = e; }
    owApps = null; owOpen = false;
    menu = { x: ev.clientX, y: ev.clientY, onEntry: !!e, entry: e };
    clampMenu();
    if (e && !e.is_dir) invoke("open_with_apps", { path: e.path }).then(a => { const seen=new Set(); owApps = (a||[]).filter(x => !seen.has(x.name) && seen.add(x.name)); clampMenu(); }).catch(()=>{});
  }

  function onKey(ev){
    const inInput = document.activeElement?.tagName === "INPUT";
    if (inInput) return;
    if (ev.key === "Backspace") { ev.preventDefault(); back(); }
    else if (ev.altKey && ev.key === "ArrowLeft") { ev.preventDefault(); back(); }
    else if (ev.altKey && ev.key === "ArrowRight") { ev.preventDefault(); fwd(); }
    else if (ev.altKey && ev.key === "ArrowUp") { ev.preventDefault(); up(); }
    else if (ev.key === "Delete") del();
    else if (ev.key === "F2" && selected) startRename(selected);
    else if (ev.ctrlKey && ev.key === "c") doCopy();
    else if (ev.ctrlKey && ev.key === "x") doCut();
    else if (ev.ctrlKey && ev.key === "v") paste();
    else if (ev.ctrlKey && ev.key === "a") { ev.preventDefault(); selectedSet = new Set(rows.map(e=>e.path)); }
    else if (ev.ctrlKey && ev.key === "t") { ev.preventDefault(); newTab(cwd); }
    else if (ev.ctrlKey && ev.key === "w") { ev.preventDefault(); closeTab(activeIdx); }
    else if (ev.key === "Enter" && selected) activate(selected);
    else if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
      ev.preventDefault();
      const idx = selected ? rows.findIndex(r => r.path === selected.path) : -1;
      const n = clamp(ev.key === "ArrowDown" ? idx + 1 : idx - 1, 0, rows.length - 1);
      if (rows[n]) { select(rows[n], n, {}); queueMicrotask(()=>document.querySelector('tr.sel,.cell.sel')?.scrollIntoView({block:'nearest'})); }
    }
    else if (ev.key.length === 1 && !ev.ctrlKey && !ev.altKey && !ev.metaKey) {
      typeBuf += ev.key.toLowerCase();
      if (typeTimer) clearTimeout(typeTimer); typeTimer = setTimeout(()=>{ typeBuf = ""; }, 800);
      const i = rows.findIndex(r => r.name.toLowerCase().startsWith(typeBuf));
      if (i >= 0) { select(rows[i], i, {}); queueMicrotask(()=>document.querySelector('tr.sel,.cell.sel')?.scrollIntoView({block:'nearest'})); }
    }
    else if (ev.ctrlKey && ev.key === "l") { ev.preventDefault(); editAddr(); }
    else if (ev.key === "F5") navigate(cwd, false);
    else if (ev.key === "Escape") {
      if (pickerMode) {
        ev.preventDefault();
        pickerAbort();
        return;
      }
      menu = null; propsData = null; confirmDel = false; selectedSet = new Set();
    }
    else if (pickerMode && ev.key === "Enter") {
      const paths = pickerEligiblePaths();
      if (paths.length) {
        ev.preventDefault();
        pickerSubmit(paths);
      }
    }
  }

  onMount(() => {
    loadStore();
    /** @type {null | (() => void)} */
    let unlistenDrives = null;
    /** @type {Array<() => void>} */
    const nativeDropUnlisteners = [];
    (async () => {
      try { pickerMode = await invoke("picker_options"); } catch {}
      places = await invoke("standard_dirs");
      await refreshDrives();
      let sp = null;
      try { sp = await invoke("start_path"); } catch {}
      navigate(sp || places[0]?.[1] || "/");
      loadTreeRoots();

      // Hotplug: coalesce rapid udev bursts (one plug fires disk + partition events)
      /** @type {ReturnType<typeof setTimeout> | null} */
      let driveDebounce = null;
      unlistenDrives = await listen("drives-changed", () => {
        if (driveDebounce) clearTimeout(driveDebounce);
        driveDebounce = setTimeout(() => refreshDrives(), 400);
      });

      const nativeDropHandler = (event) => {
        const p = event.payload || {};
        if (p.type === "enter" || p.type === "over") {
          if (!dragPaths.length) dropTarget = cwd;
        } else if (p.type === "drop") {
          const paths = p.paths || [];
          const pending = pendingExternalDrop;
          const destDir = pending && Date.now() - pending.at < 1800 ? pending.dir : cwd;
          pendingExternalDrop = null;
          dropTarget = "";
          dragPaths = [];
          if (alreadyHandledNativeDrop(paths, destDir)) return;
          copyExternalPaths(paths, destDir);
        } else {
          pendingExternalDrop = null;
          dropTarget = "";
        }
      };
      try {
        const [{ getCurrentWindow }, { getCurrentWebview }] = await Promise.all([
          import("@tauri-apps/api/window"),
          import("@tauri-apps/api/webview")
        ]);
        nativeDropUnlisteners.push(await getCurrentWindow().onDragDropEvent(nativeDropHandler));
        nativeDropUnlisteners.push(await getCurrentWebview().onDragDropEvent(nativeDropHandler));
      } catch {}
    })();

    /** @type {(ev: KeyboardEvent) => void} */
    const keyHandler = (ev) => {
      onKey(ev);
    };
    window.addEventListener("keydown", keyHandler, true);
    const tickId = setInterval(() => { nowTick = Date.now(); }, 60000);
    return () => {
      unlistenDrives?.();
      nativeDropUnlisteners.forEach((fn) => fn());
      window.removeEventListener("keydown", keyHandler, true);
      clearInterval(tickId);
    };
  });
</script>

<svelte:window
  on:click={() => { menu = null; owApps = null; }}
  on:mousemove={(ev)=>{onResizeMove(ev);lassoMove(ev);}}
  on:mouseup={()=>{endResize();lassoEnd();}}
  on:dragover={(ev)=>{ if (dragPaths.length) allowDrop(ev, false, cwd); }}
  on:dragend={onDragEnd}
/>

<div class="app" oncontextmenu={(e)=>e.preventDefault()}>
  <div class="tabbar">
    {#each tabs as t, i}
      <div class="tab" class:active={i===activeIdx} onclick={()=>switchTab(i)}
           ondragover={(e)=>allowDrop(e, true, t.path)} ondrop={(e)=>onDropFolder(e, t.path)}
           title={t.path}>
        <span class="tablabel">{t.label || basename(t.path) || '/'}</span>
        {#if tabs.length>1}<button class="tabclose" onclick={(e)=>closeTab(i,e)}>×</button>{/if}
      </div>
    {/each}
    <button class="tabadd" onclick={()=>newTab(cwd)} title="New tab (Ctrl+T)">+</button>
  </div>
  <div class="toolbar">
    <button onclick={back} disabled={hidx<=0} title="Back">‹</button>
    <button onclick={fwd} disabled={hidx>=history.length-1} title="Forward">›</button>
    <button onclick={up} title="Up">↑</button>
    <button onclick={() => navigate(places[0]?.[1])} title="Home">⌂</button>
    {#if editingAddr}
      <input id="addr" class="addr" bind:value={addr}
             onkeydown={(e)=> { if(e.key==='Enter'){ navigate(addr.trim()); editingAddr=false; } else if(e.key==='Escape'){ editingAddr=false; } }}
             onblur={()=>editingAddr=false} spellcheck="false" />
    {:else}
      <div class="crumbs"
           onclick={(e)=>{ if (e.target === e.currentTarget) editAddr(); }}
           ondblclick={editAddr}
           oncontextmenu={(e)=>{ e.preventDefault(); e.stopPropagation();
             navigator.clipboard?.readText().then(t=>{ const p=t?.trim(); if(p&&p.startsWith('/')) navigate(p); else editAddr(); }).catch(()=>editAddr()); }}
           title="Click empty area or right-click to paste a path">
        {#each crumbs as c, i}
          {#if i>0}<span class="crumbsep">›</span>{/if}
          <span class="crumb" role="button" tabindex="0" onclick={()=>navigate(c.path)} onkeydown={(e)=>crumbKey(e, c.path)}>{c.name}</span>
        {/each}
        <span class="crumb-paste-hint">›</span>
      </div>
    {/if}
    <button onclick={copyAddr} title="Copy path">📋</button>
    <input class="search" placeholder="Search…" bind:value={search} />
    {#if pickerMode}
      <button onclick={pickerAbort} title="Cancel picker">Cancel</button>
      <button class="opt-btn active" disabled={!pickerEligiblePaths().length} onclick={() => pickerSubmit()}
              title={pickerMode.directory ? "Select folder" : "Select file"}>
        {pickerEligiblePaths().length ? `Select ${pickerEligiblePaths().length}` : (pickerMode.directory ? 'Select folder' : 'Select file')}
      </button>
    {/if}
    <button class:active={showPreview} onclick={togglePreview} title="Toggle preview pane">▭</button>
    <button class:active={view==='details'} onclick={()=>view='details'} title="Details">☰</button>
    <button class:active={view==='icons'} onclick={()=>view='icons'} title="Icons">▦</button>
    <button onclick={()=>navigate(cwd,false)} title="Refresh">⟳</button>
    <button class="opt-btn" onclick={()=>showSettings=true} title="Options">⚙ Options</button>
  </div>

  <div class="body">
    <aside class="sidebar" style="width:{sidebarW}px">
      {#if favorites.length}
        <div class="sec">★ Favorites</div>
        {#each favorites as f}
          <button class="place" class:sel={cwd===f.path} class:drop={dropTarget===f.path}
                  ondragover={(e)=>allowDrop(e,true,f.path)} ondrop={(e)=>onDropFolder(e,f.path)} ondragleave={()=>dropTarget=''}
                  onclick={()=>navigate(f.path)} oncontextmenu={(e)=>placeMenu(e,f.path,true)} title={f.path}>
            <span class="ic">★</span><span class="dname">{f.name}</span>
          </button>
        {/each}
      {/if}
      <div class="sec">Places</div>
      {#each places as p}
        <button class="place" class:sel={cwd===p[1]} class:drop={dropTarget===p[1]}
                ondragover={(e)=>allowDrop(e,true,p[1])} ondrop={(e)=>onDropFolder(e,p[1])} ondragleave={()=>dropTarget=''}
                onclick={()=>navigate(p[1])} oncontextmenu={(e)=>placeMenu(e,p[1],false)}>
          {#if placeIcon(p[0])}<img class="ic img" src={`/icons/${placeIcon(p[0])}.png`} alt="" />{:else}<span class="ic">📁</span>{/if}<span class="dname">{p[0]}</span>
        </button>
      {/each}
      <div class="sec">Devices</div>
      {#each drives as d}
        <button class="place drive" class:sel={cwd===d.mountpoint && d.mountpoint}
                onclick={()=>openDrive(d)} oncontextmenu={(e)=>driveCtx(e,d)} title={d.path}>
          <img class="ic img" src={`/icons/${d.kind==='gdrive' ? 'gdrive' : d.kind==='network' ? 'network' : 'storage'}.png`} alt="" />
          <span class="dname">{d.name}</span>
          {#if !d.mountpoint}<span class="badge">mount</span>{:else if d.size}<span class="dsz">{fmtSize(d.size)}</span>{/if}
        </button>
      {/each}
      <div class="sec">Folders</div>
      <div class="tree">
        {#each treeFlat as n}
          <div class="treerow" class:sel={cwd===n.path} class:drop={dropTarget===n.path}
               style="padding-left:{6 + n.depth*14}px"
               onclick={()=>{ if (n.has_children) toggleTree(n); navigate(n.path); }} oncontextmenu={(e)=>placeMenu(e,n.path,false)}
               ondragover={(e)=>allowDrop(e,true,n.path)} ondrop={(e)=>onDropFolder(e,n.path)} ondragleave={()=>dropTarget=''}>
            <span class="twist">{n.has_children ? (treeExpanded.has(n.path) ? '▾' : '▸') : ''}</span>
            <span class="ic">📁</span><span class="dname">{n.name}</span>
          </div>
        {/each}
      </div>
      {#if recents.length}
        <div class="sec">Recent</div>
        {#each recents.slice(0,10) as r}
          <button class="place recent" class:sel={cwd===r.path} class:drop={dropTarget===r.path}
                  ondragover={(e)=>allowDrop(e,true,r.path)} ondrop={(e)=>onDropFolder(e,r.path)} ondragleave={()=>dropTarget=''}
                  onclick={()=>navigate(r.path)} oncontextmenu={(e)=>placeMenu(e,r.path,false)} title={r.path}>
            <img class="ic img" src={`/icons/recent.png`} alt="" /><span class="dname">{r.name}</span>
          </button>
        {/each}
      {/if}
      <label class="hidden-toggle"><input type="checkbox" checked={showHidden} onchange={()=>{showHidden=!showHidden; navigate(cwd,false);}} /> Hidden files</label>
    </aside>

    <div class="splitter" onmousedown={(e)=>startResize('sidebar',e)}></div>

    <main class="files {view}" class:drop={dropTarget===cwd} style="--zoom:{iconZoom}" bind:this={filesEl}
          ondragover={(e)=>allowDrop(e,true,cwd)} ondrop={(e)=>onDropFolder(e,cwd)} ondragleave={()=>{ if (dropTarget===cwd) dropTarget=''; }}
          onmousedown={lassoStart} onwheel={wheelFiles} oncontextmenu={(e)=>ctx(e,null)}>
      {#if lasso}
        <div class="lasso" style="left:{Math.min(lasso.x1,lasso.x2)}px;top:{Math.min(lasso.y1,lasso.y2)}px;width:{Math.abs(lasso.x2-lasso.x1)}px;height:{Math.abs(lasso.y2-lasso.y1)}px"></div>
      {/if}
      {#if creating || creatingFile}
        <div class="createbar">{creatingFile ? '📄' : '📁'} <input autofocus bind:value={createVal}
             onkeydown={(e)=> e.key==='Enter' ? (creatingFile?commitCreateFile():commitCreate()) : e.key==='Escape' ? (creating=false,creatingFile=false) : null}
             onblur={creatingFile?commitCreateFile:commitCreate} /></div>
      {/if}
      {#if loading}
        <div class="empty">Loading…</div>
      {:else if filtered.length === 0}
        <div class="empty">{search ? 'No matches' : 'Empty folder'}</div>
      {:else if view === 'details'}
        <table>
          <thead><tr>
            <th class="sortable" onclick={()=>setSort('name')}>Name{arrow('name')}</th>
            {#if colSize}<th class="num sortable" onclick={()=>setSort('size')}>Size{arrow('size')}</th>{/if}
            {#if colType}<th class="sortable" onclick={()=>setSort('type')}>Type{arrow('type')}</th>{/if}
            {#if colDate}<th class="sortable" onclick={()=>setSort('modified')}>Modified{arrow('modified')}</th>{/if}
          </tr></thead>
          <tbody>
            {#each rows as e, i}
              <tr data-path={e.path} class:sel={selectedSet.has(e.path)} class:drop={dropTarget===e.path && e.is_dir}
                  draggable="true" ondragstart={(ev)=>onDragStart(ev,e)}
                  ondragend={onDragEnd}
                  ondragover={(ev)=>allowDrop(ev,e.is_dir,e.path)} ondrop={(ev)=>onDropFolder(ev,e.is_dir ? e.path : cwd)} ondragleave={()=>{ const target = e.is_dir ? e.path : cwd; if (dropTarget===target) dropTarget=''; }}
                  onclick={(ev)=>select(e,i,ev)} ondblclick={()=>activate(e)} oncontextmenu={(ev)=>ctx(ev,e)}>
                <td class="name">
                  {#if iconCache[e.icon]}<img class="ficon" src={iconCache[e.icon]} alt="" />{:else}<span class="ficon ph"></span>{/if}
                  {#if renaming===e.path}
                    <input class="rename" autofocus bind:value={renameVal}
                      onkeydown={(ev)=> ev.key==='Enter'?commitRename(): ev.key==='Escape'?(renaming=null):null}
                      onblur={commitRename} onclick={(ev)=>ev.stopPropagation()}
                      onmousedown={(ev)=>ev.stopPropagation()} ondragstart={(ev)=>ev.preventDefault()} />
                  {:else}{e.name}{/if}
                </td>
                {#if colSize}<td class="num">{e.is_dir ? '' : fmtSize(e.size)}</td>{/if}
                {#if colType}<td class="type">{typeLabel(e)}</td>{/if}
                {#if colDate}<td class="date">{fmtDate(e.modified)}</td>{/if}
              </tr>
            {/each}
          </tbody>
        </table>
      {:else}
        <div class="grid">
          {#each rows as e, i}
            <button class="cell" data-path={e.path} class:sel={selectedSet.has(e.path)} class:drop={dropTarget===e.path && e.is_dir}
                 draggable="true" ondragstart={(ev)=>onDragStart(ev,e)}
                 ondragend={onDragEnd}
                 ondragover={(ev)=>allowDrop(ev,e.is_dir,e.path)} ondrop={(ev)=>onDropFolder(ev,e.is_dir ? e.path : cwd)} ondragleave={()=>{ const target = e.is_dir ? e.path : cwd; if (dropTarget===target) dropTarget=''; }}
                 onclick={(ev)=>select(e,i,ev)} ondblclick={()=>activate(e)} oncontextmenu={(ev)=>ctx(ev,e)}>
              <div class="cellicon">
                {#if thumbsEnabled && thumbCache[e.path]}
                  <img class="thumb" src={thumbCache[e.path]} alt="" />
                {:else if iconCache[e.icon]}
                  <img src={iconCache[e.icon]} alt="" />
                {:else}
                  <span class="ph"></span>
                {/if}
              </div>
              <div class="cellname">{e.name}</div>
            </button>
          {/each}
        </div>
      {/if}
    </main>

    {#if showPreview}
    <div class="splitter" onmousedown={(e)=>startResize('preview',e)}></div>
    <aside class="preview" style="--pvzoom:{previewZoom}; width:{previewW}px" onwheel={wheelPreview}>
      {#if selected}
        {#if previewText}
          <pre class="pv-text">{previewText}</pre>
        {:else if audioSrc}
          <div class="pv-audio">
            <div class="pv-music-art">🎵</div>
            <div class="pv-audio-title">{selected.name.replace(/\.[^.]+$/, '')}</div>
            <div class="pv-audio-ext">{selected.name.split('.').pop()?.toUpperCase()}</div>
            <audio bind:this={audioEl}
                   src={audioSrc}
                   onplay={() => audioPlaying = true}
                   onpause={() => audioPlaying = false}
                   ontimeupdate={() => { audioTime = audioEl?.currentTime ?? 0; }}
                   onloadedmetadata={() => { audioDuration = audioEl?.duration ?? 0; }}></audio>
            <div class="pv-audio-controls">
              <button class="pv-play" onclick={toggleAudio}>{audioPlaying ? '⏸' : '▶'}</button>
              <div class="pv-seek-row">
                <span class="pv-time">{fmtTime(audioTime)}</span>
                <input type="range" class="pv-seek" min=0 max={audioDuration || 1} step=0.1
                       value={audioTime}
                       oninput={(e) => { if (audioEl) audioEl.currentTime = +e.target.value; }} />
                <span class="pv-time">{fmtTime(audioDuration)}</span>
              </div>
            </div>
          </div>
        {:else}
          <div class="pv-image">
            {#if previewSrc}<img src={previewSrc} alt={selected.name} />
            {:else if previewError}<div class="pv-placeholder">⚠ {previewError}</div>
            {:else}<div class="pv-placeholder big">{selected.is_dir ? '📁' : '📄'}</div>{/if}
          </div>
        {/if}
        <div class="pv-meta">
          <div class="pv-name">{selected.name}</div>
          <div>{selected.is_dir ? 'Folder' : (selected.path.split('.').pop().toUpperCase()+' file')}</div>
          {#if !selected.is_dir}<div>{fmtSize(selected.size)}</div>{/if}
          <div class="pv-date">{fmtDate(selected.modified)}</div>
        </div>
      {:else}<div class="pv-placeholder">Select a file</div>{/if}
    </aside>
    {/if}
  </div>

  <div class="status">
    {rows.length} items{selectedSet.size>1 ? `  ·  ${selectedSet.size} selected${selSize? ' ('+fmtSize(selSize)+')':''}` : selected ? `  ·  ${selected.name}` : ''}
    {#if clipboard}<span class="clip">  ·  {clipboard.mode==='cut'?'✂':'⧉'} {clipboard.paths.length} ready to paste</span>{/if}
  </div>

  {#if toast}<div class="toast">{toast}</div>{/if}

  {#if menu}
    <div class="ctxmenu" bind:this={menuEl} style="left:{menu.x}px; top:{menu.y}px" oncontextmenu={(e)=>e.preventDefault()}>
      {#if menu.drive}
        <!-- DRIVE menu (sidebar devices) -->
        <button onclick={()=>openDrive(menu.drive)}>{menu.drive.mountpoint ? 'Open' : 'Mount & Open'}</button>
        {#if canUnmount(menu.drive)}
          <button onclick={()=>unmountDrive(menu.drive)}>⏏ Unmount</button>
        {/if}
        {#if menu.drive.removable}
          <hr/>
          <button onclick={()=>ejectDrive(menu.drive)}>⏏ Eject — safe to remove</button>
        {/if}
      {:else if menu.place}
        <!-- SIDEBAR place menu (folder paths) -->
        <button onclick={()=>navigate(menu.place.path)}>Open</button>
        <button onclick={()=>newTab(menu.place.path)}>Open in New Tab</button>
        <button onclick={()=>newWindow(menu.place.path)}>Open in New Window</button>
        {#if menu.place.isFav}
          <hr/>
          <button class="danger" onclick={()=>removeFavorite(menu.place.path)}>Remove</button>
        {/if}
      {:else if menu.onEntry}
        <!-- FILE-VIEW item menu -->
        <button onclick={()=>activate(menu.entry)}>Open</button>
        {#if menu.entry?.is_dir}
          <button onclick={()=>newTab(menu.entry.path)}>Open in New Tab</button>
          <button onclick={()=>newWindow(menu.entry.path)}>Open in New Window</button>
        {:else if owApps && owApps.length}
          <div class="ow-item" onmouseenter={owEnter} onmouseleave={owLeave}>
            <button class="ow-parent" class:open={owOpen}>Open With <span class="ow-arrow">▸</span></button>
            {#if owOpen}
              <div class="ow-flyout" class:left={owFlip} bind:this={owFlyoutEl}>
                {#each owApps as a}
                  <button onclick={()=>runOpenWith(a.id, menu.entry.path)}>{a.name}</button>
                {/each}
              </div>
            {/if}
          </div>
        {/if}
        <hr/>
        <button onclick={doCut}>Cut</button>
        <button onclick={doCopy}>Copy</button>
        <button disabled={!clipboard} onclick={paste}>Paste</button>
        {#if isFav(menu.entry.path)}
          <button onclick={()=>removeFavorite(menu.entry.path)}>Remove from Favorites</button>
        {:else}
          <button onclick={()=>addFavorite(menu.entry.path)}>★ Add to Favorites</button>
        {/if}
        <button onclick={()=>startRename(menu.entry)}>Rename</button>
        <button onclick={compressSel}>Compress to ZIP</button>
        {#if isArchive(menu.entry.name)}<button onclick={()=>extractSel(menu.entry.path)}>Extract Here</button>{/if}
        <hr/>
        <button class="danger" onclick={del}>Move to Trash</button>
        <button class="danger" onclick={askDelete}>Delete permanently</button>
        <hr/>
        <button onclick={()=>openProps(menu.entry.path)}>Properties</button>
        <button onclick={termHere}>Open Terminal Here</button>
      {:else}
        <!-- empty-area menu -->
        <button onclick={startCreate}>New Folder</button>
        <button onclick={startCreateFile}>New File</button>
        <button disabled={!clipboard} onclick={paste}>Paste</button>
        {#if cwd.includes('/Trash/files')}<hr/><button class="danger" onclick={doEmptyTrash}>Empty Trash</button>{/if}
        <hr/>
        <button onclick={()=>openProps(cwd)}>Properties</button>
        <button onclick={termHere}>Open Terminal Here</button>
      {/if}
    </div>
  {/if}

  {#if propsData}
    <div class="modal-overlay" onclick={()=>propsData=null}>
      <div class="modal" onclick={(e)=>e.stopPropagation()}>
        <h3>{propsData.name}</h3>
        <table class="proptbl">
          <tbody>
            <tr><td>Type</td><td>{propsData.kind}</td></tr>
            <tr><td>Location</td><td class="mono">{propsData.path}</td></tr>
            {#if propsData.is_dir}
              <tr><td>Contains</td><td>{propsData.items < 0 ? '—' : propsData.items + ' items'}</td></tr>
            {:else}
              <tr><td>Size</td><td>{fmtSize(propsData.size)}  ({propsData.size.toLocaleString()} bytes)</td></tr>
            {/if}
            <tr><td>Permissions</td><td class="mono">{propsData.permissions}</td></tr>
            <tr><td>Modified</td><td>{fmtDate(propsData.modified)}</td></tr>
            {#if propsData.created}<tr><td>Created</td><td>{fmtDate(propsData.created)}</td></tr>{/if}
          </tbody>
        </table>
        <div class="modal-actions"><button onclick={()=>propsData=null}>Close</button></div>
      </div>
    </div>
  {/if}

  {#if confirmDel}
    <div class="modal-overlay" onclick={()=>confirmDel=false}>
      <div class="modal" onclick={(e)=>e.stopPropagation()}>
        <h3>Delete permanently?</h3>
        <p>{selectedSet.size} item{selectedSet.size>1?'s':''} will be permanently deleted. This cannot be undone.</p>
        <div class="modal-actions">
          <button onclick={()=>confirmDel=false}>Cancel</button>
          <button class="danger" onclick={delPerm}>Delete</button>
        </div>
      </div>
    </div>
  {/if}

  {#if showSettings}
    <div class="modal-overlay" onclick={()=>showSettings=false}>
      <div class="modal settings-modal" onclick={(e)=>e.stopPropagation()}>
        <h3>Options</h3>
        <div class="opt-section">Timestamps</div>
        <table class="proptbl">
          <tbody>
            <tr>
              <td>"Just now" duration</td>
              <td>
                <select class="set-select" bind:value={justNowMins} onchange={saveSettings}>
                  <option value={0}>Off — always show exact time</option>
                  <option value={1}>1 minute</option>
                  <option value={2}>2 minutes (default)</option>
                  <option value={5}>5 minutes</option>
                  <option value={10}>10 minutes</option>
                  <option value={30}>30 minutes</option>
                  <option value={60}>1 hour</option>
                </select>
              </td>
            </tr>
          </tbody>
        </table>
        <div class="opt-section">Details View — Columns</div>
        <table class="proptbl">
          <tbody>
            <tr><td>Show Size</td><td><label class="opt-toggle"><input type="checkbox" bind:checked={colSize} onchange={saveColVis} /> Visible</label></td></tr>
            <tr><td>Show Type</td><td><label class="opt-toggle"><input type="checkbox" bind:checked={colType} onchange={saveColVis} /> Visible</label></td></tr>
            <tr><td>Show Modified</td><td><label class="opt-toggle"><input type="checkbox" bind:checked={colDate} onchange={saveColVis} /> Visible</label></td></tr>
          </tbody>
        </table>
        <div class="opt-section">Grid / Icons View</div>
        <table class="proptbl">
          <tbody>
            <tr><td>Image thumbnails</td><td><label class="opt-toggle"><input type="checkbox" bind:checked={thumbsEnabled} onchange={()=>{ try{localStorage.setItem("zf_thumbs",String(thumbsEnabled));}catch{} navigate(cwd,false); }} /> Show previews for images</label></td></tr>
          </tbody>
        </table>
        <div class="modal-actions"><button onclick={()=>showSettings=false}>Close</button></div>
      </div>
    </div>
  {/if}
</div>

<style>
  :global(html), :global(body) { margin:0; height:100%; }
  :global(body){ font-family:"Inter","Segoe UI",system-ui,sans-serif; background:#1b1d22; color:#eceef2; -webkit-font-smoothing:antialiased; }
  .app{ display:flex; flex-direction:column; height:100vh; font-size:14.5px; cursor:default; user-select:none; -webkit-user-select:none; }
  .app input{ user-select:text; -webkit-user-select:text; cursor:text; }
  tbody tr, tbody td, .cell, .place, .tab { cursor:default; }

  .tabbar{ display:flex; align-items:stretch; gap:2px; background:#16181c; padding:4px 6px 0; }
  .tab{ display:flex; align-items:center; gap:6px; padding:6px 12px; background:#23262d; color:#aeb3bb; border-radius:7px 7px 0 0; cursor:pointer; max-width:220px; font-size:13px; }
  .tab.active{ background:#2c3038; color:#fff; }
  .tab:hover:not(.active){ background:#262a31; }
  .tablabel{ overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .tabclose{ background:none; border:none; color:#8b909a; cursor:pointer; font-size:15px; line-height:1; padding:0 3px; border-radius:4px; }
  .tabclose:hover{ background:#d33; color:#fff; }
  .tabadd{ background:none; border:none; color:#8b909a; cursor:pointer; font-size:18px; padding:0 10px; border-radius:6px; }
  .tabadd:hover{ background:#2c3038; color:#fff; }
  tr.drop, .place.drop, .cell.drop, .tab.drop, .files.drop{ outline:2px solid #3a6df0 !important; outline-offset:-2px; background:#2a3550 !important; }
  .ow-item{ position:relative; display:flex; }
  .ow-item > .ow-parent{ width:100%; display:flex; align-items:center; justify-content:space-between; }
  .ow-parent .ow-arrow{ opacity:.55; font-size:11px; margin-left:10px; }
  .ow-item:hover > .ow-parent, .ow-parent.open{ background:#3a6df0; color:#fff; }
  .ow-flyout{ position:absolute; top:-5px; left:100%; min-width:172px; max-width:240px; max-height:60vh; overflow-y:auto;
              background:#2c3038; border:1px solid #444a55; border-radius:8px; padding:4px;
              box-shadow:0 8px 28px rgba(0,0,0,.6); z-index:101; display:flex; flex-direction:column; }
  .ow-flyout.left{ left:auto; right:100%; }
  .ow-flyout button{ white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }

  .toolbar{ display:flex; gap:4px; padding:6px 8px; background:#23262d; border-bottom:1px solid #000; align-items:center; }
  .toolbar button{ background:#2c3038; color:#cfd3da; border:1px solid #3a3f49; border-radius:6px; padding:5px 9px; cursor:pointer; min-width:30px; }
  .opt-btn{ background:#2c3038; border-color:#555b66; color:#dde0e6; font-size:13px; gap:4px; white-space:nowrap; }
  .toolbar button:hover:not(:disabled){ background:#353b45; }
  .toolbar button:disabled{ opacity:.35; cursor:default; }
  .toolbar button.active{ background:#3a6df0; border-color:#3a6df0; color:#fff; }
  .addr{ flex:1; background:#15171b; color:#e3e5ea; border:1px solid #3a3f49; border-radius:6px; padding:6px 9px; font-family:monospace; }
  .crumbs{ flex:1; display:flex; align-items:center; gap:0; overflow:hidden; min-width:0; padding:0 4px; color:#aeb3bb; }
  .crumb{ color:#c4c8cf; cursor:pointer; min-width:0; padding:0 1px; white-space:nowrap; font:inherit; font-size:14px; line-height:1.2; }
  .crumb:hover{ color:#fff; text-decoration:underline; text-underline-offset:2px; }
  .crumb:focus-visible{ outline:1px solid #687386; outline-offset:2px; border-radius:2px; }
  .crumbsep{ color:#656b75; flex:none; padding:0 5px; font-size:14px; }
  .crumb-paste-hint{ flex:1; color:transparent; cursor:text; min-width:20px; padding:0 4px; }
  .splitter{ width:6px; cursor:col-resize; background:transparent; flex:none; transition:background .1s; }
  .splitter:hover{ background:#3a6df0; }
  .search{ width:150px; background:#15171b; color:#e3e5ea; border:1px solid #3a3f49; border-radius:6px; padding:6px 9px; }

  .body{ flex:1; display:flex; min-height:0; }
  .sidebar{ width:185px; flex:none; background:#202329; border-right:1px solid #000; padding:8px; display:flex; flex-direction:column; gap:1px; overflow:auto; }
  .sec{ color:#6b7079; font-size:11px; text-transform:uppercase; letter-spacing:.05em; padding:10px 8px 4px; }
  .place{ display:flex; align-items:center; gap:8px; text-align:left; background:none; border:none; color:#c4c8cf; padding:6px 10px; border-radius:6px; cursor:pointer; width:100%; }
  .place:hover{ background:#2c3038; }
  .place.sel{ background:#3a6df0; color:#fff; }
  .place .ic{ width:18px; text-align:center; flex:none; font-size:13px; }
  .ic.img{ width:18px; height:18px; object-fit:contain; flex:none; vertical-align:middle; }
  .place.recent{ color:#9aa0aa; }
  .tree{ display:flex; flex-direction:column; }
  .treerow{ display:flex; align-items:center; gap:4px; padding:4px 6px; border-radius:6px; cursor:pointer; color:#c4c8cf; white-space:nowrap; overflow:hidden; }
  .treerow:hover{ background:#2c3038; }
  .treerow.sel{ background:#3a6df0; color:#fff; }
  .twist{ width:16px; flex:none; font-size:10px; text-align:center; opacity:.7; }
  .treerow .dname{ flex:1; overflow:hidden; text-overflow:ellipsis; }
  .dname{ flex:1; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .badge{ font-size:10px; background:#3a6df0; color:#fff; border-radius:4px; padding:1px 5px; }
  .dsz{ font-size:10px; color:#6b7079; }
  .hidden-toggle{ margin-top:auto; color:#8b909a; font-size:12px; padding:8px 4px; display:flex; gap:6px; align-items:center; }

  .files{ flex:1; overflow:auto; background:#1b1d22; position:relative; }
  .lasso{ position:fixed; pointer-events:none; border:1px solid #3a6df0; background:rgba(58,109,240,0.12); z-index:9999; }
  .empty{ padding:40px; text-align:center; color:#6b7079; }
  .createbar{ padding:8px 10px; background:#23262d; display:flex; gap:8px; align-items:center; }
  .createbar input, .rename{ background:#15171b; color:#fff; border:1px solid #3a6df0; border-radius:4px; padding:3px 6px; font:inherit; }

  table{ width:100%; border-collapse:collapse; font-size:calc(14.5px * var(--zoom,1)); }
  thead th{ position:sticky; top:0; background:#23262d; text-align:left; padding:8px 14px; color:#b8bdc6; font-weight:600; border-bottom:1px solid #000; font-size:14.5px; }
  th.num{ text-align:right; }
  th.sortable{ cursor:pointer; user-select:none; }
  th.sortable:hover{ color:#fff; }
  .ficon{ width:calc(20px * var(--zoom,1)); height:calc(20px * var(--zoom,1)); vertical-align:middle; margin-right:9px; }
  .ficon.ph{ display:inline-block; }
  td.type{ color:#aab0ba; }
  tbody tr:hover{ background:#24272e; }
  tbody tr.sel{ background:#3a6df0; color:#fff; }
  td{ padding:7px 14px; white-space:nowrap; }
  td.name{ max-width:520px; overflow:hidden; text-overflow:ellipsis; }
  td.num{ text-align:right; color:#c4c9d1; }
  td.date{ color:#aab0ba; }
  tr.sel td{ color:#fff; }

  .grid{ display:grid; grid-template-columns:repeat(auto-fill,minmax(calc(110px * var(--zoom,1)),1fr)); gap:6px; padding:10px; }
  .cell{ background:none; border:1px solid transparent; border-radius:8px; padding:10px 6px; cursor:pointer; text-align:center; color:inherit; font:inherit; }
  .cell:hover{ background:#24272e; }
  .cell.sel{ background:#3a6df0; color:#fff; }
  .cellicon{ line-height:1.2; height:calc(52px * var(--zoom,1)); display:flex; align-items:center; justify-content:center; }
  .cellicon img{ width:calc(48px * var(--zoom,1)); height:calc(48px * var(--zoom,1)); }
  .cellicon .ph{ width:calc(48px * var(--zoom,1)); height:calc(48px * var(--zoom,1)); display:inline-block; }
  .cellicon .thumb{ width:calc(72px * var(--zoom,1)); height:calc(72px * var(--zoom,1)); object-fit:cover; border-radius:4px; }
  .cellname{ font-size:calc(12px * var(--zoom,1)); word-break:break-word; margin-top:4px; }

  .preview{ width:320px; flex:none; background:#202329; border-left:1px solid #000; display:flex; flex-direction:column; }
  .pv-image{ flex:1; display:flex; align-items:center; justify-content:center; padding:12px; min-height:0; overflow:auto; }
  .pv-image img{ max-width:calc(100% * var(--pvzoom,1)); max-height:calc(100% * var(--pvzoom,1)); object-fit:contain; border-radius:6px; box-shadow:0 4px 16px rgba(0,0,0,.5); }
  .pv-text{ flex:1; min-height:0; width:100%; margin:0; padding:10px 12px; box-sizing:border-box; overflow:auto;
            background:#16181c; border-radius:6px; color:#cdd2da;
            font:12px/1.5 ui-monospace, "Cascadia Code", "JetBrains Mono", Menlo, Consolas, monospace;
            white-space:pre-wrap; word-break:break-word; }
  .pv-placeholder{ color:#6b7079; text-align:center; padding:20px; }
  .pv-placeholder.big{ font-size:80px; }
  .pv-audio{ display:flex; flex-direction:column; align-items:center; justify-content:center; flex:1; gap:14px; padding:20px; }
  .pv-music-art{ font-size:72px; line-height:1; user-select:none; }
  .pv-audio-title{ font-size:14px; font-weight:600; color:#e3e5ea; text-align:center; word-break:break-word; padding:0 8px; }
  .pv-audio-ext{ font-size:11px; color:#6b7079; text-transform:uppercase; letter-spacing:.08em; }
  .pv-audio-controls{ width:100%; display:flex; flex-direction:column; align-items:center; gap:10px; }
  .pv-play{ width:50px; height:50px; border-radius:50%; background:#3b82f6; border:none; color:#fff; font-size:22px; cursor:pointer; display:flex; align-items:center; justify-content:center; transition:background .15s; }
  .pv-play:hover{ background:#2563eb; }
  .pv-seek-row{ display:flex; align-items:center; gap:8px; width:100%; }
  .pv-seek{ flex:1; accent-color:#3b82f6; cursor:pointer; }
  .pv-time{ font-size:11px; color:#8a8f99; min-width:34px; text-align:center; font-variant-numeric:tabular-nums; }
  .pv-meta{ padding:12px 14px; border-top:1px solid #000; font-size:12px; color:#aeb3bb; }
  .pv-name{ font-weight:600; color:#e3e5ea; word-break:break-all; margin-bottom:4px; }
  .pv-date{ color:#6b7079; margin-top:4px; }

  .status{ padding:4px 12px; background:#23262d; border-top:1px solid #000; color:#8b909a; font-size:12px; }
  .clip{ color:#3a6df0; }
  .toast{ position:fixed; bottom:34px; left:50%; transform:translateX(-50%); background:#3a6df0; color:#fff; padding:8px 16px; border-radius:8px; box-shadow:0 4px 16px rgba(0,0,0,.5); z-index:50; }

  .ctxmenu{ position:fixed; background:#2c3038; border:1px solid #444a55; border-radius:8px; padding:4px; min-width:180px; box-shadow:0 8px 28px rgba(0,0,0,.6); z-index:100; display:flex; flex-direction:column; }
  .ctxmenu button{ text-align:left; background:none; border:none; color:#e3e5ea; padding:7px 12px; border-radius:5px; cursor:pointer; font:inherit; }
  .ctxmenu button:hover:not(:disabled){ background:#3a6df0; color:#fff; }
  .ctxmenu button:disabled{ opacity:.35; }
  .ctxmenu button.danger:hover{ background:#d33; }
  .ctxmenu hr{ border:none; border-top:1px solid #444a55; margin:4px 2px; }

  .modal-overlay{ position:fixed; inset:0; background:rgba(0,0,0,.55); display:flex; align-items:center; justify-content:center; z-index:200; }
  .modal{ background:#23262d; border:1px solid #3a3f49; border-radius:10px; padding:20px 22px; min-width:380px; max-width:580px; box-shadow:0 12px 40px rgba(0,0,0,.6); }
  .modal h3{ margin:0 0 14px; font-size:16px; word-break:break-all; }
  .modal p{ color:#c4c9d1; line-height:1.5; }
  .proptbl{ width:100%; border-collapse:collapse; font-size:13.5px; }
  .proptbl td{ padding:5px 8px; vertical-align:top; }
  .proptbl td:first-child{ color:#8b909a; white-space:nowrap; width:110px; }
  .mono{ font-family:monospace; word-break:break-all; }
  .modal-actions{ display:flex; justify-content:flex-end; gap:8px; margin-top:18px; }
  .modal-actions button{ background:#2c3038; color:#e3e5ea; border:1px solid #3a3f49; border-radius:6px; padding:7px 16px; cursor:pointer; font:inherit; }
  .modal-actions button:hover{ background:#353b45; }
  .modal-actions button.danger{ background:#c0392b; border-color:#c0392b; }
  .modal-actions button.danger:hover{ background:#e74c3c; }
  .set-select{ background:#15171b; color:#e3e5ea; border:1px solid #3a3f49; border-radius:5px; padding:4px 8px; font:inherit; cursor:pointer; }
  .set-select:focus{ outline:1px solid #3a6df0; }
  .settings-modal{ min-width:420px; }
  .opt-section{ font-size:11px; text-transform:uppercase; letter-spacing:.06em; color:#6b7079; padding:12px 0 4px; }
  .opt-toggle{ display:flex; align-items:center; gap:7px; cursor:pointer; }
</style>
