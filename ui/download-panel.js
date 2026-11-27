(() => {
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;

  let currentState = {
    preferences: { theme: 'system', language: 'id', groupDownloadsByProfile: true },
    downloads: []
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
    document.documentElement.lang = currentState.preferences?.language || 'id';
  }

  function statusMeta(status) {
    if (status === 'inProgress') {
      return { icon: 'fa-spinner fa-spin', label: t('dlInProgress'), cls: 'inProgress' };
    }
    if (status === 'completed') {
      return { icon: 'fa-circle-check', label: t('dlCompleted'), cls: 'completed' };
    }
    return { icon: 'fa-triangle-exclamation', label: t('dlFailed'), cls: 'failed' };
  }

  function formatTime(epochSecs) {
    if (!epochSecs) return '';
    try {
      const dt = new Date(epochSecs * 1000);
      return dt.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    } catch {
      return '';
    }
  }

  function render() {
    applyTheme();
    document.getElementById('dlPanelTitle').textContent = t('downloadsTitle');
    document.getElementById('clearDownloadsLabel').textContent = t('clearDownloadsBtn');
    document.getElementById('groupByProfileLabel').textContent = t('groupByProfileLabel');
    document.getElementById('dlEmptyTitle').textContent = t('emptyDownloadsTitle');
    document.getElementById('dlEmptyDesc').textContent = t('emptyDownloadsDesc');

    const closeBtn = document.getElementById('closeDlPanelBtn');
    closeBtn.setAttribute('aria-label', t('closeModal'));
    closeBtn.title = t('closeModal');

    const toggle = document.getElementById('groupByProfileToggle');
    toggle.checked = currentState.preferences?.groupDownloadsByProfile !== false;

    const listData = currentState.downloads || [];
    const clearBtn = document.getElementById('clearDownloadsBtn');
    clearBtn.disabled = !listData.some((d) => d.status !== 'inProgress');

    document.getElementById('dlCountText').textContent = `${listData.length} ${t('dlItemsSuffix')}`;

    const empty = document.getElementById('dlEmptyState');
    const container = document.getElementById('downloadsList');
    container.innerHTML = '';
    empty.hidden = listData.length > 0;

    listData.forEach((item) => {
      const card = document.createElement('article');
      card.className = 'dl-card';

      const top = document.createElement('div');
      top.className = 'dl-top';

      const badge = document.createElement('span');
      badge.className = 'dl-badge';
      badge.style.backgroundColor = item.profileColor || '#1d4ed8';
      badge.textContent = item.profileName || 'RoleTab';

      const name = document.createElement('span');
      name.className = 'dl-name';
      name.textContent = item.fileName || 'download';
      name.title = item.path || item.url || '';

      top.append(badge, name);

      const meta = document.createElement('div');
      meta.className = 'dl-meta';

      const st = statusMeta(item.status);
      const statusWrap = document.createElement('span');
      statusWrap.className = `dl-status ${st.cls}`;
      const timeStr = formatTime(item.finishedAt || item.startedAt);
      statusWrap.innerHTML = `<i class="fa-solid ${st.icon}" aria-hidden="true"></i>`;
      const statusTxt = document.createElement('span');
      statusTxt.textContent = timeStr ? `${st.label} | ${timeStr}` : st.label;
      statusWrap.appendChild(statusTxt);

      const revealBtn = document.createElement('button');
      revealBtn.type = 'button';
      revealBtn.className = 'btn btn-ghost';
      revealBtn.innerHTML = `<i class="fa-solid fa-folder-open" aria-hidden="true"></i><span>${t('showInFolderBtn')}</span>`;
      revealBtn.addEventListener('click', async () => {
        try {
          await invoke('show_download_in_folder', { downloadId: item.id });
        } catch (err) {
          console.error(err);
        }
      });

      meta.append(statusWrap, revealBtn);

      const pathLine = document.createElement('div');
      pathLine.className = 'dl-path';
      pathLine.textContent = item.path || '';
      pathLine.title = item.path || '';

      card.append(top, meta, pathLine);
      container.appendChild(card);
    });
  }

  document.getElementById('closeDlPanelBtn').addEventListener('click', async () => {
    await invoke('set_download_panel_open', { open: false });
  });

  document.getElementById('clearDownloadsBtn').addEventListener('click', async () => {
    await invoke('clear_download_history');
  });

  document.getElementById('groupByProfileToggle').addEventListener('change', async (e) => {
    await invoke('set_preferences', {
      preferences: {
        ...currentState.preferences,
        groupDownloadsByProfile: Boolean(e.target.checked)
      }
    });
  });

  mediaDark.addEventListener('change', () => {
    if ((currentState.preferences?.theme || 'system') === 'system') applyTheme();
  });

  window.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      invoke('set_download_panel_open', { open: false });
    }
  });

  (async () => {
    await listen('shell-state', (e) => {
      currentState = e.payload;
      render();
    });
    currentState = await invoke('get_shell_state');
    render();
  })();
})();
