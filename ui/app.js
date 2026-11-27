(() => {
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;
  const currentWebview = window.__TAURI__.webview?.getCurrentWebview?.();

  let currentState = {
    webview2Version: '',
    profilesDir: '',
    preferences: { theme: 'system', language: 'id' },
    profiles: [],
    colorPresets: ['#b91c1c', '#1d4ed8', '#0f766e', '#6d28d9', '#b45309', '#047857', '#be185d', '#334155'],
    tabs: [],
    activeTabId: null,
    activeAwakeCount: 0,
    totalMemoryMb: 0,
    downloads: [],
    workspaces: [],
    profilePanelOpen: false,
    downloadPanelOpen: false,
    monitorPanelOpen: false,
    findBarOpen: false,
    findQuery: ''
  };
  let selectedModalColor = '#1d4ed8';
  let activeDrag = null;
  let suppressTabClick = false;
  let lastRenderedIds = '';
  let cachedAppInfo = null;
  let lastUpdateCheck = null;
  let updateProgress = null;
  const RELEASES_URL = 'https://github.com/insansimple/roletab/releases/latest';
  const mediaDark = window.matchMedia('(prefers-color-scheme: dark)');

  function t(key) {
    const lang = currentState.preferences?.language || 'id';
    return (window.ROLETAB_I18N[lang] || window.ROLETAB_I18N.id)[key] || key;
  }

  function applyTheme() {
    const mode = currentState.preferences?.theme || 'system';
    const resolved = mode === 'system' ? (mediaDark.matches ? 'dark' : 'light') : mode;
    document.documentElement.setAttribute('data-theme', resolved);

    const themeIcon = document.getElementById('themeIcon');
    if (mode === 'dark') themeIcon.className = 'fa-solid fa-moon';
    else if (mode === 'light') themeIcon.className = 'fa-solid fa-sun';
    else themeIcon.className = 'fa-solid fa-circle-half-stroke';
  }

  function applyStaticTranslations() {
    const lang = currentState.preferences?.language || 'id';
    document.documentElement.lang = lang;

    const setAriaTitle = (id, ariaKey, titleKey = ariaKey) => {
      const el = document.getElementById(id);
      if (!el) return;
      el.setAttribute('aria-label', t(ariaKey));
      el.title = t(titleKey);
    };

    setAriaTitle('newTabBtn', 'newTab', 'newTabTooltip');
    setAriaTitle('profileMgrBtn', 'profileManager');
    setAriaTitle('overflowBtn', 'allTabs');
    setAriaTitle('backBtn', 'back');
    setAriaTitle('forwardBtn', 'forward');
    setAriaTitle('homeBtn', 'home');
    setAriaTitle('goBtn', 'goUrl');
    setAriaTitle('newEphemBtn', 'newEphemeralTab');
    setAriaTitle('downloadsBtn', 'downloadsTitle');
    setAriaTitle('monitorBtn', 'taskMonitorTitle');
    setAriaTitle('awakeWarnBtn', 'awakeWarnTooltip');
    setAriaTitle('updateReadyBtn', 'updateReadyTooltip');
    setAriaTitle('themeBtn', 'themeTooltip');
    setAriaTitle('menuBtn', 'menuLabel', 'menuTooltip');
    setAriaTitle('closeProfileModalBtn', 'closeModal');
    setAriaTitle('closeAboutModalBtn', 'closeModal');
    setAriaTitle('zoomPill', 'zoomResetTooltip');
    setAriaTitle('findPrevBtn', 'findPrev');
    setAriaTitle('findNextBtn', 'findNext');
    setAriaTitle('findCloseBtn', 'closeModal');
    document.getElementById('findInput').placeholder = t('findPlaceholder');

    document.getElementById('newProfileModalTitle').textContent = t('newProfileTitle');
    document.getElementById('newProfileInputLabel').textContent = t('newProfileLabel');
    document.getElementById('newProfileColorLabel').textContent = t('colorLabel');
    document.getElementById('newProfileNameInput').placeholder = t('newProfilePlaceholder');
    document.getElementById('newProfileHint').textContent = t('newProfileHint');
    document.getElementById('cancelProfileModalBtn').textContent = t('cancel');
    document.getElementById('submitProfileModalBtn').textContent = t('createProfileBtn');

    document.getElementById('aboutModalTitle').textContent = t('aboutTitle');
    document.getElementById('aboutUpdateHeadingText').textContent = t('updateSectionTitle');
    document.getElementById('aboutProfilesDirLabel').textContent = t('aboutDataDirLabel');
    document.getElementById('closeAboutFooterBtn').textContent = t('closeModal');
    document.getElementById('openReleasesLabel').textContent = t('openReleasesBtn');
    document.getElementById('checkUpdatesLabel').textContent = t('checkUpdatesBtn');
    renderUpdateToolbarPill();
    if (!document.getElementById('aboutModalBackdrop').hidden) {
      renderAboutInfo();
    }
    document.getElementById('crashCloseTabBtn').textContent = t('closeTab');
    document.getElementById('crashRecoverBtn').textContent = t('recoverTabBtn');

    const langBtn = document.getElementById('langBtn');
    langBtn.textContent = t('langLabel');
    langBtn.title = t('langTooltip');

    const urlInput = document.getElementById('urlInput');
    urlInput.placeholder = t('addressPlaceholder');
    urlInput.setAttribute('aria-label', t('addressLabel'));
  }

  function shortProfileLabel(name, isEphemeral) {
    if (isEphemeral) return 'eph';
    const raw = (name || '').trim();
    const lower = raw.toLowerCase();
    if (lower === 'admin') return 'adm';
    if (lower === 'user') return 'usr';
    if (lower.length <= 3) return lower;
    const alnum = lower.replace(/[^a-z0-9]/g, '');
    return (alnum || lower).slice(0, 3);
  }

  function roleStyle(tab) {
    if (tab.isEphemeral) {
      return {
        cls: 'ephem',
        color: tab.profileColor || '#4b5563',
        label: t('ephemeralLabel'),
        short: 'eph',
        icon: 'fa-user-secret'
      };
    }
    const lower = (tab.profileKey || '').toLowerCase();
    const short = shortProfileLabel(tab.profileKey, false);
    const color = tab.profileColor || '#1d4ed8';
    const icon = lower === 'admin' ? 'fa-user-shield' : (lower === 'user' ? 'fa-user' : 'fa-user-tag');
    return { cls: 'custom', color, label: tab.profileKey, short, icon };
  }

  function updateProtocolIcon(rawUrl, hasCertWarning) {
    const icon = document.getElementById('protoIcon');
    icon.className = 'fa-solid proto-icon';
    if (hasCertWarning) {
      icon.classList.add('fa-triangle-exclamation', 'proto-warn');
      icon.title = t('protoCertWarn');
    } else if (!rawUrl || rawUrl === 'about:blank') {
      icon.classList.add('fa-compass', 'proto-blank');
      icon.title = t('protoBlank');
    } else if (rawUrl.startsWith('https://')) {
      icon.classList.add('fa-lock', 'proto-https');
      icon.title = t('protoHttps');
    } else if (rawUrl.includes('127.0.0.1') || rawUrl.includes('localhost')) {
      icon.classList.add('fa-server', 'proto-local');
      icon.title = t('protoLocal');
    } else {
      icon.classList.add('fa-globe', 'proto-http');
      icon.title = t('protoHttp');
    }
  }

  function createFallbackIcon(style) {
    const icon = document.createElement('i');
    icon.className = `fa-solid ${style.icon} tab-lead-icon`;
    icon.setAttribute('aria-hidden', 'true');
    icon.style.fontSize = '11px';
    return icon;
  }

  function syncTabLeadIcon(el, tab, style) {
    const existing = el.querySelector('.tab-lead-icon, .tab-favicon, .tab-lead-spinner');
    const isLoading = Boolean(tab.isLoading);
    const desiredFav = tab.faviconUrl || '';
    if (
      existing &&
      existing.dataset.loading === String(isLoading) &&
      existing.dataset.favUrl === desiredFav
    ) {
      return;
    }

    let nextNode;
    if (isLoading) {
      const spinner = document.createElement('i');
      spinner.className = 'fa-solid fa-circle-notch fa-spin tab-lead-spinner';
      spinner.setAttribute('aria-hidden', 'true');
      spinner.dataset.loading = 'true';
      spinner.dataset.favUrl = desiredFav;
      nextNode = spinner;
    } else if (!desiredFav) {
      nextNode = createFallbackIcon(style);
      nextNode.dataset.loading = 'false';
      nextNode.dataset.favUrl = '';
    } else {
      const img = document.createElement('img');
      img.className = 'tab-favicon';
      img.src = desiredFav;
      img.alt = '';
      img.dataset.loading = 'false';
      img.dataset.favUrl = desiredFav;
      img.onerror = () => {
        const fb = createFallbackIcon(style);
        fb.dataset.loading = 'false';
        fb.dataset.favUrl = desiredFav;
        img.replaceWith(fb);
      };
      nextNode = img;
    }

    if (existing) {
      existing.replaceWith(nextNode);
    } else {
      el.prepend(nextNode);
    }
  }

  function clearTabDropIndicators() {
    document.querySelectorAll('#tabs .tab').forEach((node) => {
      node.classList.remove('drag-over', 'drag-over-right');
    });
  }

  function startTabDrag(el, startEvent) {
    if (startEvent.button !== 0) return;
    if (startEvent.target?.closest?.('.tab-close')) return;

    const tabsContainer = document.getElementById('tabs');
    const tabEls = Array.from(tabsContainer.querySelectorAll('.tab'));
    const fromIndex = Number(el.dataset.tabIndex);
    if (!Number.isInteger(fromIndex) || tabEls.length < 2) return;

    const restingMids = tabEls.map((node, idx) => {
      const rect = node.getBoundingClientRect();
      return { idx, mid: rect.left + rect.width / 2 };
    });

    const startX = startEvent.clientX;
    let dragging = false;
    let targetIndex = fromIndex;
    activeDrag = { fromIndex, tabId: el.dataset.tabId, dragging: false };

    const updateTarget = (clientX) => {
      let insertSlot = 0;
      for (const item of restingMids) {
        if (item.idx === fromIndex) continue;
        if (item.mid < clientX) insertSlot += 1;
      }
      targetIndex = insertSlot;
      clearTabDropIndicators();
      if (targetIndex < fromIndex && tabEls[targetIndex]) {
        tabEls[targetIndex].classList.add('drag-over');
      } else if (targetIndex > fromIndex && tabEls[targetIndex]) {
        tabEls[targetIndex].classList.add('drag-over-right');
      }
    };

    const onPointerMove = (moveEvent) => {
      const dx = moveEvent.clientX - startX;
      if (!dragging) {
        if (Math.abs(dx) <= 6) return;
        dragging = true;
        if (activeDrag) activeDrag.dragging = true;
        el.classList.add('dragging');
        document.body.style.userSelect = 'none';
        try {
          el.setPointerCapture?.(startEvent.pointerId);
        } catch {}
      }
      el.style.transform = `translateX(${dx}px)`;
      updateTarget(moveEvent.clientX);
    };

    const finishDrag = async (commit) => {
      window.removeEventListener('pointermove', onPointerMove);
      window.removeEventListener('pointerup', onPointerUp);
      window.removeEventListener('pointercancel', onPointerCancel);
      activeDrag = null;
      document.body.style.userSelect = '';
      el.classList.remove('dragging');
      el.style.transform = '';
      clearTabDropIndicators();
      try {
        if (el.hasPointerCapture?.(startEvent.pointerId)) {
          el.releasePointerCapture(startEvent.pointerId);
        }
      } catch {}

      if (dragging) {
        suppressTabClick = true;
        setTimeout(() => { suppressTabClick = false; }, 0);
        if (commit && targetIndex !== fromIndex) {
          await invoke('reorder_tab', { fromIndex, toIndex: targetIndex });
        }
      }
    };

    const onPointerUp = () => { void finishDrag(true); };
    const onPointerCancel = () => { void finishDrag(false); };

    window.addEventListener('pointermove', onPointerMove);
    window.addEventListener('pointerup', onPointerUp);
    window.addEventListener('pointercancel', onPointerCancel);
  }

  function buildTabElement(tab, index) {
    const style = roleStyle(tab);
    const el = document.createElement('div');
    el.dataset.tabId = tab.id;
    el.dataset.tabIndex = String(index);
    el.setAttribute('role', 'tab');

    el.addEventListener('click', () => {
      if (suppressTabClick) {
        suppressTabClick = false;
        return;
      }
      switchTab(el.dataset.tabId);
    });
    el.addEventListener('contextmenu', async (e) => {
      e.preventDefault();
      await invoke('show_tab_context_menu', {
        tabId: el.dataset.tabId,
        x: e.clientX,
        y: e.clientY
      });
    });
    el.addEventListener('keydown', (e) => {
      const curIdx = Number(el.dataset.tabIndex);
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        switchTab(el.dataset.tabId);
      } else if (e.key === 'ArrowRight' && currentState.tabs.length > 1) {
        e.preventDefault();
        const next = currentState.tabs[(curIdx + 1) % currentState.tabs.length];
        switchTab(next.id);
      } else if (e.key === 'ArrowLeft' && currentState.tabs.length > 1) {
        e.preventDefault();
        const prev = currentState.tabs[(curIdx - 1 + currentState.tabs.length) % currentState.tabs.length];
        switchTab(prev.id);
      }
    });
    el.addEventListener('auxclick', (e) => {
      if (e.button === 1) {
        e.preventDefault();
        closeTab(el.dataset.tabId);
      }
    });
    el.addEventListener('pointerdown', (e) => startTabDrag(el, e));

    const leadIcon = createFallbackIcon(style);
    leadIcon.dataset.favUrl = '';

    const title = document.createElement('span');
    title.className = 'tab-title';

    const badge = document.createElement('span');
    badge.className = `tab-role ${style.cls}`;

    const pinIcon = document.createElement('i');
    pinIcon.className = 'fa-solid fa-thumbtack tab-pin-icon';
    pinIcon.setAttribute('aria-hidden', 'true');
    pinIcon.hidden = true;

    const audioIcon = document.createElement('i');
    audioIcon.className = 'fa-solid fa-volume-high tab-audio-icon';
    audioIcon.setAttribute('aria-hidden', 'true');
    audioIcon.hidden = true;

    const crashIcon = document.createElement('i');
    crashIcon.className = 'fa-solid fa-triangle-exclamation tab-crash-icon';
    crashIcon.setAttribute('aria-hidden', 'true');
    crashIcon.hidden = true;

    const sleepIcon = document.createElement('i');
    sleepIcon.className = 'fa-solid fa-moon tab-sleep-icon';
    sleepIcon.setAttribute('aria-hidden', 'true');
    sleepIcon.hidden = true;

    const close = document.createElement('button');
    close.type = 'button';
    close.className = 'tab-close';
    close.addEventListener('click', (e) => {
      e.stopPropagation();
      closeTab(el.dataset.tabId);
    });
    const closeIcon = document.createElement('i');
    closeIcon.className = 'fa-solid fa-xmark';
    closeIcon.setAttribute('aria-hidden', 'true');
    close.appendChild(closeIcon);

    el.append(leadIcon, title, badge, pinIcon, audioIcon, crashIcon, sleepIcon, close);
    return el;
  }

  function updateTabElement(el, tab, index) {
    const style = roleStyle(tab);
    const isActive = tab.id === currentState.activeTabId;
    const isDragging = Boolean(activeDrag?.dragging && activeDrag.tabId === tab.id);
    el.dataset.tabIndex = String(index);
    el.className = 'tab' + (isActive ? ' active' : '') + (tab.isSleeping ? ' sleeping' : '') + (isDragging ? ' dragging' : '');
    el.setAttribute('aria-selected', String(isActive));
    el.tabIndex = isActive ? 0 : -1;

    syncTabLeadIcon(el, tab, style);

    const badge = el.querySelector('.tab-role');
    badge.className = `tab-role ${style.cls}`;
    badge.style.backgroundColor = style.color;
    badge.textContent = style.short;
    badge.title = style.label;

    const title = el.querySelector('.tab-title');
    const titleLabel = tab.title && tab.title !== 'about:blank' ? tab.title : t('tabDefault');
    title.textContent = titleLabel;
    const tipParts = [titleLabel];
    if (tab.browserPid) tipParts.push(`PID ${tab.browserPid}`);
    if (typeof tab.memoryMb === 'number') tipParts.push(`${tab.memoryMb} MB`);
    if (tab.isPinned) tipParts.push(t('statusPinned'));
    if (tab.isPlayingAudio) tipParts.push('Audio');
    title.title = tipParts.join(' | ');

    const pinIcon = el.querySelector('.tab-pin-icon');
    pinIcon.hidden = !tab.isPinned;
    pinIcon.title = t('statusPinned');

    const audioIcon = el.querySelector('.tab-audio-icon');
    audioIcon.hidden = !tab.isPlayingAudio;
    audioIcon.title = 'Audio';

    const crashIcon = el.querySelector('.tab-crash-icon');
    crashIcon.hidden = !tab.crashKind;
    crashIcon.title = t('statusCrashed');

    const sleepIcon = el.querySelector('.tab-sleep-icon');
    sleepIcon.hidden = !tab.isSleeping;
    sleepIcon.title = t('sleeping');

    const close = el.querySelector('.tab-close');
    close.setAttribute('aria-label', t('closeTab'));
    close.title = t('closeTabTooltip');
  }

  function render() {
    applyTheme();
    applyStaticTranslations();

    const mgrBtn = document.getElementById('profileMgrBtn');
    if (mgrBtn) {
      mgrBtn.classList.toggle('active', Boolean(currentState.profilePanelOpen));
    }

    const dlBtn = document.getElementById('downloadsBtn');
    if (dlBtn) {
      dlBtn.classList.toggle('active', Boolean(currentState.downloadPanelOpen));
    }
    const monBtn = document.getElementById('monitorBtn');
    if (monBtn) {
      monBtn.classList.toggle('active', Boolean(currentState.monitorPanelOpen));
    }
    const dlDot = document.getElementById('dlActiveDot');
    if (dlDot) {
      dlDot.hidden = !(currentState.downloads || []).some((d) => d.status === 'inProgress');
    }

    const awakeCount = typeof currentState.activeAwakeCount === 'number'
      ? currentState.activeAwakeCount
      : currentState.tabs.filter((tab) => !tab.isSleeping).length;
    const awakeBtn = document.getElementById('awakeWarnBtn');
    if (awakeBtn) {
      awakeBtn.hidden = awakeCount <= 12;
      document.getElementById('awakeWarnText').textContent = `${awakeCount} ${t('awakeWarnSuffix')}`;
    }

    renderUpdateToolbarPill();

    const container = document.getElementById('tabs');
    const nextIds = currentState.tabs.map(tab => tab.id).join(',');
    if (nextIds !== lastRenderedIds) {
      container.innerHTML = '';
      currentState.tabs.forEach((tab, index) => {
        const el = buildTabElement(tab, index);
        updateTabElement(el, tab, index);
        container.appendChild(el);
      });
      lastRenderedIds = nextIds;
    } else {
      currentState.tabs.forEach((tab, index) => {
        const el = container.children[index];
        if (el) updateTabElement(el, tab, index);
      });
    }

    const activeIndex = currentState.tabs.findIndex(tab => tab.id === currentState.activeTabId);
    if (activeIndex >= 0 && container.children[activeIndex]) {
      container.children[activeIndex].scrollIntoView({ block: 'nearest', inline: 'nearest' });
    }

    const activeTab = activeIndex >= 0 ? currentState.tabs[activeIndex] : null;
    const urlInput = document.getElementById('urlInput');
    const backBtn = document.getElementById('backBtn');
    const forwardBtn = document.getElementById('forwardBtn');
    const reloadBtn = document.getElementById('reloadBtn');
    const reloadIcon = document.getElementById('reloadIcon');

    if (activeTab) {
      if (document.activeElement !== urlInput) {
        urlInput.value = activeTab.url === 'about:blank' ? '' : activeTab.url;
      }
      updateProtocolIcon(activeTab.url, activeTab.hasCertWarning);
      backBtn.disabled = !activeTab.canGoBack;
      forwardBtn.disabled = !activeTab.canGoForward;

      if (activeTab.isLoading) {
        reloadIcon.className = 'fa-solid fa-xmark';
        reloadBtn.title = t('stop');
        reloadBtn.setAttribute('aria-label', t('stop'));
      } else {
        reloadIcon.className = 'fa-solid fa-rotate-right';
        reloadBtn.title = t('reloadTooltip');
        reloadBtn.setAttribute('aria-label', t('reload'));
      }
    }

    const crashScreen = document.getElementById('crashScreen');
    if (activeTab && activeTab.crashKind) {
      document.body.classList.add('crash-open');
      crashScreen.hidden = false;
      const titleEl = document.getElementById('crashTitleText');
      const descEl = document.getElementById('crashDescText');
      if (activeTab.crashKind === 'browserProcessExited') {
        titleEl.textContent = t('crashTitleBrowser');
        descEl.textContent = t('crashDescBrowser');
      } else if (activeTab.crashKind === 'renderProcessUnresponsive') {
        titleEl.textContent = t('crashTitleUnresponsive');
        descEl.textContent = t('crashDescUnresponsive');
      } else {
        titleEl.textContent = t('crashTitleRender');
        descEl.textContent = t('crashDescRender');
      }
      document.getElementById('crashUrlText').textContent = `${activeTab.profileKey} | ${activeTab.url}`;
    } else {
      document.body.classList.remove('crash-open');
      crashScreen.hidden = true;
    }

    const zoom = activeTab && typeof activeTab.zoomFactor === 'number' ? activeTab.zoomFactor : 1.0;
    const zoomPill = document.getElementById('zoomPill');
    if (zoomPill) {
      zoomPill.hidden = Math.abs(zoom - 1.0) < 0.01;
      zoomPill.textContent = `${Math.round(zoom * 100)}%`;
    }

    const findOpen = Boolean(currentState.findBarOpen);
    document.body.classList.toggle('find-open', findOpen);
    document.getElementById('findBar').hidden = !findOpen;
  }

  async function openTab(profileKey, isEphemeral) {
    await invoke('create_tab', { profileKey, isEphemeral, url: null });
  }

  async function openTabWithActiveRole() {
    const activeTab = currentState.tabs.find(tab => tab.id === currentState.activeTabId);
    if (activeTab) {
      return openTab(activeTab.isEphemeral ? 'ephemeral' : activeTab.profileId, activeTab.isEphemeral);
    }
    return openTab('ephemeral', true);
  }

  async function switchTab(tabId) {
    await invoke('switch_tab', { tabId });
  }

  async function closeTab(tabId) {
    await invoke('close_tab', { tabId });
  }

  async function navigateUrl() {
    const url = document.getElementById('urlInput').value.trim();
    if (!url) return;
    await invoke('navigate_active_tab', { url });
  }

  async function goBack() { await invoke('go_back_active_tab'); }
  async function goForward() { await invoke('go_forward_active_tab'); }
  async function goHome() { await invoke('go_home_active_tab'); }

  async function reloadOrStop() {
    const activeTab = currentState.tabs.find(tab => tab.id === currentState.activeTabId);
    if (activeTab && activeTab.isLoading) {
      await invoke('stop_active_tab');
    } else {
      await invoke('reload_active_tab');
    }
  }

  async function openDevtools() {
    await invoke('open_active_devtools');
  }

  async function openNewTabMenu() {
    const rect = document.getElementById('newTabBtn').getBoundingClientRect();
    await invoke('show_new_tab_menu', { x: rect.left, y: rect.bottom });
  }

  async function openAppMenu() {
    const rect = document.getElementById('menuBtn').getBoundingClientRect();
    await invoke('show_app_menu', { x: rect.right, y: rect.bottom });
  }

  async function openOverflowMenu() {
    await invoke('show_tab_overflow_menu');
  }

  async function toggleProfilePanel() {
    await invoke('toggle_profile_panel');
  }

  async function toggleDownloadPanel() {
    await invoke('toggle_download_panel');
  }

  async function toggleMonitorPanel() {
    await invoke('toggle_monitor_panel');
  }

  async function recoverActiveTab() {
    if (currentState.activeTabId) {
      await invoke('recover_tab', { tabId: currentState.activeTabId });
    }
  }

  async function sleepInactiveTabs() {
    await invoke('sleep_inactive_tabs');
  }

  async function adjustZoom(delta) {
    await invoke('adjust_active_tab_zoom', { delta });
  }

  async function runFind(forward = true) {
    const inp = document.getElementById('findInput');
    const q = inp.value;
    const count = await invoke('find_in_active_tab', { query: q, forward });
    const countEl = document.getElementById('findCountText');
    countEl.textContent = count > 0 ? `${count} ${t('findMatchSuffix')}` : t('findNoMatch');
  }

  async function openFindBar() {
    await invoke('set_find_bar_open', { open: true });
    if (currentWebview && !document.hasFocus()) {
      await currentWebview.setFocus().catch(() => {});
    }
    const inp = document.getElementById('findInput');
    inp.focus();
    inp.select();
  }

  async function closeFindBar() {
    document.getElementById('findCountText').textContent = t('findNoMatch');
    await invoke('set_find_bar_open', { open: false });
  }

  function renderModalColorRow() {
    const row = document.getElementById('newProfileColorRow');
    if (!row) return;
    row.innerHTML = '';
    const presets = currentState.colorPresets || [];
    presets.forEach((hex) => {
      const btn = document.createElement('button');
      btn.type = 'button';
      btn.className = 'swatch-btn' + (selectedModalColor.toLowerCase() === hex.toLowerCase() ? ' selected' : '');
      btn.style.backgroundColor = hex;
      btn.setAttribute('aria-label', hex);
      if (selectedModalColor.toLowerCase() === hex.toLowerCase()) {
        btn.innerHTML = '<i class="fa-solid fa-check" aria-hidden="true"></i>';
      }
      btn.addEventListener('click', () => {
        selectedModalColor = hex;
        renderModalColorRow();
      });
      row.appendChild(btn);
    });
  }

  async function openNewProfileModal() {
    const backdrop = document.getElementById('profileModalBackdrop');
    const input = document.getElementById('newProfileNameInput');
    const err = document.getElementById('newProfileError');

    err.textContent = '';
    err.hidden = true;
    input.value = '';
    const presets = currentState.colorPresets || [];
    if (presets.length > 0) {
      selectedModalColor = presets[currentState.profiles.length % presets.length];
    }
    renderModalColorRow();
    document.body.classList.add('modal-open');
    backdrop.hidden = false;

    await invoke('set_shell_overlay_open', { open: true });
    if (currentWebview && !document.hasFocus()) {
      await currentWebview.setFocus().catch(() => {});
    }
    input.focus();
  }

  async function closeNewProfileModal() {
    const backdrop = document.getElementById('profileModalBackdrop');
    backdrop.hidden = true;
    document.body.classList.remove('modal-open');
    await invoke('set_shell_overlay_open', { open: false });
  }

  function formatUpdateBytes(bytes) {
    if (typeof bytes !== 'number' || Number.isNaN(bytes) || bytes < 0) return '0 KB';
    if (bytes >= 1024 * 1024) {
      return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
    }
    return `${Math.max(1, Math.round(bytes / 1024))} KB`;
  }

  function renderUpdateToolbarPill() {
    const updBtn = document.getElementById('updateReadyBtn');
    const updLabel = document.getElementById('updateReadyLabel');
    if (!updBtn || !updLabel) return;

    const stage = updateProgress?.stage;
    if (stage === 'downloading') {
      updBtn.hidden = false;
      const pct = typeof updateProgress.percent === 'number' ? ` ${updateProgress.percent}%` : '...';
      updLabel.textContent = `${t('updateDownloadingLabel')}${pct}`;
      return;
    }
    if (stage === 'installing') {
      updBtn.hidden = false;
      updLabel.textContent = t('installingBtn');
      return;
    }
    if (lastUpdateCheck && lastUpdateCheck.available) {
      updBtn.hidden = false;
      updLabel.textContent = lastUpdateCheck.latestVersion
        ? `${t('updateBtnLabel')} v${lastUpdateCheck.latestVersion}`
        : t('updateBtnLabel');
      return;
    }
    updBtn.hidden = true;
    updLabel.textContent = t('updateBtnLabel');
  }

  function renderAboutInfo() {
    const metaEl = document.getElementById('aboutMetaLine');
    const dirEl = document.getElementById('aboutProfilesDirBox');
    const badgeEl = document.getElementById('updateVersionBadge');
    const statusIcon = document.getElementById('updateStatusIcon');
    const statusText = document.getElementById('aboutUpdateStatusText');
    const progressWrap = document.getElementById('updateProgressWrap');
    const progressTrack = document.getElementById('updateProgressTrack');
    const progressFill = document.getElementById('updateProgressFill');
    const progressStage = document.getElementById('updateProgressStageText');
    const progressSize = document.getElementById('updateProgressSizeText');
    const notesBox = document.getElementById('aboutUpdateNotesBox');
    const checkBtn = document.getElementById('checkUpdatesBtn');
    const installBtn = document.getElementById('installUpdateBtn');
    const installIcon = document.getElementById('installUpdateIcon');
    const installLabel = document.getElementById('installUpdateLabel');
    const openRelBtn = document.getElementById('openReleasesBtn');

    if (cachedAppInfo) {
      metaEl.textContent = `${cachedAppInfo.appName} v${cachedAppInfo.version} | WebView2 ${cachedAppInfo.webview2Version} | DevTools Ready`;
      dirEl.textContent = cachedAppInfo.profilesDir || '-';
    }

    const stage = updateProgress?.stage;
    const isBusy = Boolean(lastUpdateCheck?.checking || stage === 'downloading' || stage === 'installing');
    checkBtn.disabled = isBusy;

    badgeEl.textContent = '';
    if (lastUpdateCheck?.available && lastUpdateCheck.latestVersion) {
      badgeEl.hidden = false;
      const curVer = lastUpdateCheck.currentVersion || cachedAppInfo?.version || '';
      if (curVer) {
        badgeEl.append(`v${curVer} `);
        const arrow = document.createElement('i');
        arrow.className = 'fa-solid fa-arrow-right';
        arrow.setAttribute('aria-hidden', 'true');
        badgeEl.append(arrow, ` v${lastUpdateCheck.latestVersion}`);
      } else {
        badgeEl.textContent = `v${lastUpdateCheck.latestVersion}`;
      }
    } else if (lastUpdateCheck?.currentVersion || cachedAppInfo?.version) {
      badgeEl.hidden = false;
      badgeEl.textContent = `v${lastUpdateCheck?.currentVersion || cachedAppInfo.version}`;
    } else {
      badgeEl.hidden = true;
    }

    if (lastUpdateCheck?.available && lastUpdateCheck.releaseNotes) {
      notesBox.hidden = false;
      notesBox.textContent = lastUpdateCheck.releaseNotes;
    } else {
      notesBox.hidden = true;
      notesBox.textContent = '';
    }

    progressWrap.hidden = true;
    progressFill.classList.remove('indeterminate');
    progressFill.style.width = '0%';
    progressTrack.setAttribute('aria-valuenow', '0');
    openRelBtn.hidden = true;

    if (stage === 'downloading') {
      statusIcon.className = 'fa-solid fa-download update-status-icon is-ready';
      statusText.textContent = t('updateDownloadingStatus');
      progressWrap.hidden = false;
      const pct = updateProgress.percent;
      if (typeof pct === 'number') {
        progressFill.style.width = `${pct}%`;
        progressTrack.setAttribute('aria-valuenow', String(pct));
      } else {
        progressFill.classList.add('indeterminate');
      }
      progressStage.textContent = typeof pct === 'number' ? `${pct}%` : t('updateDownloadingLabel');
      const dlText = formatUpdateBytes(updateProgress.downloadedBytes || 0);
      progressSize.textContent = updateProgress.totalBytes
        ? `${dlText} / ${formatUpdateBytes(updateProgress.totalBytes)}`
        : t('updateCalculatingSize');

      installBtn.hidden = false;
      installBtn.disabled = true;
      installIcon.className = 'fa-solid fa-spinner fa-spin';
      installLabel.textContent = t('downloadingBtn');
    } else if (stage === 'installing') {
      statusIcon.className = 'fa-solid fa-arrows-rotate fa-spin update-status-icon is-ready';
      statusText.textContent = t('updateInstallingStatus');
      progressWrap.hidden = false;
      progressFill.style.width = '100%';
      progressTrack.setAttribute('aria-valuenow', '100');
      progressStage.textContent = '100%';
      progressSize.textContent = t('installingBtn');

      installBtn.hidden = false;
      installBtn.disabled = true;
      installIcon.className = 'fa-solid fa-spinner fa-spin';
      installLabel.textContent = t('installingBtn');
    } else if (stage === 'error') {
      statusIcon.className = 'fa-solid fa-triangle-exclamation update-status-icon is-error';
      statusText.textContent = updateProgress.message || lastUpdateCheck?.error || 'Update error';
      openRelBtn.hidden = false;
      installBtn.hidden = !Boolean(lastUpdateCheck?.available);
      installBtn.disabled = false;
      installIcon.className = 'fa-solid fa-rotate-right';
      installLabel.textContent = t('retryUpdateBtn');
    } else if (!lastUpdateCheck) {
      statusIcon.className = 'fa-solid fa-circle-info update-status-icon';
      statusText.textContent = t('updateIdleStatus');
      installBtn.hidden = true;
    } else if (lastUpdateCheck.checking) {
      statusIcon.className = 'fa-solid fa-arrows-rotate fa-spin update-status-icon is-ready';
      statusText.textContent = t('updateCheckingStatus');
      progressWrap.hidden = false;
      progressFill.classList.add('indeterminate');
      progressStage.textContent = t('updateCheckingStatus');
      progressSize.textContent = '';
      installBtn.hidden = true;
    } else if (lastUpdateCheck.available) {
      statusIcon.className = 'fa-solid fa-circle-arrow-down update-status-icon is-ready';
      statusText.textContent = `${t('updateAvailableStatus')} (v${lastUpdateCheck.latestVersion})`;
      installBtn.hidden = false;
      installBtn.disabled = false;
      installIcon.className = 'fa-solid fa-download';
      installLabel.textContent = t('installUpdateBtn');
    } else if (lastUpdateCheck.error) {
      statusIcon.className = 'fa-solid fa-triangle-exclamation update-status-icon is-error';
      statusText.textContent = lastUpdateCheck.error;
      openRelBtn.hidden = false;
      installBtn.hidden = true;
    } else {
      statusIcon.className = 'fa-solid fa-circle-check update-status-icon is-ok';
      statusText.textContent = `${t('updateUpToDateStatus')} (v${lastUpdateCheck.currentVersion})`;
      installBtn.hidden = true;
    }
  }

  async function openAboutModal() {
    document.getElementById('profileModalBackdrop').hidden = true;
    const backdrop = document.getElementById('aboutModalBackdrop');
    document.body.classList.add('modal-open');
    backdrop.hidden = false;

    cachedAppInfo = await invoke('get_app_info');
    renderAboutInfo();
    await invoke('set_shell_overlay_open', { open: true });
    if (currentWebview && !document.hasFocus()) {
      await currentWebview.setFocus().catch(() => {});
    }
    if (!lastUpdateCheck && !updateProgress) {
      void checkForUpdatesNow();
    }
  }

  async function closeAboutModal() {
    document.getElementById('aboutModalBackdrop').hidden = true;
    document.body.classList.remove('modal-open');
    await invoke('set_shell_overlay_open', { open: false });
  }

  async function checkForUpdatesNow() {
    if (updateProgress?.stage === 'downloading' || updateProgress?.stage === 'installing') return;
    updateProgress = null;
    lastUpdateCheck = { checking: true };
    renderUpdateToolbarPill();
    renderAboutInfo();
    try {
      lastUpdateCheck = await invoke('check_for_updates');
    } catch (err) {
      lastUpdateCheck = { available: false, error: String(err) };
    } finally {
      renderUpdateToolbarPill();
      renderAboutInfo();
    }
  }

  async function installUpdateNow() {
    updateProgress = {
      stage: 'downloading',
      downloadedBytes: 0,
      totalBytes: null,
      percent: 0,
      message: null
    };
    renderUpdateToolbarPill();
    renderAboutInfo();
    try {
      await invoke('install_available_update');
    } catch (err) {
      updateProgress = {
        stage: 'error',
        downloadedBytes: updateProgress?.downloadedBytes || 0,
        totalBytes: updateProgress?.totalBytes || null,
        percent: null,
        message: String(err)
      };
      renderUpdateToolbarPill();
      renderAboutInfo();
    }
  }

  async function openReleasesPage() {
    const activeTab = currentState.tabs.find(tab => tab.id === currentState.activeTabId);
    const profileKey = activeTab ? (activeTab.isEphemeral ? 'ephemeral' : activeTab.profileId) : 'ephemeral';
    const isEphemeral = activeTab ? Boolean(activeTab.isEphemeral) : true;
    await closeAboutModal();
    await invoke('create_tab', { profileKey, isEphemeral, url: RELEASES_URL });
  }

  async function submitNewProfile(e) {
    e.preventDefault();
    const input = document.getElementById('newProfileNameInput');
    const err = document.getElementById('newProfileError');
    const raw = input.value.trim();

    let validationErr = '';
    if (!raw) {
      validationErr = t('errProfileEmpty');
    } else if (!/[a-zA-Z0-9]/.test(raw)) {
      validationErr = t('errProfileAlpha');
    } else if (raw.toLowerCase() === 'ephemeral') {
      validationErr = t('errProfileReserved');
    } else {
      const duplicate = (currentState.profiles || []).some(
        (existing) => (existing.name || '').toLowerCase() === raw.toLowerCase()
      );
      if (duplicate) {
        validationErr = t('errProfileDuplicate');
      }
    }

    if (validationErr) {
      err.textContent = validationErr;
      err.hidden = false;
      input.focus();
      return;
    }

    try {
      await invoke('create_tab_with_new_profile', { profileName: raw, color: selectedModalColor });
      document.getElementById('profileModalBackdrop').hidden = true;
      document.body.classList.remove('modal-open');
    } catch (backendErr) {
      err.textContent = String(backendErr);
      err.hidden = false;
      input.focus();
    }
  }

  async function cycleTheme() {
    const order = ['system', 'dark', 'light'];
    const cur = currentState.preferences?.theme || 'system';
    const next = order[(order.indexOf(cur) + 1) % order.length];
    await invoke('set_preferences', {
      preferences: { ...currentState.preferences, theme: next }
    });
  }

  async function toggleLanguage() {
    const cur = currentState.preferences?.language || 'id';
    const next = cur === 'id' ? 'en' : 'id';
    await invoke('set_preferences', {
      preferences: { ...currentState.preferences, language: next }
    });
  }

  async function handleShortcut(action) {
    if (action === 'new_tab') return openTabWithActiveRole();
    if (action === 'new_ephemeral_tab') return openTab('ephemeral', true);
    if (action === 'close_tab' && currentState.activeTabId) return closeTab(currentState.activeTabId);
    if (action === 'reload_tab') return invoke('reload_active_tab');
    if (action === 'toggle_devtools') return openDevtools();
    if (action === 'find_in_page') return openFindBar();
    if (action === 'zoom_in') return adjustZoom(0.1);
    if (action === 'zoom_out') return adjustZoom(-0.1);
    if (action === 'zoom_reset') return adjustZoom(null);
    if (action === 'focus_address_bar') {
      if (currentWebview && !document.hasFocus()) {
        await currentWebview.setFocus();
      }
      const inp = document.getElementById('urlInput');
      inp.focus();
      inp.select();
      return;
    }
    if ((action === 'next_tab' || action === 'prev_tab') && currentState.tabs.length > 1) {
      const idx = currentState.tabs.findIndex(tab => tab.id === currentState.activeTabId);
      const delta = action === 'next_tab' ? 1 : -1;
      const next = currentState.tabs[(idx + delta + currentState.tabs.length) % currentState.tabs.length];
      return switchTab(next.id);
    }
  }

  function bindToolbarEvents() {
    document.getElementById('profileMgrBtn').addEventListener('click', toggleProfilePanel);
    document.getElementById('downloadsBtn').addEventListener('click', toggleDownloadPanel);
    document.getElementById('monitorBtn').addEventListener('click', toggleMonitorPanel);
    document.getElementById('zoomPill').addEventListener('click', () => adjustZoom(null));
    document.getElementById('findInput').addEventListener('input', () => runFind(true));
    document.getElementById('findInput').addEventListener('keydown', (e) => {
      if (e.key === 'Enter') {
        e.preventDefault();
        runFind(!e.shiftKey);
      } else if (e.key === 'Escape') {
        e.preventDefault();
        closeFindBar();
      }
    });
    document.getElementById('findPrevBtn').addEventListener('click', () => runFind(false));
    document.getElementById('findNextBtn').addEventListener('click', () => runFind(true));
    document.getElementById('findCloseBtn').addEventListener('click', closeFindBar);
    document.getElementById('crashRecoverBtn').addEventListener('click', recoverActiveTab);
    document.getElementById('crashCloseTabBtn').addEventListener('click', () => {
      if (currentState.activeTabId) closeTab(currentState.activeTabId);
    });
    document.getElementById('awakeWarnBtn').addEventListener('click', sleepInactiveTabs);
    document.getElementById('updateReadyBtn').addEventListener('click', openAboutModal);
    document.getElementById('newTabBtn').addEventListener('click', openNewTabMenu);
    document.getElementById('overflowBtn').addEventListener('click', openOverflowMenu);
    document.getElementById('backBtn').addEventListener('click', goBack);
    document.getElementById('forwardBtn').addEventListener('click', goForward);
    document.getElementById('reloadBtn').addEventListener('click', reloadOrStop);
    document.getElementById('homeBtn').addEventListener('click', goHome);
    document.getElementById('goBtn').addEventListener('click', navigateUrl);
    document.getElementById('newEphemBtn').addEventListener('click', () => openTab('ephemeral', true));
    document.getElementById('themeBtn').addEventListener('click', cycleTheme);
    document.getElementById('langBtn').addEventListener('click', toggleLanguage);
    document.getElementById('menuBtn').addEventListener('click', openAppMenu);

    document.getElementById('newProfileForm').addEventListener('submit', submitNewProfile);
    document.getElementById('closeProfileModalBtn').addEventListener('click', closeNewProfileModal);
    document.getElementById('cancelProfileModalBtn').addEventListener('click', closeNewProfileModal);
    document.getElementById('profileModalBackdrop').addEventListener('click', (e) => {
      if (e.target.id === 'profileModalBackdrop') {
        closeNewProfileModal();
      }
    });
    document.getElementById('newProfileNameInput').addEventListener('input', () => {
      const err = document.getElementById('newProfileError');
      err.textContent = '';
      err.hidden = true;
    });

    document.getElementById('closeAboutModalBtn').addEventListener('click', closeAboutModal);
    document.getElementById('closeAboutFooterBtn').addEventListener('click', closeAboutModal);
    document.getElementById('openReleasesBtn').addEventListener('click', openReleasesPage);
    document.getElementById('checkUpdatesBtn').addEventListener('click', checkForUpdatesNow);
    document.getElementById('installUpdateBtn').addEventListener('click', installUpdateNow);
    document.getElementById('aboutModalBackdrop').addEventListener('click', (e) => {
      if (e.target.id === 'aboutModalBackdrop') {
        closeAboutModal();
      }
    });

    document.getElementById('tabs').addEventListener('wheel', (e) => {
      if (e.deltaY !== 0) {
        e.currentTarget.scrollLeft += e.deltaY;
        e.preventDefault();
      }
    }, { passive: false });

    document.getElementById('urlInput').addEventListener('keydown', (e) => {
      if (e.key === 'Enter') navigateUrl();
    });

    // Ensure child webview focus transitions cleanly when interacting with UI Shell on Windows.
    window.addEventListener('pointerdown', () => {
      if (currentWebview && !document.hasFocus()) {
        currentWebview.setFocus().catch(() => {});
      }
    });

    mediaDark.addEventListener('change', () => {
      if ((currentState.preferences?.theme || 'system') === 'system') applyTheme();
    });

    window.addEventListener('keydown', (e) => {
      if (e.key === 'Escape' && !document.getElementById('profileModalBackdrop').hidden) {
        e.preventDefault();
        closeNewProfileModal();
        return;
      }
      if (e.key === 'Escape' && !document.getElementById('aboutModalBackdrop').hidden) {
        e.preventDefault();
        closeAboutModal();
        return;
      }
      if (e.key === 'Escape' && currentState.findBarOpen) {
        e.preventDefault();
        closeFindBar();
        return;
      }
      if (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === 'n') {
        e.preventDefault(); handleShortcut('new_ephemeral_tab');
      } else if (e.ctrlKey && e.shiftKey && e.key === 'Tab') {
        e.preventDefault(); handleShortcut('prev_tab');
      } else if (e.ctrlKey && !e.shiftKey && e.key.toLowerCase() === 't') {
        e.preventDefault(); handleShortcut('new_tab');
      } else if (e.ctrlKey && !e.shiftKey && e.key.toLowerCase() === 'w') {
        e.preventDefault(); handleShortcut('close_tab');
      } else if (e.ctrlKey && !e.shiftKey && e.key === 'Tab') {
        e.preventDefault(); handleShortcut('next_tab');
      } else if (e.ctrlKey && !e.shiftKey && e.key.toLowerCase() === 'l') {
        e.preventDefault(); handleShortcut('focus_address_bar');
      } else if (e.ctrlKey && !e.shiftKey && e.key.toLowerCase() === 'f') {
        e.preventDefault(); handleShortcut('find_in_page');
      } else if (e.ctrlKey && (e.key === '=' || e.key === '+')) {
        e.preventDefault(); handleShortcut('zoom_in');
      } else if (e.ctrlKey && !e.shiftKey && e.key === '-') {
        e.preventDefault(); handleShortcut('zoom_out');
      } else if (e.ctrlKey && !e.shiftKey && e.key === '0') {
        e.preventDefault(); handleShortcut('zoom_reset');
      } else if ((e.ctrlKey && !e.shiftKey && e.key.toLowerCase() === 'r') || e.key === 'F5') {
        e.preventDefault(); handleShortcut('reload_tab');
      } else if (e.key === 'F12') {
        e.preventDefault(); handleShortcut('toggle_devtools');
      }
    });
  }

  (async () => {
    bindToolbarEvents();
    await listen('shell-state', (e) => {
      currentState = e.payload;
      render();
    });
    await listen('shell-shortcut', (e) => handleShortcut(e.payload.action));
    await listen('open-new-profile-modal', () => openNewProfileModal());
    await listen('open-about-modal', () => openAboutModal());
    await listen('update-available', (e) => {
      lastUpdateCheck = e.payload;
      render();
      if (!document.getElementById('aboutModalBackdrop').hidden) {
        renderAboutInfo();
      }
    });
    await listen('update-progress', (e) => {
      updateProgress = e.payload;
      renderUpdateToolbarPill();
      if (!document.getElementById('aboutModalBackdrop').hidden) {
        renderAboutInfo();
      }
    });

    currentState = await invoke('get_shell_state');
    render();
  })();
})();
