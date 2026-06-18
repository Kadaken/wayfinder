<script>
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";

  let places = $state([]);
  let drives = $state([]);
  let cwd = $state("");
  let entries = $state([]);
  let selectedSet = $state(new Set());
  let selected = $state(null);      // last-clicked, drives the preview
  let lastIndex = $state(-1);
  let previewSrc = $state("");
  let previewError = $state("");
  let view = $state("details");
  let showHidden = $state(false);
  let addr = $state("");
  let history = $state([]);
  let hidx = $state(-1);
  let loading = $state(false);
  let search = $state("");
  let iconZoom = $state(1);
  let previewZoom = $state(1);
  const clamp = (v,a,b) => Math.max(a, Math.min(b, v));
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
  let clipboard = $state(null);     // { mode:"copy"|"cut", paths:[] }
  let menu = $state(null);          // { x, y, onEntry }
  let renaming = $state(null);      // path being renamed
  let renameVal = $state("");
  let creating = $state(false);
  let createVal = $state("");
  let toast = $state("");
  let favorites = $state([]);
  let recents = $state([]);

  const basename = (p) => p === "/" ? "/" : (p.replace(/\/+$/,"").split("/").pop() || p);
  function loadStore(){
    try { favorites = JSON.parse(localStorage.getItem("zf_favorites") || "[]"); } catch { favorites = []; }
    try { recents = JSON.parse(localStorage.getItem("zf_recents") || "[]"); } catch { recents = []; }
    try { const z = parseFloat(localStorage.getItem("zf_zoom")); if (z) iconZoom = z; } catch {}
  }
  const saveFav = () => { try { localStorage.setItem("zf_favorites", JSON.stringify(favorites)); } catch {} };
  const saveRec = () => { try { localStorage.setItem("zf_recents", JSON.stringify(recents)); } catch {} };
  const isFav = (path) => favorites.some(f => f.path === path);
  function addFavorite(path){ if (!isFav(path)){ favorites = [...favorites, { name: basename(path), path }]; saveFav(); flash("★ Added to Favorites"); } menu = null; }
  function removeFavorite(path){ favorites = favorites.filter(f => f.path !== path); saveFav(); menu = null; }
  function pushRecent(path){ if (recents.some(r => r.path === path)) return; recents = [{ name: basename(path), path }, ...recents].slice(0, 12); saveRec(); }
  let propsData = $state(null);
  let confirmDel = $state(false);
  // tabs
  let tabs = $state([{ path:"", label:"", history:[], hidx:-1 }]);
  let activeIdx = $state(0);
  // drag & drop
  let dragPaths = $state([]);
  let dropTarget = $state("");
  // open-with apps (loaded when right-clicking a file)
  let owApps = $state(null);
  let owOpen = $state(false);

  function syncTab(){ tabs[activeIdx] = { path: cwd, label: basename(cwd) || "/", history: [...history], hidx }; tabs = tabs; }
  function newTab(path){ tabs = [...tabs, { path: path||cwd, label:"", history:[], hidx:-1 }]; activeIdx = tabs.length-1; history = []; hidx = -1; menu = null; navigate(path || cwd); }
  function switchTab(i){ if (i===activeIdx) return; syncTab(); activeIdx = i; const t = tabs[i]; history = [...t.history]; hidx = t.hidx; navigate(t.path, false); }
  function closeTab(i, ev){ if (ev) ev.stopPropagation(); if (tabs.length===1) return; const wasActive = i===activeIdx; tabs = tabs.filter((_,j)=>j!==i); if (activeIdx > i || activeIdx >= tabs.length) activeIdx = Math.max(0, activeIdx-1); if (wasActive){ const t = tabs[activeIdx]; history=[...t.history]; hidx=t.hidx; navigate(t.path,false); } }

  function onDragStart(ev, e){ if (!selectedSet.has(e.path)){ selectedSet = new Set([e.path]); selected = e; } dragPaths = [...selectedSet]; ev.dataTransfer.effectAllowed = "copyMove"; }
  function allowDrop(ev, isDir, path){ if (isDir && dragPaths.length){ ev.preventDefault(); dropTarget = path; } }
  function onDropFolder(ev, destDir){ ev.preventDefault(); dropTarget=""; const srcs = dragPaths.filter(p => p !== destDir); dragPaths=[]; if (!srcs.length) return; const op = ev.ctrlKey ? "copy_paths" : "move_paths"; invoke(op, { srcs, destDir }).then(()=>{ flash(ev.ctrlKey?"Copied":"Moved"); navigate(cwd,false); }).catch(e=>flash("⚠ "+e)); }
  function runOpenWith(id, path){ invoke("open_with", { appId:id, path }); menu=null; owApps=null; }

  function placeMenu(ev, path, isFav){ ev.preventDefault(); ev.stopPropagation(); owApps=null; menu = { x: ev.clientX, y: ev.clientY, place: { path, isFav } }; }
  function newWindow(path){ invoke("new_window", { path }); menu = null; }
  async function openProps(path){ menu = null; try { propsData = await invoke("properties", { path }); } catch(e){ flash("⚠ " + e); } }
  function askDelete(){ menu = null; if (selectedSet.size) confirmDel = true; }
  async function delPerm(){ confirmDel = false; try { await invoke("delete_permanent", { paths: [...selectedSet] }); flash("Deleted"); navigate(cwd, false); } catch(e){ flash("⚠ " + e); } }

  const fmtSize = (n) => {
    const u = ["B","KB","MB","GB","TB"]; let i=0;
    while (n >= 1024 && i < u.length-1){ n/=1024; i++; }
    return i===0 ? `${n} B` : `${n.toFixed(1)} ${u[i]}`;
  };
  const fmtDate = (s) => s ? new Date(s*1000).toLocaleString() : "";
  const flash = (m) => { toast = m; setTimeout(() => toast = "", 2600); };
  function copyAddr(){
    const t = cwd;
    (navigator.clipboard?.writeText(t) ?? Promise.reject())
      .then(() => flash("📋 Copied path"))
      .catch(() => flash("⚠ copy failed"));
  }

  let sortKey = $state("name");
  let sortDir = $state(1);
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
    loading = true; menu = null; renaming = null; creating = false;
    try {
      entries = await invoke("list_dir", { path, showHidden });
      for (const nm of new Set(entries.map(e=>e.icon))) ensureIcon(nm);
      cwd = path; addr = path; search = "";
      selectedSet = new Set(); selected = null; lastIndex = -1;
      previewSrc = ""; previewError = "";
      if (record) { history = [...history.slice(0, hidx+1), path]; hidx = history.length-1; pushRecent(path); }
      refreshDrives();
      syncTab();
    } catch (e) { previewError = String(e); flash("⚠ " + e); }
    finally { loading = false; }
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
    previewSrc = ""; previewError = ""; previewZoom = 1;
    if (e.is_image) {
      try { previewSrc = await invoke("read_data_url", { path: e.path }); }
      catch (err) { previewError = String(err); }
    }
  }

  function activate(e) { if (e.is_dir) navigate(e.path); else invoke("open_path", { path: e.path }); }

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
  function startCreate(){ creating = true; createVal = "New Folder"; menu = null; }
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
    if (e && !e.is_dir) invoke("open_with_apps", { path: e.path }).then(a => { const seen=new Set(); owApps = (a||[]).filter(x => !seen.has(x.name) && seen.add(x.name)); }).catch(()=>{});
  }

  function onKey(ev){
    const inInput = ["INPUT"].includes(document.activeElement?.tagName);
    if (inInput) return;
    if (ev.key === "Backspace") { ev.preventDefault(); up(); }
    else if (ev.altKey && ev.key === "ArrowLeft") back();
    else if (ev.altKey && ev.key === "ArrowRight") fwd();
    else if (ev.key === "Delete") del();
    else if (ev.key === "F2" && selected) startRename(selected);
    else if (ev.ctrlKey && ev.key === "c") doCopy();
    else if (ev.ctrlKey && ev.key === "x") doCut();
    else if (ev.ctrlKey && ev.key === "v") paste();
    else if (ev.ctrlKey && ev.key === "a") { ev.preventDefault(); selectedSet = new Set(rows.map(e=>e.path)); }
    else if (ev.ctrlKey && ev.key === "t") { ev.preventDefault(); newTab(cwd); }
    else if (ev.ctrlKey && ev.key === "w") { ev.preventDefault(); closeTab(activeIdx); }
    else if (ev.key === "Enter" && selected) activate(selected);
    else if (ev.ctrlKey && ev.key === "l") { ev.preventDefault(); document.getElementById("addr")?.select(); }
    else if (ev.key === "F5") navigate(cwd, false);
    else if (ev.key === "Escape") { menu = null; propsData = null; confirmDel = false; selectedSet = new Set(); }
  }

  onMount(async () => {
    loadStore();
    places = await invoke("standard_dirs");
    await refreshDrives();
    let sp = null;
    try { sp = await invoke("start_path"); } catch {}
    navigate(sp || places[0]?.[1] || "/");
  });
</script>

<svelte:window on:keydown={onKey} on:click={() => { menu = null; owApps = null; }} />

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
    <input id="addr" class="addr" bind:value={addr}
           onkeydown={(e)=> e.key==='Enter' && navigate(addr.trim())} spellcheck="false" />
    <button onclick={copyAddr} title="Copy path to clipboard">📋</button>
    <input class="search" placeholder="Search…" bind:value={search} />
    <button class:active={view==='details'} onclick={()=>view='details'} title="Details">☰</button>
    <button class:active={view==='icons'} onclick={()=>view='icons'} title="Icons">▦</button>
    <button onclick={()=>navigate(cwd,false)} title="Refresh">⟳</button>
  </div>

  <div class="body">
    <aside class="sidebar">
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
          <span class="ic">📁</span><span class="dname">{p[0]}</span>
        </button>
      {/each}
      <div class="sec">Devices</div>
      {#each drives as d}
        <button class="place drive" class:sel={cwd===d.mountpoint && d.mountpoint}
                onclick={()=>openDrive(d)} title={d.path}>
          <span>{d.kind==='gdrive' ? '☁' : d.kind==='network' ? '🌐' : '🖴'}</span>
          <span class="dname">{d.name}</span>
          {#if !d.mountpoint}<span class="badge">mount</span>{:else if d.size}<span class="dsz">{fmtSize(d.size)}</span>{/if}
        </button>
      {/each}
      {#if recents.length}
        <div class="sec">Recent</div>
        {#each recents.slice(0,10) as r}
          <button class="place recent" class:sel={cwd===r.path} class:drop={dropTarget===r.path}
                  ondragover={(e)=>allowDrop(e,true,r.path)} ondrop={(e)=>onDropFolder(e,r.path)} ondragleave={()=>dropTarget=''}
                  onclick={()=>navigate(r.path)} oncontextmenu={(e)=>placeMenu(e,r.path,false)} title={r.path}>
            <span class="ic">🕘</span><span class="dname">{r.name}</span>
          </button>
        {/each}
      {/if}
      <label class="hidden-toggle"><input type="checkbox" checked={showHidden} onchange={()=>{showHidden=!showHidden; navigate(cwd,false);}} /> Hidden files</label>
    </aside>

    <main class="files {view}" style="--zoom:{iconZoom}" onwheel={wheelFiles} oncontextmenu={(e)=>ctx(e,null)}>
      {#if creating}
        <div class="createbar">📁 <input autofocus bind:value={createVal}
             onkeydown={(e)=> e.key==='Enter'?commitCreate(): e.key==='Escape'?(creating=false):null}
             onblur={commitCreate} /></div>
      {/if}
      {#if loading}
        <div class="empty">Loading…</div>
      {:else if filtered.length === 0}
        <div class="empty">{search ? 'No matches' : 'Empty folder'}</div>
      {:else if view === 'details'}
        <table>
          <thead><tr>
            <th class="sortable" onclick={()=>setSort('name')}>Name{arrow('name')}</th>
            <th class="num sortable" onclick={()=>setSort('size')}>Size{arrow('size')}</th>
            <th class="sortable" onclick={()=>setSort('type')}>Type{arrow('type')}</th>
            <th class="sortable" onclick={()=>setSort('modified')}>Modified{arrow('modified')}</th>
          </tr></thead>
          <tbody>
            {#each rows as e, i}
              <tr class:sel={selectedSet.has(e.path)} class:drop={dropTarget===e.path && e.is_dir}
                  draggable="true" ondragstart={(ev)=>onDragStart(ev,e)}
                  ondragover={(ev)=>allowDrop(ev,e.is_dir,e.path)} ondrop={(ev)=>onDropFolder(ev,e.path)} ondragleave={()=>dropTarget=''}
                  onclick={(ev)=>select(e,i,ev)} ondblclick={()=>activate(e)} oncontextmenu={(ev)=>ctx(ev,e)}>
                <td class="name">
                  {#if iconCache[e.icon]}<img class="ficon" src={iconCache[e.icon]} alt="" />{:else}<span class="ficon ph"></span>{/if}
                  {#if renaming===e.path}
                    <input class="rename" autofocus bind:value={renameVal}
                      onkeydown={(ev)=> ev.key==='Enter'?commitRename(): ev.key==='Escape'?(renaming=null):null}
                      onblur={commitRename} onclick={(ev)=>ev.stopPropagation()} />
                  {:else}{e.name}{/if}
                </td>
                <td class="num">{e.is_dir ? '' : fmtSize(e.size)}</td>
                <td class="type">{typeLabel(e)}</td>
                <td class="date">{fmtDate(e.modified)}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {:else}
        <div class="grid">
          {#each rows as e, i}
            <button class="cell" class:sel={selectedSet.has(e.path)} class:drop={dropTarget===e.path && e.is_dir}
                 draggable="true" ondragstart={(ev)=>onDragStart(ev,e)}
                 ondragover={(ev)=>allowDrop(ev,e.is_dir,e.path)} ondrop={(ev)=>onDropFolder(ev,e.path)} ondragleave={()=>dropTarget=''}
                 onclick={(ev)=>select(e,i,ev)} ondblclick={()=>activate(e)} oncontextmenu={(ev)=>ctx(ev,e)}>
              <div class="cellicon">
                {#if iconCache[e.icon]}<img src={iconCache[e.icon]} alt="" />{:else}<span class="ph"></span>{/if}
              </div>
              <div class="cellname">{e.name}</div>
            </button>
          {/each}
        </div>
      {/if}
    </main>

    <aside class="preview" style="--pvzoom:{previewZoom}" onwheel={wheelPreview}>
      {#if selected}
        <div class="pv-image">
          {#if previewSrc}<img src={previewSrc} alt={selected.name} />
          {:else if previewError}<div class="pv-placeholder">⚠ {previewError}</div>
          {:else}<div class="pv-placeholder big">{selected.is_dir ? '📁' : '📄'}</div>{/if}
        </div>
        <div class="pv-meta">
          <div class="pv-name">{selected.name}</div>
          <div>{selected.is_dir ? 'Folder' : (selected.path.split('.').pop().toUpperCase()+' file')}</div>
          {#if !selected.is_dir}<div>{fmtSize(selected.size)}</div>{/if}
          <div class="pv-date">{fmtDate(selected.modified)}</div>
        </div>
      {:else}<div class="pv-placeholder">Select a file</div>{/if}
    </aside>
  </div>

  <div class="status">
    {rows.length} items{selectedSet.size>1 ? `  ·  ${selectedSet.size} selected${selSize? ' ('+fmtSize(selSize)+')':''}` : selected ? `  ·  ${selected.name}` : ''}
    {#if clipboard}<span class="clip">  ·  {clipboard.mode==='cut'?'✂':'⧉'} {clipboard.paths.length} ready to paste</span>{/if}
  </div>

  {#if toast}<div class="toast">{toast}</div>{/if}

  {#if menu}
    <div class="ctxmenu" style="left:{menu.x}px; top:{menu.y}px" oncontextmenu={(e)=>e.preventDefault()}>
      {#if menu.place}
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
          <button onclick={(e)=>{ e.stopPropagation(); owOpen = !owOpen; }}>Open With&nbsp; {owOpen ? '▾' : '▸'}</button>
          {#if owOpen}
            {#each owApps as a}
              <button class="sub" onclick={()=>runOpenWith(a.id, menu.entry.path)}>{a.name}</button>
            {/each}
          {/if}
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
        <hr/>
        <button class="danger" onclick={del}>Move to Trash</button>
        <button class="danger" onclick={askDelete}>Delete permanently</button>
        <hr/>
        <button onclick={()=>openProps(menu.entry.path)}>Properties</button>
        <button onclick={termHere}>Open Terminal Here</button>
      {:else}
        <!-- empty-area menu -->
        <button onclick={startCreate}>New Folder</button>
        <button disabled={!clipboard} onclick={paste}>Paste</button>
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
  tr.drop, .place.drop, .cell.drop, .tab.drop{ outline:2px solid #3a6df0 !important; outline-offset:-2px; background:#2a3550 !important; }
  .ctxmenu .sub{ padding-left:22px; color:#c4c9d1; font-size:12.5px; }

  .toolbar{ display:flex; gap:4px; padding:6px 8px; background:#23262d; border-bottom:1px solid #000; align-items:center; }
  .toolbar button{ background:#2c3038; color:#cfd3da; border:1px solid #3a3f49; border-radius:6px; padding:5px 9px; cursor:pointer; min-width:30px; }
  .toolbar button:hover:not(:disabled){ background:#353b45; }
  .toolbar button:disabled{ opacity:.35; cursor:default; }
  .toolbar button.active{ background:#3a6df0; border-color:#3a6df0; color:#fff; }
  .addr{ flex:1; background:#15171b; color:#e3e5ea; border:1px solid #3a3f49; border-radius:6px; padding:6px 9px; font-family:monospace; }
  .search{ width:150px; background:#15171b; color:#e3e5ea; border:1px solid #3a3f49; border-radius:6px; padding:6px 9px; }

  .body{ flex:1; display:flex; min-height:0; }
  .sidebar{ width:185px; background:#202329; border-right:1px solid #000; padding:8px; display:flex; flex-direction:column; gap:1px; overflow:auto; }
  .sec{ color:#6b7079; font-size:11px; text-transform:uppercase; letter-spacing:.05em; padding:10px 8px 4px; }
  .place{ display:flex; align-items:center; gap:8px; text-align:left; background:none; border:none; color:#c4c8cf; padding:6px 10px; border-radius:6px; cursor:pointer; width:100%; }
  .place:hover{ background:#2c3038; }
  .place.sel{ background:#3a6df0; color:#fff; }
  .place .ic{ width:18px; text-align:center; flex:none; font-size:13px; }
  .place.recent{ color:#9aa0aa; }
  .dname{ flex:1; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .badge{ font-size:10px; background:#3a6df0; color:#fff; border-radius:4px; padding:1px 5px; }
  .dsz{ font-size:10px; color:#6b7079; }
  .hidden-toggle{ margin-top:auto; color:#8b909a; font-size:12px; padding:8px 4px; display:flex; gap:6px; align-items:center; }

  .files{ flex:1; overflow:auto; background:#1b1d22; }
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
  .cellname{ font-size:calc(12px * var(--zoom,1)); word-break:break-word; margin-top:4px; }

  .preview{ width:320px; background:#202329; border-left:1px solid #000; display:flex; flex-direction:column; }
  .pv-image{ flex:1; display:flex; align-items:center; justify-content:center; padding:12px; min-height:0; overflow:auto; }
  .pv-image img{ max-width:calc(100% * var(--pvzoom,1)); max-height:calc(100% * var(--pvzoom,1)); object-fit:contain; border-radius:6px; box-shadow:0 4px 16px rgba(0,0,0,.5); }
  .pv-placeholder{ color:#6b7079; text-align:center; padding:20px; }
  .pv-placeholder.big{ font-size:80px; }
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
</style>
