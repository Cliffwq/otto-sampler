import { dragIcon } from "./assets/drag-icon.js";
// Otto-Sampler Phase 1 Javascript Core MVP
const { invoke, Channel } = window.__TAURI__.core;
const convertFileSrc = window.__TAURI__.core.convertFileSrc;

function uiIcon(name) {
  return '<svg class="ui-icon" viewBox="0 0 24 24" aria-hidden="true"><use href="assets/ui-icons.svg#' + name + '"/></svg>';
}

// --- STATE MANAGEMENT ---
let favorites = [];
let modalPaths = []; // Track paths inside Favorites manager modal
let activeFavorite = "";
let currentQuery = "";
let currentFilter = "all"; // "all" | "audio" | "video"
let currentSortField = "filename"; // "filename" | "duration" | "mtime"
let searchPageState = null;
let scanStatsTimer = null;
let currentSortDir = "asc"; // "asc" | "desc"

// Shared Audio Object for low-latency playback
const audio = new Audio();
let playingPath = "";
let isPlaying = false;
let duration = 0;
let contextMenuPath = "";

// Helper: Format seconds to MM:SS
function formatTime(secs) {
  if (isNaN(secs) || !isFinite(secs)) return "00:00";
  const m = Math.floor(secs / 60).toString().padStart(2, "0");
  const s = Math.floor(secs % 60).toString().padStart(2, "0");
  return `${m}:${s}`;
}

// Custom startDrag helper to initiate native OLE drag operations
async function startDrag(options) {
  const onEventChannel = new Channel();
  onEventChannel.onmessage = (event) => {
    console.log("Drag event:", event);
  };


  const payload = {
    item: options.item,
    image: options.icon || dragIcon,
    options: {},
    onEvent: onEventChannel,
  };

  try {
    await invoke("plugin:drag|start_drag", payload);
  } catch (err) {
    console.error("Failed to start drag:", err);
    alert("拖拽失败: " + err);
  }
}

// System file double click helper
async function openSystemFile(path) {
  try {
    // Standard Windows double click path opener
    await invoke("plugin:opener|open_path", { path: path });
  } catch (err) {
    console.error("Failed to open file via system:", err);
    alert("打开文件失败: " + err);
  }
}

async function runFileMenuAction(action, path) {
  if (action === "properties") {
    await showPropertiesModal(path);
    return;
  }

  const commandMap = {
    reveal: "reveal_file",
    openWith: "open_file_with",
  };
  const command = commandMap[action];
  if (!command || !path) return;

  try {
    await invoke(command, { path });
  } catch (err) {
    console.error(`File menu action failed: ${action}`, err);
    alert("文件操作失败: " + err);
  }
}

function ensureFileContextMenu() {
  let menu = document.querySelector("#file-context-menu");
  if (menu) return menu;

  menu = document.createElement("div");
  menu.id = "file-context-menu";
  menu.className = "file-context-menu hidden";
  menu.innerHTML = `
    <button type="button" class="file-context-menu-item" data-action="reveal">打开所在位置</button>
    <button type="button" class="file-context-menu-item" data-action="openWith">打开方式</button>
    <button type="button" class="file-context-menu-item" data-action="properties">属性</button>
  `;

  menu.addEventListener("click", async (e) => {
    const item = e.target.closest(".file-context-menu-item");
    if (!item) return;

    const path = contextMenuPath;
    hideFileContextMenu();
    await runFileMenuAction(item.dataset.action, path);
  });

  document.body.appendChild(menu);
  return menu;
}

function showFileContextMenu(e, path) {
  e.preventDefault();
  e.stopPropagation();

  contextMenuPath = path;
  const menu = ensureFileContextMenu();
  menu.classList.remove("hidden");

  const menuRect = menu.getBoundingClientRect();
  const left = Math.min(e.clientX, window.innerWidth - menuRect.width - 8);
  const top = Math.min(e.clientY, window.innerHeight - menuRect.height - 8);

  menu.style.left = `${Math.max(8, left)}px`;
  menu.style.top = `${Math.max(8, top)}px`;
}

function hideFileContextMenu() {
  const menu = document.querySelector("#file-context-menu");
  if (menu) {
    menu.classList.add("hidden");
  }
  contextMenuPath = "";
}

async function copyText(value) {
  try {
    await navigator.clipboard.writeText(value);
  } catch (err) {
    console.error("Clipboard write failed:", err);
    alert("复制失败: " + err);
  }
}

function closePropertiesModal() {
  const modal = document.querySelector("#properties-modal");
  if (modal) {
    modal.classList.add("hidden");
  }
}

function renderPropertiesFields(info) {
  const fieldsBox = document.querySelector("#properties-fields");
  fieldsBox.innerHTML = "";

  const fields = info.fields || [];
  fields.forEach((field) => {
    const row = document.createElement("div");
    row.className = "property-row";

    const label = document.createElement("label");
    label.className = "property-label";
    label.textContent = field.label;

    const input = document.createElement("input");
    input.className = "property-value";
    input.type = "text";
    input.readOnly = true;
    input.value = field.value || "-";
    input.title = field.value || "-";

    const copyBtn = document.createElement("button");
    copyBtn.type = "button";
    copyBtn.className = "property-copy-btn";
    copyBtn.textContent = "复制";
    copyBtn.addEventListener("click", () => copyText(input.value));

    row.appendChild(label);
    row.appendChild(input);
    row.appendChild(copyBtn);
    fieldsBox.appendChild(row);
  });
}

async function showPropertiesModal(path) {
  if (!path) return;

  const modal = document.querySelector("#properties-modal");
  const title = document.querySelector("#properties-title");
  const fieldsBox = document.querySelector("#properties-fields");

  modal.classList.remove("hidden");
  title.textContent = "属性";
  fieldsBox.innerHTML = '<div class="properties-loading">正在读取文件属性...</div>';

  try {
    const info = await invoke("get_file_properties", { path });
    title.textContent = info.filename || "属性";
    renderPropertiesFields(info);
  } catch (err) {
    console.error("Load file properties failed:", err);
    fieldsBox.innerHTML = `<div class="properties-error">读取属性失败: ${err}</div>`;
  }
}

// --- CORE APP CONTROLLERS ---

// Boot initialization
async function initApp() {
  restoreSortPreference();
  await loadFavorites();

  // Sync preserved search input from hot-reloads
  const searchInput = document.querySelector("#search-input");
  if (searchInput && searchInput.value) {
    currentQuery = searchInput.value;
  }

  // Try to find a default favorite and boot search
  const defaultFav = favorites.find(f => f.is_default) || favorites[0];
  if (defaultFav) {
    activeFavorite = defaultFav.name;
    document.querySelector("#favorites-select").value = activeFavorite;

    // Perform initial search list
    await triggerSearch();
  }

  setupAudioPlayer();
  setupEventListeners();
  await setupWindowControls();
  if (activeFavorite && startupScanEnabled()) {
    void triggerIncrementalScan();
  }
}

function startupScanEnabled() {
  try { return localStorage.getItem("otto-sampler-startup-scan") !== "false"; }
  catch { return true; }
}

// Load favorites lists from database
async function loadFavorites() {
  try {
    favorites = await invoke("get_favorites");
    renderFavoritesDropdowns();
    renderFavoritesModalList();
  } catch (err) {
    console.error("Failed to load favorites:", err);
  }
}

// Render dropdown select list
function renderFavoritesDropdowns() {
  const select = document.querySelector("#favorites-select");

  // Preserve first placeholder
  select.innerHTML = '<option value="">-- 请选择路径收藏夹 --</option>';

  favorites.forEach(fav => {
    const opt = document.createElement("option");
    opt.value = fav.name;
    opt.textContent = `${fav.name} (${fav.paths.length}个路径)`;
    if (fav.name === activeFavorite) {
      opt.selected = true;
    }
    select.appendChild(opt);
  });
}

// Render list inside the modal manager
function renderFavoritesModalList() {
  const container = document.querySelector("#modal-favorites-list");
  container.innerHTML = "";

  if (favorites.length === 0) {
    container.innerHTML = '<div style="padding: 10px; color: var(--text-muted); text-align: center;">暂无已存收藏夹</div>';
    return;
  }

  favorites.forEach(fav => {
    const div = document.createElement("div");
    div.className = `modal-fav-item ${fav.is_default ? "default-fav" : ""}`;

    const nameSpan = document.createElement("span");
    nameSpan.className = "modal-fav-name";
    nameSpan.innerHTML = `${uiIcon("folder")} ${fav.name} ${fav.is_default ? '<span class="modal-fav-default-indicator">默认</span>' : ""}`;

    // Load into edit fields on click
    nameSpan.addEventListener("click", () => {
      document.querySelector("#fav-name-input").value = fav.name;
      modalPaths = [...fav.paths];
      renderModalPathsList();
    });

    const delBtn = document.createElement("button");
    delBtn.className = "modal-fav-delete";
    delBtn.innerHTML = uiIcon("trash");
    delBtn.setAttribute("aria-label", "删除收藏夹");
    delBtn.dataset.hint = "删除此收藏夹";
    delBtn.addEventListener("click", async (e) => {
      e.stopPropagation();
      if (confirm(`确定要删除收藏夹 "${fav.name}" 吗？`)) {
        try {
          await invoke("delete_favorite", { name: fav.name });
          if (activeFavorite === fav.name) {
            activeFavorite = "";
          }
          await loadFavorites();
        } catch (err) {
          alert("删除失败: " + err);
        }
      }
    });

    div.appendChild(nameSpan);
    div.appendChild(delBtn);
    container.appendChild(div);
  });
}

function renderModalPathsList() {
  const container = document.querySelector("#selected-paths-list");
  container.innerHTML = "";

  if (modalPaths.length === 0) {
    container.innerHTML = '<div class="no-paths-tip">暂无路径，请点击“添加路径”</div>';
    return;
  }

  modalPaths.forEach((path, idx) => {
    const item = document.createElement("div");
    item.className = "path-item";

    const textSpan = document.createElement("span");
    textSpan.className = "path-text";
    textSpan.textContent = path;

    const removeBtn = document.createElement("button");
    removeBtn.className = "path-remove-btn";
    removeBtn.innerHTML = "×";
    removeBtn.dataset.hint = "移除此路径";
    removeBtn.setAttribute("aria-label", "移除此路径");
    removeBtn.addEventListener("click", () => {
      modalPaths.splice(idx, 1);
      renderModalPathsList();
    });

    item.appendChild(textSpan);
    item.appendChild(removeBtn);
    container.appendChild(item);
  });
}

// Establish Sound Player mechanics
function setupAudioPlayer() {
  const playPauseBtn = document.querySelector("#play-pause-btn");
  const stopBtn = document.querySelector("#stop-btn");
  const progressSlider = document.querySelector("#progress-slider");
  const playerTimeText = document.querySelector("#player-time");
  const volumeSlider = document.querySelector("#volume-slider");
  const volIcon = document.querySelector("#vol-icon");
  const footer = document.querySelector(".player-footer");

  // Load saved volume
  const savedVolume = localStorage.getItem("otto-sampler-volume");
  if (savedVolume !== null) {
    volumeSlider.value = savedVolume;
  }
  const vol = volumeSlider.value / 100;
  audio.volume = vol;
  if (vol === 0) {
    volIcon.innerHTML = uiIcon("volume-off");
  } else if (vol < 0.5) {
    volIcon.innerHTML = uiIcon("volume-low");
  } else {
    volIcon.innerHTML = uiIcon("volume");
  }

  // Audio update loops
  audio.addEventListener("timeupdate", () => {
    if (!audio.duration) return;
    const progress = (audio.currentTime / audio.duration) * 100;
    progressSlider.value = progress;
    playerTimeText.textContent = `${formatTime(audio.currentTime)} / ${formatTime(audio.duration)}`;
  });

  audio.addEventListener("loadedmetadata", () => {
    duration = audio.duration;
    playerTimeText.textContent = `00:00 / ${formatTime(audio.duration)}`;
  });

  audio.addEventListener("ended", () => {
    resetPlayerState();
  });

  // Play Pause Toggle
  playPauseBtn.addEventListener("click", () => {
    if (!playingPath) return;
    togglePlayback();
  });

  // Stop playback completely
  stopBtn.addEventListener("click", () => {
    stopPlayback();
  });

  // Progress Seek Scrub
  progressSlider.addEventListener("input", (e) => {
    if (!audio.duration) return;
    const seekTime = (e.target.value / 100) * audio.duration;
    audio.currentTime = seekTime;
  });

  // Volume slider control
  volumeSlider.addEventListener("input", (e) => {
    const val = e.target.value;
    localStorage.setItem("otto-sampler-volume", val);
    const vol = val / 100;
    audio.volume = vol;
    if (vol === 0) {
      volIcon.innerHTML = uiIcon("volume-off");
    } else if (vol < 0.5) {
      volIcon.innerHTML = uiIcon("volume-low");
    } else {
      volIcon.innerHTML = uiIcon("volume");
    }
  });

  // Global keyboard spacebar play/pause hook
  window.addEventListener("keydown", (e) => {
    // Avoid space triggers when focusing inputs or textareas
    if (document.activeElement.tagName === "INPUT" || document.activeElement.tagName === "TEXTAREA") {
      return;
    }
    if (e.code === "Space") {
      e.preventDefault();
      if (playingPath) {
        togglePlayback();
      }
    }
  });
}

// Toggle play status
function togglePlayback() {
  const playPauseBtn = document.querySelector("#play-pause-btn");
  const footer = document.querySelector(".player-footer");

  if (isPlaying) {
    audio.pause();
    isPlaying = false;
    playPauseBtn.innerHTML = uiIcon("play");
    playPauseBtn.setAttribute("aria-label", "播放");
    footer.classList.remove("player-playing");
  } else {
    audio.play().catch(err => console.error("Play error:", err));
    isPlaying = true;
    playPauseBtn.innerHTML = uiIcon("pause");
    playPauseBtn.setAttribute("aria-label", "暂停");
    footer.classList.add("player-playing");
  }
  updateCardActiveStates();
}

// Stop audio play
function stopPlayback() {
  audio.pause();
  audio.currentTime = 0;
  isPlaying = false;
  document.querySelector("#play-pause-btn").innerHTML = uiIcon("play");
  document.querySelector("#play-pause-btn").setAttribute("aria-label", "播放");
  document.querySelector(".player-footer").classList.remove("player-playing");
  document.querySelector("#progress-slider").value = 0;
  document.querySelector("#player-time").textContent = "00:00 / 00:00";
  updateCardActiveStates();
}

// Reset player UI
function resetPlayerState() {
  isPlaying = false;
  document.querySelector("#play-pause-btn").innerHTML = uiIcon("play");
  document.querySelector("#play-pause-btn").setAttribute("aria-label", "播放");
  document.querySelector(".player-footer").classList.remove("player-playing");
  document.querySelector("#progress-slider").value = 0;
  updateCardActiveStates();
}

// A new search owns its own page state; late replies cannot append to a new query.
async function triggerSearch() {
  const list = document.querySelector("#results-list");
  const count = document.querySelector("#results-count");
  searchPageState = null;
  list.innerHTML = "";
  list.scrollTop = 0;
  list.setAttribute("aria-busy", "false");
  count.textContent = "0";
  if (!activeFavorite) {
    list.innerHTML = '<div class="empty-state"><h3>未选择路径收藏夹</h3><p>请在顶部选择路径收藏夹。</p></div>';
    return;
  }
  const state = {
    params: { favoriteName: activeFavorite, query: currentQuery, fileType: currentFilter, sortField: currentSortField, sortDir: currentSortDir },
    offset: 0, hasMore: true, loading: false, failed: false,
  };
  searchPageState = state;
  await loadNextSearchPage(state);
}

async function loadNextSearchPage(state = searchPageState, retry = false) {
  if (!state || state !== searchPageState || state.loading || !state.hasMore || (state.failed && !retry)) return;
  state.failed = false;
  const list = document.querySelector("#results-list");
  list.querySelector(".results-page-status")?.remove();
  const status = document.createElement("div");
  status.className = "results-page-status";
  status.textContent = "正在加载…";
  list.appendChild(status);
  state.loading = true;
  list.setAttribute("aria-busy", "true");
  try {
    const page = await invoke("search_files", { ...state.params, offset: state.offset });
    if (state !== searchPageState) return;
    status.remove();
    renderFileCards(page.records, state.offset > 0);
    state.offset += page.records.length;
    state.hasMore = page.hasMore;
    document.querySelector("#results-count").textContent = (state.hasMore ? "前" : "共") + state.offset;
  } catch (err) {
    if (state !== searchPageState) return;
    state.failed = true;
    console.error("Search error:", err);
    status.textContent = "";
    const retry = document.createElement("button");
    retry.className = "icon-btn";
    retry.textContent = "加载失败，点击重试";
    retry.addEventListener("click", () => loadNextSearchPage(state, true));
    status.appendChild(retry);
  } finally {
    state.loading = false;
    if (state === searchPageState) list.setAttribute("aria-busy", "false");
  }
}

// Render matching results list
function renderFileCards(records, append = false) {
  const list = document.querySelector("#results-list");
  if (!append) list.innerHTML = "";

  if (records.length === 0 && !append) {
    list.innerHTML = `
      <div class="empty-state">
        <span class="empty-icon">🔍</span>
        <h3>没有找到匹配的素材</h3>
        <p>请尝试搜索其他关键词，或运行“增量扫描”索引最新物理文件！</p>
      </div>
    `;
    return;
  }

  records.forEach(file => {
    const card = document.createElement("div");
    const isAudio = file.file_type === "audio";

    card.className = `sample-card ${isAudio ? "audio" : "video"}`;
    card.dataset.path = file.path;

    // Highlight if currently playing
    if (isAudio && file.path === playingPath) {
      card.classList.add("playing-active");
    }

    const ext = file.filename.split(".").pop().toUpperCase();
    const shortExt = ext.substring(0, 4);

    const iconDiv = document.createElement("div");
    iconDiv.className = "card-icon format-icon";

    iconDiv.textContent = shortExt;

    const durationSpan = document.createElement("span");
    durationSpan.className = "card-duration";
    durationSpan.textContent = formatTime(file.duration_secs || 0);

    const infoDiv = document.createElement("div");
    infoDiv.className = "card-info";

    const nameSpan = document.createElement("span");
    nameSpan.className = "card-name";
    nameSpan.title = file.filename;

    nameSpan.textContent = file.filename;

    const pathSpan = document.createElement("span");
    pathSpan.textContent = file.path;
    pathSpan.className = "card-path";
    pathSpan.style.display = "none"; // hidden path representation but searchable inside DOM

    infoDiv.appendChild(nameSpan);
    infoDiv.appendChild(pathSpan);

    card.appendChild(iconDiv);
    card.appendChild(durationSpan);
    card.appendChild(infoDiv);

    // --- EVENT BINDINGS FOR FILE CARD ---

    // 1. Play / Launch Trigger
    card.addEventListener("click", () => {
      if (isAudio) {
        playAudioFile(file.path, file.filename);
      } else {
        // Double click launches video; single click does nothing or previews path
        console.log("Single click on video path:", file.path);
      }
    });

    // 2. Video Double Click Open
    card.addEventListener("dblclick", () => {
      if (!isAudio) {
        openSystemFile(file.path);
      }
    });

    card.addEventListener("contextmenu", (e) => {
      showFileContextMenu(e, file.path);
    });

    // 3. OLE Drag out of application (Dual-trigger pointer-tracking implementation)
    let isPointerDown = false;
    let startX = 0;
    let startY = 0;
    const dragThreshold = 5;

    card.addEventListener("mousedown", (e) => {
      if (e.button !== 0) return; // Left click only
      isPointerDown = true;
      startX = e.clientX;
      startY = e.clientY;
    });

    card.addEventListener("mousemove", (e) => {
      if (!isPointerDown) return;
      const dx = Math.abs(e.clientX - startX);
      const dy = Math.abs(e.clientY - startY);

      if (dx > dragThreshold || dy > dragThreshold) {
        isPointerDown = false; // Reset
        startFileDrag(file.path);
      }
    });

    card.addEventListener("mouseup", () => {
      isPointerDown = false;
    });

    card.addEventListener("dragstart", (e) => {
      e.preventDefault();
      startFileDrag(file.path);
    });

    list.appendChild(card);
  });
}

// Drag file helper
async function startFileDrag(path) {
  // Convert standard forward slashes to Windows native backslashes to satisfy COM/OLE protocol
  const winPath = path.replace(/\//g, "\\");
  console.log("OLE dragout triggered for native path:", winPath);

  await startDrag({
    item: [winPath]
  });
}

// Handle audio clicking
function playAudioFile(path, filename) {
  const playPauseBtn = document.querySelector("#play-pause-btn");
  const footer = document.querySelector(".player-footer");

  if (playingPath === path) {
    // FL Studio logic: Click same active sound file starts playing from 0.0s immediately!
    audio.currentTime = 0;
    if (!isPlaying) {
      audio.play().catch(err => console.error("Play error:", err));
      isPlaying = true;
      playPauseBtn.innerHTML = uiIcon("pause");
    playPauseBtn.setAttribute("aria-label", "暂停");
      footer.classList.add("player-playing");
    }
  } else {
    // Load new sound path using Tauri's safe converted asset src url
    playingPath = path;
    const assetUrl = convertFileSrc(path);
    audio.src = assetUrl;

    document.querySelector("#playing-title").textContent = filename;

    audio.play()
      .then(() => {
        isPlaying = true;
        playPauseBtn.innerHTML = uiIcon("pause");
    playPauseBtn.setAttribute("aria-label", "暂停");
        footer.classList.add("player-playing");
      })
      .catch(err => {
        console.error("Play error:", err);
        resetPlayerState();
      });
  }
  updateCardActiveStates();
}

// Highlight the playing active file card
function updateCardActiveStates() {
  const cards = document.querySelectorAll(".sample-card");
  cards.forEach(card => {
    if (card.dataset.path === playingPath && isPlaying) {
      card.classList.add("playing-active");
    } else {
      card.classList.remove("playing-active");
    }
  });
}

// Incremental scan trigger
async function triggerIncrementalScan() {
  if (!activeFavorite) {
    alert("请选择一个路径收藏夹后再执行扫描！");
    return;
  }

  const scanBtn = document.querySelector("#scan-btn");
  if (scanBtn.disabled) return;
  const statusBar = document.querySelector("#scan-status-bar");
  const statsBar = document.querySelector("#scan-stats-bar");
  clearTimeout(scanStatsTimer);

  // UI Block
  scanBtn.disabled = true;
  scanBtn.classList.add("scan-spinning");
  statusBar.classList.remove("hidden");
  statsBar.classList.add("hidden");

  try {
    console.log("Invoking scan_favorite for:", activeFavorite);
    const result = await invoke("scan_favorite", { name: activeFavorite });

    // UI Unlock and populate results
    statusBar.classList.add("hidden");
    statsBar.classList.remove("hidden");

    document.querySelector("#stats-added").textContent = result.added;
    document.querySelector("#stats-updated").textContent = result.updated;
    document.querySelector("#stats-deleted").textContent = result.deleted;
    document.querySelector("#stats-total").textContent = result.total;
    scanStatsTimer = setTimeout(() => statsBar.classList.add("hidden"), 5000);

    // Refresh results list immediately!
    await triggerSearch();
  } catch (err) {
    console.error("Scan error:", err);
    alert("扫描索引失败: " + err);
    statusBar.classList.add("hidden");
  } finally {
    scanBtn.disabled = false;
    scanBtn.classList.remove("scan-spinning");
  }
}

// Bind standard events
function setupEventListeners() {
  setupResetSettings();
  const startupScan = document.getElementById("startup-scan");
  startupScan.checked = startupScanEnabled();
  startupScan.addEventListener("change", () => {
    localStorage.setItem("otto-sampler-startup-scan", String(startupScan.checked));
  });
  window.addEventListener("otto-reset-preferences", () => { startupScan.checked = true; });
  const resultList = document.querySelector("#results-list");
  resultList.addEventListener("scroll", () => {
    if (resultList.scrollHeight - resultList.scrollTop - resultList.clientHeight <= 32) {
      loadNextSearchPage();
    }
  }, { passive: true });
  const favSelect = document.querySelector("#favorites-select");
  const scanBtn = document.querySelector("#scan-btn");
  const closeStatsBtn = document.querySelector("#close-stats-btn");
  const searchInput = document.querySelector("#search-input");
  const clearBtn = document.querySelector("#clear-btn");
  const filterChips = document.querySelectorAll(".filter-chip");
  const sortSelect = document.querySelector("#sort-select");

  // Top bar toggles
  const favBtn = document.querySelector("#favorites-btn");
  const alwaysOnTopBtn = document.querySelector("#always-on-top-btn");
  const favModal = document.querySelector("#favorites-modal");
  const modalClose = document.querySelector("#modal-close-btn");
  const propertiesModal = document.querySelector("#properties-modal");
  const propertiesClose = document.querySelector("#properties-close-btn");

  // Selection change
  window.addEventListener("contextmenu", (e) => {
    if (!e.target.closest(".sample-card")) {
      e.preventDefault();
    }
  }, true);

  window.addEventListener("click", (e) => {
    if (!e.target.closest("#file-context-menu")) {
      hideFileContextMenu();
    }
  });

  window.addEventListener("scroll", hideFileContextMenu, true);

  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      hideFileContextMenu();
      closePropertiesModal();
    }
  });

  // Selection change
  favSelect.addEventListener("change", async (e) => {
    activeFavorite = e.target.value;
    document.querySelector("#scan-stats-bar").classList.add("hidden");
    await triggerSearch();
  });

  // Directory Scan
  scanBtn.addEventListener("click", () => {
    triggerIncrementalScan();
  });

  // Stats Close
  closeStatsBtn.addEventListener("click", () => {
    document.querySelector("#scan-stats-bar").classList.add("hidden");
  });

  // Modal overlay toggles
  favBtn.addEventListener("click", () => {
    favModal.classList.remove("hidden");
    document.querySelector("#fav-name-input").value = "";
    modalPaths = [];
    renderModalPathsList();
    renderFavoritesModalList();
  });

  modalClose.addEventListener("click", () => {
    favModal.classList.add("hidden");
  });

  // Close modal when clicking dark region
  favModal.addEventListener("click", (e) => {
    if (e.target === favModal) {
      favModal.classList.add("hidden");
    }
  });

  propertiesClose.addEventListener("click", closePropertiesModal);

  propertiesModal.addEventListener("click", (e) => {
    if (e.target === propertiesModal) {
      closePropertiesModal();
    }
  });

  // Add path handler
  const addPathBtn = document.querySelector("#add-path-btn");
  addPathBtn.addEventListener("click", async () => {
    try {
      const selected = await invoke("select_directory");
      if (selected) {
        if (!modalPaths.includes(selected)) {
          modalPaths.push(selected);
          renderModalPathsList();
        } else {
          alert("该路径已在列表中！");
        }
      }
    } catch (err) {
      console.error("Select directory error:", err);
      alert("选择路径失败: " + err);
    }
  });

  // Modal Favorite operations
  const saveFavBtn = document.querySelector("#fav-save-btn");
  const defFavBtn = document.querySelector("#fav-default-btn");

  saveFavBtn.addEventListener("click", async () => {
    const name = document.querySelector("#fav-name-input").value.trim();

    if (!name) {
      alert("请输入收藏夹组合名称！");
      return;
    }
    if (modalPaths.length === 0) {
      alert("请至少选择一个物理绝对路径！");
      return;
    }

    try {
      await invoke("add_favorite", { name, paths: modalPaths });
      alert("💾 收藏夹组合已保存成功！");

      // Clean inputs
      document.querySelector("#fav-name-input").value = "";
      modalPaths = [];
      renderModalPathsList();

      await loadFavorites();
    } catch (err) {
      alert("保存失败: " + err);
    }
  });

  defFavBtn.addEventListener("click", async () => {
    const name = document.querySelector("#fav-name-input").value.trim();
    if (!name) {
      alert("请从上方列表选择或输入一个需要设为默认的收藏夹名称！");
      return;
    }

    try {
      await invoke("set_default_favorite", { name });
      alert(`📌 已将 "${name}" 设为启动默认载入收藏夹！`);
      await loadFavorites();
    } catch (err) {
      alert("设为默认失败: " + err);
    }
  });

  // Real-time input search event
  let searchTimeout;
  searchInput.addEventListener("input", (e) => {
    currentQuery = e.target.value;
    searchPageState = null;
    clearTimeout(searchTimeout);
    searchTimeout = setTimeout(async () => {
      await triggerSearch();
    }, 200);
  });

  clearBtn.addEventListener("click", async () => {
    clearTimeout(searchTimeout);
    searchInput.value = "";
    currentQuery = "";
    searchInput.focus();
    await triggerSearch();
  });

  sortSelect.addEventListener("change", async (e) => {
    const [field, dir] = e.target.value.split(":");
    currentSortField = field;
    currentSortDir = dir;
    try { localStorage.setItem("otto-sampler-sort", `${field}:${dir}`); } catch (err) { console.warn("Cannot save sort preference", err); }
    await triggerSearch();
  });

  // Category chip filters
  filterChips.forEach(chip => {
    chip.addEventListener("click", async () => {
      filterChips.forEach(c => c.classList.remove("active"));
      chip.classList.add("active");
      currentFilter = chip.dataset.type;
      await triggerSearch();
    });
  });
}

// Initialise on load
window.addEventListener("DOMContentLoaded", () => {
  setupControlHints();
  initApp().catch(err => console.error("Initialisation error:", err));
});

function setupControlHints() {
  const output = document.getElementById("control-hint");
  const selector = "button, input[type=range], [role=button]";
  let hovered = null;
  let focused = null;
  let keyboardMode = false;
  const find = target => target instanceof Element ? target.closest(selector) : null;
  const render = () => {
    const control = hovered || (keyboardMode ? focused : null);
    output.textContent = control?.isConnected
      ? control.dataset.hint || control.getAttribute("aria-label") || control.textContent.trim()
      : "";
  };
  document.addEventListener("pointerover", event => { hovered = find(event.target); render(); });
  document.addEventListener("pointerout", event => { hovered = find(event.relatedTarget); render(); });
  document.addEventListener("pointerdown", () => { keyboardMode = false; render(); });
  document.addEventListener("keydown", event => { if (event.key === "Tab") keyboardMode = true; });
  document.addEventListener("focusin", event => { focused = find(event.target); render(); });
  document.addEventListener("focusout", event => { focused = find(event.relatedTarget); render(); });
  window.addEventListener("blur", () => { hovered = focused = null; render(); });
  new MutationObserver(render).observe(document.body, {
    subtree: true, attributes: true, attributeFilter: ["data-hint", "aria-label"]
  });
}

function restoreSortPreference() {
  const select = document.querySelector("#sort-select");
  try {
    const saved = localStorage.getItem("otto-sampler-sort");
    if (Array.from(select.options).some(option => option.value === saved)) {
      [currentSortField, currentSortDir] = saved.split(":");
      select.value = saved;
    }
  } catch (err) { console.warn("Cannot restore sort preference", err); }
}

async function setupWindowControls() {
  const currentWindow = window.__TAURI__.window.getCurrentWindow();
  const titlebar = document.querySelector(".titlebar");
  titlebar.addEventListener("mousedown", event => {
    if (event.button !== 0 || event.detail > 1 || event.target.closest(".controls")) return;
    event.preventDefault();
    invoke("begin_window_drag").catch(err => console.error("Window drag failed:", err));
  });
  titlebar.addEventListener("dblclick", event => {
    if (event.button !== 0 || event.target.closest(".controls")) return;
    currentWindow.toggleMaximize().catch(console.error);
  });
  const pin = document.querySelector("#always-on-top-btn");
  let pinState = { docked: false, alwaysOnTop: false };
  let busy = false;
  function renderPin(state) {
    pinState = state;
    pin.classList.toggle("active", state.alwaysOnTop);
    pin.setAttribute("aria-pressed", String(state.alwaysOnTop));
    pin.disabled = busy || state.docked;
    pin.dataset.hint = state.docked ? "侧边收纳时自动置顶" : state.alwaysOnTop ? "取消置顶" : "置顶";
  }
  pin.addEventListener("click", async () => {
    if (busy || pinState.docked) return;
    busy = true;
    renderPin(pinState);
    try { renderPin(await invoke("toggle_window_pin")); }
    catch (err) { alert("切换窗口置顶失败: " + err); }
    finally { busy = false; renderPin(pinState); }
  });
  for (const [id, action] of [["minimize-btn", "minimize"], ["maximize-btn", "toggleMaximize"], ["close-btn", "close"]]) {
    document.getElementById(id).addEventListener("click", () => {
      currentWindow[action]().catch(err => alert("窗口操作失败: " + err));
    });
  }
  async function updateMaximize() {
    const maximized = await currentWindow.isMaximized();
    const button = document.getElementById("maximize-btn");
    button.dataset.hint = maximized ? "还原" : "最大化";
    button.setAttribute("aria-label", button.dataset.hint);
    button.innerHTML = maximized
      ? '<svg viewBox="0 0 16 16"><path d="M5 3V1h10v10h-2"/><rect x="1" y="5" width="10" height="10"/></svg>'
      : '<svg viewBox="0 0 16 16"><rect x="3" y="3" width="10" height="10"/></svg>';
  }
  await window.__TAURI__.event.listen("window-pin-state", event => renderPin(event.payload));
  renderPin(await invoke("get_window_pin_state"));
  await currentWindow.onResized(() => updateMaximize().catch(console.error));
  await updateMaximize();
}

function setupResetSettings() {
  const panel = document.getElementById("settings-confirm");
  const message = document.getElementById("settings-confirm-text");
  const status = document.getElementById("settings-status");
  const apply = document.getElementById("settings-confirm-apply");
  const cancel = document.getElementById("settings-confirm-cancel");
  const preferences = document.getElementById("reset-preferences-btn");
  const index = document.getElementById("reset-index-btn");
  let pending = null;
  let busy = false;
  function dismiss() { pending = null; panel.classList.add("hidden"); }
  document.getElementById("settings-btn").addEventListener("click", () => {
    dismiss();
    if (!busy) status.textContent = "";
  });
  function request(kind) {
    if (busy) return;
    dismiss();
    status.textContent = "";
    if (kind === "index" && document.getElementById("scan-btn").disabled) {
      status.textContent = "扫描进行中，请完成后再重置索引。";
      return;
    }
    pending = kind;
    message.textContent = kind === "preferences"
      ? "恢复默认主题（MPC60、随系统）、文件名 A-Z 排序、80% 音量，并开启启动扫描？"
      : "清空所有已扫描的文件索引？收藏路径和素材文件会保留，之后需重新扫描。";
    panel.classList.remove("hidden");
    cancel.focus();
  }
  preferences.addEventListener("click", () => request("preferences"));
  index.addEventListener("click", () => request("index"));
  cancel.addEventListener("click", () => { dismiss(); preferences.focus(); });
  apply.addEventListener("click", async () => {
    if (!pending || busy) return;
    const kind = pending;
    busy = true;
    for (const button of [preferences, index, apply, cancel]) button.disabled = true;
    status.textContent = "正在重置…";
    try {
      if (kind === "preferences") {
        for (const key of ["otto-sampler-theme", "otto-sampler-palette", "otto-sampler-sort", "otto-sampler-volume", "otto-sampler-startup-scan"]) localStorage.removeItem(key);
        window.dispatchEvent(new Event("otto-reset-preferences"));
        currentSortField = "filename";
        currentSortDir = "asc";
        document.getElementById("sort-select").value = "filename:asc";
        audio.volume = 0.8;
        document.getElementById("volume-slider").value = "80";
        document.getElementById("vol-icon").innerHTML = uiIcon("volume");
        await triggerSearch();
        status.textContent = "偏好设置已恢复默认。";
      } else {
        await invoke("reset_file_index");
        document.getElementById("scan-stats-bar").classList.add("hidden");
        await triggerSearch();
        status.textContent = "索引已清空，点击增量扫描重新建立。";
      }
      dismiss();
    } catch (err) {
      status.textContent = "重置失败：" + err;
    } finally {
      busy = false;
      for (const button of [preferences, index, apply, cancel]) button.disabled = false;
    }
  });
}
