(() => {
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;

  let currentState = {
    preferences: { theme: 'system', language: 'id', sleepIdleMinutes: 15, uiDensity: 'comfortable', accentColor: null },
    colorPresets: [],
    tabs: [],
    activeTabId: null,
    activeAwakeCount: 0,
    totalMemoryMb: 0
  };
  const mediaDark = window.matchMedia('(prefers-color-scheme: dark)');

  function t(key) {
    const lang = currentState.preferences?.language || 'id';
    return (window.ROLETAB_I18N[lang] || window.ROLETAB_I18N.id)[key] || key;
  }

  function applyTheme() {
    const mode = currentState.preferences?.theme || 'system';
    const resolved = mode === 'system' ? (mediaDark.matches ? 'dark' : 'light') : mode;
    document.documentElement.setAttribute('data-theme', resolved);
    document.documentElement.setAttribute('data-density', currentState.preferences?.uiDensity || 'comfortable');
    document.documentElement.lang = currentState.preferences?.language || 'id';
    const accent = currentState.preferences?.accentColor;
    if (accent) {
      document.documentElement.style.setProperty('--accent-focus', accent);
      document.documentElement.style.setProperty('--primary-btn', accent);
    } else {
      document.documentElement.style.removeProperty('--accent-focus');
      document.documentElement.style.removeProperty('--primary-btn');
    }
  }

  function updateIdleOptions() {
    const sel = document.getElementById('idleSleepSelect');
    const lang = currentState.preferences?.language || 'id';
    const suffix = lang === 'en' ? 'min' : 'menit';
    sel.options[0].textContent = `5 ${suffix}`;
    sel.options[1].textContent = `15 ${suffix}`;
    sel.options[2].textContent = `30 ${suffix}`;
    sel.options[3].textContent = t('autoSleepOff');

    const curMins = currentState.preferences?.sleepIdleMinutes;
    sel.value = String(typeof curMins === 'number' ? curMins : 15);

    const denSel = document.getElementById('uiDensitySelect');
    if (denSel) {
      denSel.options[0].textContent = t('densityComfortable');
      denSel.options[1].textContent = t('densityCompact');
      denSel.value = currentState.preferences?.uiDensity || 'comfortable';
    }
  }

  function renderAccentSwatches() {
    const row = document.getElementById('accentSwatchRow');
    if (!row) return;
    row.innerHTML = '';
    const activeAccent = (currentState.preferences?.accentColor || '').toLowerCase();

    const defBtn = document.createElement('button');
    defBtn.type = 'button';
    defBtn.className = 'accent-dot-btn' + (!activeAccent ? ' selected' : '');
    defBtn.style.backgroundColor = '#1a73e8';
    defBtn.title = t('accentDefault');
    defBtn.setAttribute('aria-label', t('accentDefault'));
    if (!activeAccent) {
      defBtn.innerHTML = '<i class="fa-solid fa-check" aria-hidden="true"></i>';
    }
    defBtn.addEventListener('click', () => {
      invoke('set_preferences', { preferences: { ...currentState.preferences, accentColor: null } });
    });
    row.appendChild(defBtn);

    (currentState.colorPresets || []).slice(0, 6).forEach((hex) => {
      const btn = document.createElement('button');
      btn.type = 'button';
      const selected = activeAccent === hex.toLowerCase();
      btn.className = 'accent-dot-btn' + (selected ? ' selected' : '');
      btn.style.backgroundColor = hex;
      btn.title = hex;
      btn.setAttribute('aria-label', hex);
      if (selected) {
        btn.innerHTML = '<i class="fa-solid fa-check" aria-hidden="true"></i>';
      }
      btn.addEventListener('click', () => {
        invoke('set_preferences', { preferences: { ...currentState.preferences, accentColor: hex } });
      });
      row.appendChild(btn);
    });
  }

  function render() {
    applyTheme();
    document.getElementById('monPanelTitle').textContent = t('taskMonitorTitle');
    document.getElementById('idleSleepLabel').textContent = t('autoSleepIdleLabel');
    document.getElementById('uiDensityLabel').textContent = t('densityLabel');
    document.getElementById('accentThemeLabel').textContent = t('accentLabel');
    document.getElementById('sleepIdleLabel').textContent = t('sleepIdleBtn');
    renderAccentSwatches();

    const refBtn = document.getElementById('refreshMonBtn');
    refBtn.setAttribute('aria-label', t('reload'));
    refBtn.title = t('reload');

    const closeBtn = document.getElementById('closeMonPanelBtn');
    closeBtn.setAttribute('aria-label', t('closeModal'));
    closeBtn.title = t('closeModal');

    updateIdleOptions();

    const tabs = currentState.tabs || [];
    const totalMb = currentState.totalMemoryMb || 0;
    const awakeCount = typeof currentState.activeAwakeCount === 'number'
      ? currentState.activeAwakeCount
      : tabs.filter((tb) => !tb.isSleeping).length;
    document.getElementById('monTotalRamText').textContent =
      `RAM ${totalMb} MB | ${awakeCount}/${tabs.length} ${t('awakeWarnSuffix')}`;

    const list = document.getElementById('monitorList');
    list.innerHTML = '';

    tabs.forEach((tab) => {
      const card = document.createElement('article');
      card.className = 'mon-card';

      const top = document.createElement('div');
      top.className = 'mon-top';

      const badge = document.createElement('span');
      badge.className = 'mon-badge';
      badge.style.backgroundColor = tab.profileColor || '#1d4ed8';
      badge.textContent = tab.isEphemeral ? 'eph' : (tab.profileKey || 'RoleTab');

      const title = document.createElement('span');
      title.className = 'mon-title';
      title.textContent = tab.title && tab.title !== 'about:blank' ? tab.title : t('tabDefault');
      title.title = tab.url || '';

      top.append(badge, title);

      const metaRow = document.createElement('div');
      metaRow.className = 'mon-meta-row';

      const info = document.createElement('span');
      const pidPart = tab.browserPid ? `PID ${tab.browserPid}` : 'PID -';
      const memPart = typeof tab.memoryMb === 'number' ? `${tab.memoryMb} MB` : '- MB';
      let statusLabel = tab.id === currentState.activeTabId ? t('statusActive') : t('statusAwake');
      let statusCls = '';
      if (tab.crashKind) {
        statusLabel = t('statusCrashed');
        statusCls = 'crash';
      } else if (tab.isSleeping) {
        statusLabel = 'Sleep';
        statusCls = 'sleep';
      } else if (tab.isPlayingAudio) {
        statusLabel = 'Audio';
        statusCls = 'audio';
      } else if (tab.isPinned) {
        statusLabel = t('statusPinned');
        statusCls = 'pinned';
      }

      const textPart = document.createElement('span');
      textPart.textContent = `${pidPart} | ${memPart} | `;
      const tagPart = document.createElement('span');
      tagPart.className = `status-tag ${statusCls}`;
      tagPart.textContent = statusLabel;
      info.append(textPart, tagPart);

      const actions = document.createElement('div');
      actions.className = 'mon-actions';

      const pinBtn = document.createElement('button');
      pinBtn.type = 'button';
      pinBtn.className = 'btn btn-ghost' + (tab.isPinned ? ' active-pin' : '');
      pinBtn.title = tab.isPinned ? t('unpinTab') : t('pinTab');
      pinBtn.setAttribute('aria-label', pinBtn.title);
      pinBtn.innerHTML = `<i class="fa-solid fa-thumbtack" aria-hidden="true"></i>`;
      pinBtn.addEventListener('click', () => invoke('toggle_tab_pin', { tabId: tab.id }));
      actions.appendChild(pinBtn);

      if (tab.crashKind) {
        const recBtn = document.createElement('button');
        recBtn.type = 'button';
        recBtn.className = 'btn btn-ghost';
        recBtn.textContent = t('recoverTabBtn');
        recBtn.addEventListener('click', () => invoke('recover_tab', { tabId: tab.id }));
        actions.appendChild(recBtn);
      } else {
        const sleepBtn = document.createElement('button');
        sleepBtn.type = 'button';
        sleepBtn.className = 'btn btn-ghost';
        sleepBtn.title = t('sleepTabBtn');
        sleepBtn.setAttribute('aria-label', t('sleepTabBtn'));
        sleepBtn.disabled = tab.id === currentState.activeTabId || tab.isSleeping || tab.isPinned;
        sleepBtn.innerHTML = `<i class="fa-solid fa-moon" aria-hidden="true"></i>`;
        sleepBtn.addEventListener('click', () => invoke('sleep_tab', { tabId: tab.id }));
        actions.appendChild(sleepBtn);
      }

      metaRow.append(info, actions);
      card.append(top, metaRow);
      list.appendChild(card);
    });
  }

  document.getElementById('closeMonPanelBtn').addEventListener('click', () => {
    invoke('set_monitor_panel_open', { open: false });
  });

  document.getElementById('refreshMonBtn').addEventListener('click', () => {
    invoke('refresh_task_monitor');
  });

  document.getElementById('sleepIdleBtn').addEventListener('click', () => {
    invoke('sleep_inactive_tabs');
  });

  document.getElementById('idleSleepSelect').addEventListener('change', async (e) => {
    const mins = Number(e.target.value);
    await invoke('set_preferences', {
      preferences: { ...currentState.preferences, sleepIdleMinutes: mins }
    });
  });

  document.getElementById('uiDensitySelect').addEventListener('change', async (e) => {
    const uiDensity = e.target.value === 'compact' ? 'compact' : 'comfortable';
    await invoke('set_preferences', {
      preferences: { ...currentState.preferences, uiDensity }
    });
  });

  mediaDark.addEventListener('change', () => {
    if ((currentState.preferences?.theme || 'system') === 'system') applyTheme();
  });

  window.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      invoke('set_monitor_panel_open', { open: false });
    }
  });

  (async () => {
    await listen('shell-state', (e) => {
      currentState = e.payload;
      render();
    });
    await invoke('refresh_task_monitor');
    currentState = await invoke('get_shell_state');
    render();
  })();
})();
