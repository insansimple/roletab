(() => {
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;

  let currentState = {
    profilesDir: '',
    preferences: { theme: 'system', language: 'id', defaultProfile: null },
    profiles: [],
    workspaces: [],
    colorPresets: ['#b91c1c', '#1d4ed8', '#0f766e', '#6d28d9', '#b45309', '#047857', '#be185d', '#334155']
  };
  let activeView = 'profiles';
  let lastSummaries = [];
  let pendingDeleteId = null;
  let pendingClearId = null;
  let editingProfileId = null;
  let editingColor = '#1d4ed8';
  let editingProxyMode = 'direct';
  let editingProxyAddress = '';
  let createSelectedColor = '#1d4ed8';
  let openMenuProfId = null;
  let noticeTimer = null;
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

  function applyTexts() {
    document.getElementById('panelTitle').textContent = t('profileManager');
    document.getElementById('importProfileLabel').textContent = t('importBtn');
    document.getElementById('toggleCreateLabel').textContent = t('createProfileAction');
    document.getElementById('tabProfilesViewBtn').textContent = t('profilesTab');
    document.getElementById('tabWorkspacesViewBtn').textContent = t('workspacesTab');
    document.getElementById('sideCreateLabel').textContent = t('newProfileLabel');
    document.getElementById('sideCreateColorLabel').textContent = t('colorLabel');
    document.getElementById('sideProfileInput').placeholder = t('newProfilePlaceholder');
    document.getElementById('sideCreateCancelBtn').textContent = t('cancel');
    document.getElementById('sideCreateSubmitBtn').textContent = t('createProfileBtn');
    document.getElementById('wsNameInput').placeholder = t('workspaceNamePlaceholder');
    document.getElementById('wsSaveSubmitBtn').textContent = t('saveWorkspaceBtn');

    const closeBtn = document.getElementById('closePanelBtn');
    closeBtn.setAttribute('aria-label', t('closeModal'));
    closeBtn.title = t('closeModal');
  }

  function showNotice(msg) {
    const el = document.getElementById('panelNotice');
    el.textContent = msg;
    el.hidden = false;
    if (noticeTimer) clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => {
      el.hidden = true;
    }, 3200);
  }

  function shortCode(name) {
    const raw = (name || '').trim();
    const lower = raw.toLowerCase();
    if (lower === 'admin') return 'adm';
    if (lower === 'user') return 'usr';
    if (lower.length <= 3) return lower;
    const alnum = lower.replace(/[^a-z0-9]/g, '');
    return (alnum || lower).slice(0, 3);
  }

  function formatBytes(bytes) {
    if (!bytes || bytes <= 0) return '0 B';
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
  }

  function renderColorSwatches(container, activeHex, onPick) {
    container.innerHTML = '';
    const presets = currentState.colorPresets || [];
    presets.forEach((hex) => {
      const btn = document.createElement('button');
      btn.type = 'button';
      const isSelected = activeHex.toLowerCase() === hex.toLowerCase();
      btn.className = 'swatch-btn' + (isSelected ? ' selected' : '');
      btn.style.backgroundColor = hex;
      btn.setAttribute('aria-label', hex);
      if (isSelected) {
        btn.innerHTML = '<i class="fa-solid fa-check" aria-hidden="true"></i>';
      }
      btn.addEventListener('click', () => onPick(hex));
      container.appendChild(btn);
    });
  }

  function closeActionMenu() {
    openMenuProfId = null;
    const menu = document.getElementById('profileActionMenu');
    if (menu) {
      menu.hidden = true;
      menu.innerHTML = '';
    }
  }

  function openActionMenu(prof, anchorBtn) {
    const menu = document.getElementById('profileActionMenu');
    if (openMenuProfId === prof.id && !menu.hidden) {
      closeActionMenu();
      return;
    }

    openMenuProfId = prof.id;
    menu.innerHTML = '';

    const addItem = (iconCls, label, onClick, opts = {}) => {
      const btn = document.createElement('button');
      btn.type = 'button';
      btn.className = 'action-menu-item' + (opts.danger ? ' danger' : '');
      btn.disabled = Boolean(opts.disabled);
      btn.innerHTML = `<i class="fa-solid ${iconCls}" aria-hidden="true"></i><span>${label}</span>`;
      btn.addEventListener('click', (ev) => {
        ev.stopPropagation();
        closeActionMenu();
        onClick();
      });
      menu.appendChild(btn);
    };

    addItem('fa-pen', t('editBtn'), () => {
      pendingDeleteId = null;
      pendingClearId = null;
      editingProfileId = editingProfileId === prof.id ? null : prof.id;
      editingColor = prof.color || '#1d4ed8';
      editingProxyMode = prof.proxyMode || 'direct';
      editingProxyAddress = prof.proxyAddress || '';
      renderSummaries(lastSummaries);
    });

    addItem('fa-file-export', t('exportBtn'), async () => {
      try {
        await invoke('export_profile_zip', { profileId: prof.id });
        showNotice(t('exportedNotice'));
        await refresh();
      } catch (err) {
        showNotice(String(err));
      }
    });

    addItem(
      'fa-star',
      t('setDefaultBtn'),
      async () => {
        await invoke('set_default_profile', { profileKey: prof.id });
        await refresh();
      },
      { disabled: prof.isDefault }
    );

    addItem('fa-broom', t('clearBtn'), () => {
      editingProfileId = null;
      pendingDeleteId = null;
      pendingClearId = pendingClearId === prof.id ? null : prof.id;
      renderSummaries(lastSummaries);
    });

    const sep = document.createElement('div');
    sep.className = 'action-menu-sep';
    menu.appendChild(sep);

    addItem(
      'fa-trash',
      t('deleteBtn'),
      () => {
        editingProfileId = null;
        pendingClearId = null;
        pendingDeleteId = pendingDeleteId === prof.id ? null : prof.id;
        renderSummaries(lastSummaries);
      },
      { danger: true }
    );

    menu.hidden = false;
    const rect = anchorBtn.getBoundingClientRect();
    const menuW = menu.offsetWidth || 160;
    const menuH = menu.offsetHeight || 165;
    const left = Math.max(8, Math.min(rect.right - menuW, window.innerWidth - menuW - 8));
    const top =
      rect.bottom + 4 + menuH > window.innerHeight - 8
        ? Math.max(8, rect.top - menuH - 4)
        : rect.bottom + 4;
    menu.style.left = `${left}px`;
    menu.style.top = `${top}px`;
  }

  function renderSummaries(summaries) {
    closeActionMenu();
    lastSummaries = summaries || [];
    const dirLabel = document.getElementById('profilesDirLabel');
    dirLabel.textContent = currentState.profilesDir || '';
    dirLabel.title = currentState.profilesDir || '';

    const list = document.getElementById('profileList');
    list.innerHTML = '';

    if (lastSummaries.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'empty-state';

      const icon = document.createElement('i');
      icon.className = 'fa-solid fa-users';
      icon.setAttribute('aria-hidden', 'true');

      const title = document.createElement('h2');
      title.textContent = t('emptyProfilesTitle');

      const desc = document.createElement('p');
      desc.textContent = t('emptyProfilesDesc');

      const btn = document.createElement('button');
      btn.type = 'button';
      btn.className = 'btn btn-primary';
      btn.innerHTML = `<i class="fa-solid fa-plus" aria-hidden="true"></i> ${t('createProfileAction')}`;
      btn.addEventListener('click', showCreateForm);

      empty.append(icon, title, desc, btn);
      list.appendChild(empty);
      return;
    }

    lastSummaries.forEach((prof) => {
      const item = document.createElement('div');
      item.className = 'profile-item';

      const top = document.createElement('div');
      top.className = 'profile-top';

      const left = document.createElement('div');
      left.className = 'profile-left';

      const badge = document.createElement('span');
      badge.className = 'role-badge';
      badge.style.backgroundColor = prof.color || '#1d4ed8';
      badge.textContent = shortCode(prof.name);

      const info = document.createElement('div');
      info.className = 'profile-info';

      const nameRow = document.createElement('div');
      nameRow.className = 'profile-name-row';

      const nameEl = document.createElement('span');
      nameEl.className = 'profile-name';
      nameEl.textContent = prof.name;
      nameEl.title = prof.name;
      nameRow.appendChild(nameEl);

      if (prof.isDefault) {
        const defTag = document.createElement('span');
        defTag.className = 'default-tag';
        defTag.textContent = t('defaultTag');
        nameRow.appendChild(defTag);
      }

      const meta = document.createElement('span');
      meta.className = 'profile-meta';
      const proxySuffix =
        prof.proxyMode && prof.proxyMode !== 'direct' && prof.proxyAddress
          ? ` | ${prof.proxyMode.toUpperCase()} ${prof.proxyAddress}`
          : '';
      meta.textContent = `${prof.openTabCount} ${t('openTabCountSuffix')} | ${formatBytes(prof.sizeBytes)}${proxySuffix}`;
      meta.title = prof.path;

      info.append(nameRow, meta);
      left.append(badge, info);

      const actions = document.createElement('div');
      actions.className = 'profile-actions';

      const openBtn = document.createElement('button');
      openBtn.type = 'button';
      openBtn.className = 'btn btn-primary';
      openBtn.innerHTML = `<i class="fa-solid fa-arrow-up-right-from-square" aria-hidden="true"></i> ${t('openTabBtn')}`;
      openBtn.addEventListener('click', async () => {
        await invoke('create_tab', { profileKey: prof.id, isEphemeral: false, url: null });
      });

      const moreBtn = document.createElement('button');
      moreBtn.type = 'button';
      moreBtn.className = 'icon-btn';
      moreBtn.setAttribute('aria-label', t('profileActions'));
      moreBtn.title = t('profileActions');
      moreBtn.innerHTML = '<i class="fa-solid fa-ellipsis-vertical" aria-hidden="true"></i>';
      moreBtn.addEventListener('click', (ev) => {
        ev.stopPropagation();
        openActionMenu(prof, moreBtn);
      });

      actions.append(openBtn, moreBtn);
      top.append(left, actions);
      item.append(top);

      if (editingProfileId === prof.id) {
        const editForm = document.createElement('form');
        editForm.className = 'edit-box';
        editForm.noValidate = true;

        const nameInput = document.createElement('input');
        nameInput.className = 'create-input';
        nameInput.type = 'text';
        nameInput.maxLength = 40;
        nameInput.value = prof.name;

        const swatchRow = document.createElement('div');
        swatchRow.className = 'swatch-row';
        const repaintEditSwatches = () => {
          renderColorSwatches(swatchRow, editingColor, (hex) => {
            editingColor = hex;
            repaintEditSwatches();
          });
        };
        repaintEditSwatches();

        const errEl = document.createElement('p');
        errEl.className = 'create-error';
        errEl.hidden = true;

        const proxyLabel = document.createElement('label');
        proxyLabel.className = 'create-label';
        proxyLabel.textContent = t('proxyLabel');

        const proxyRow = document.createElement('div');
        proxyRow.className = 'proxy-row';

        const proxySelect = document.createElement('select');
        proxySelect.className = 'proxy-select';
        [
          { val: 'direct', txt: t('proxyDirect') },
          { val: 'http', txt: t('proxyHttp') },
          { val: 'socks5', txt: t('proxySocks5') }
        ].forEach((opt) => {
          const o = document.createElement('option');
          o.value = opt.val;
          o.textContent = opt.txt;
          proxySelect.appendChild(o);
        });
        proxySelect.value = editingProxyMode;

        const proxyInput = document.createElement('input');
        proxyInput.className = 'create-input';
        proxyInput.type = 'text';
        proxyInput.placeholder = t('proxyPlaceholder');
        proxyInput.value = editingProxyAddress;
        proxyInput.disabled = editingProxyMode === 'direct';

        proxySelect.addEventListener('change', () => {
          editingProxyMode = proxySelect.value;
          proxyInput.disabled = editingProxyMode === 'direct';
        });
        proxyRow.append(proxySelect, proxyInput);

        const formActions = document.createElement('div');
        formActions.className = 'create-actions';

        const cancelEditBtn = document.createElement('button');
        cancelEditBtn.type = 'button';
        cancelEditBtn.className = 'btn btn-ghost';
        cancelEditBtn.textContent = t('cancel');
        cancelEditBtn.addEventListener('click', () => {
          editingProfileId = null;
          renderSummaries(lastSummaries);
        });

        const saveEditBtn = document.createElement('button');
        saveEditBtn.type = 'submit';
        saveEditBtn.className = 'btn btn-primary';
        saveEditBtn.textContent = t('saveBtn');

        editForm.addEventListener('submit', async (ev) => {
          ev.preventDefault();
          const nextName = nameInput.value.trim();
          const err = validateProfileNameClient(nextName, prof.id);
          if (err) {
            errEl.textContent = err;
            errEl.hidden = false;
            nameInput.focus();
            return;
          }
          try {
            await invoke('update_profile', {
              profileId: prof.id,
              name: nextName,
              color: editingColor,
              proxyMode: proxySelect.value,
              proxyAddress: proxySelect.value === 'direct' ? null : proxyInput.value.trim()
            });
            editingProfileId = null;
            await refresh();
          } catch (backendErr) {
            errEl.textContent = String(backendErr);
            errEl.hidden = false;
          }
        });

        formActions.append(cancelEditBtn, saveEditBtn);
        editForm.append(nameInput, swatchRow, proxyLabel, proxyRow, errEl, formActions);
        item.appendChild(editForm);
      }

      if (pendingClearId === prof.id) {
        const confirmRow = document.createElement('div');
        confirmRow.className = 'confirm-row neutral';

        const promptText = document.createElement('span');
        promptText.textContent =
          prof.openTabCount > 0 ? t('confirmClearPrompt') : t('confirmClearClosedPrompt');

        const confirmBtns = document.createElement('div');
        confirmBtns.className = 'confirm-btns';

        const cancelBtn = document.createElement('button');
        cancelBtn.type = 'button';
        cancelBtn.className = 'btn btn-ghost';
        cancelBtn.textContent = t('cancel');
        cancelBtn.addEventListener('click', () => {
          pendingClearId = null;
          renderSummaries(lastSummaries);
        });

        const confirmClrBtn = document.createElement('button');
        confirmClrBtn.type = 'button';
        confirmClrBtn.className = 'btn btn-primary';
        confirmClrBtn.textContent = t('clearBtn');
        confirmClrBtn.addEventListener('click', async () => {
          pendingClearId = null;
          await invoke('clear_profile_data', {
            profileId: prof.id,
            reloadTabs: prof.openTabCount > 0
          });
          showNotice(t('clearedNotice'));
          await refresh();
        });

        confirmBtns.append(cancelBtn, confirmClrBtn);
        confirmRow.append(promptText, confirmBtns);
        item.appendChild(confirmRow);
      }

      if (pendingDeleteId === prof.id) {
        const confirmRow = document.createElement('div');
        confirmRow.className = 'confirm-row';

        const promptText = document.createElement('span');
        promptText.textContent = t('confirmDeletePrompt');

        const confirmBtns = document.createElement('div');
        confirmBtns.className = 'confirm-btns';

        const cancelBtn = document.createElement('button');
        cancelBtn.type = 'button';
        cancelBtn.className = 'btn btn-ghost';
        cancelBtn.textContent = t('cancel');
        cancelBtn.addEventListener('click', () => {
          pendingDeleteId = null;
          renderSummaries(lastSummaries);
        });

        const confirmDelBtn = document.createElement('button');
        confirmDelBtn.type = 'button';
        confirmDelBtn.className = 'btn btn-danger';
        confirmDelBtn.textContent = t('deleteBtn');
        confirmDelBtn.addEventListener('click', async () => {
          pendingDeleteId = null;
          await invoke('delete_profile_dir', { profileKey: prof.id });
          await refresh();
        });

        confirmBtns.append(cancelBtn, confirmDelBtn);
        confirmRow.append(promptText, confirmBtns);
        item.appendChild(confirmRow);
      }

      list.appendChild(item);
    });
  }

  function validateProfileNameClient(raw, excludeId) {
    if (!raw) return t('errProfileEmpty');
    if (!/[a-zA-Z0-9]/.test(raw)) return t('errProfileAlpha');
    if (raw.toLowerCase() === 'ephemeral') return t('errProfileReserved');
    const duplicate = (currentState.profiles || []).some(
      (existing) =>
        existing.id !== excludeId &&
        (existing.name || '').toLowerCase() === raw.toLowerCase()
    );
    if (duplicate) return t('errProfileDuplicate');
    return '';
  }

  function paintCreateSwatches() {
    const container = document.getElementById('sideCreateColorRow');
    renderColorSwatches(container, createSelectedColor, (hex) => {
      createSelectedColor = hex;
      paintCreateSwatches();
    });
  }

  function showCreateForm() {
    const form = document.getElementById('sideCreateForm');
    const input = document.getElementById('sideProfileInput');
    const err = document.getElementById('sideCreateError');
    const presets = currentState.colorPresets || [];
    if (presets.length > 0) {
      createSelectedColor = presets[(currentState.profiles || []).length % presets.length];
    }
    paintCreateSwatches();
    err.textContent = '';
    err.hidden = true;
    input.value = '';
    form.hidden = false;
    input.focus();
  }

  function hideCreateForm() {
    document.getElementById('sideCreateForm').hidden = true;
  }

  async function submitCreateForm(e) {
    e.preventDefault();
    const input = document.getElementById('sideProfileInput');
    const err = document.getElementById('sideCreateError');
    const raw = input.value.trim();

    const validationErr = validateProfileNameClient(raw, null);
    if (validationErr) {
      err.textContent = validationErr;
      err.hidden = false;
      input.focus();
      return;
    }

    try {
      await invoke('create_tab_with_new_profile', {
        profileName: raw,
        color: createSelectedColor
      });
      hideCreateForm();
      await refresh();
    } catch (backendErr) {
      err.textContent = String(backendErr);
      err.hidden = false;
      input.focus();
    }
  }

  function renderWorkspaces() {
    const list = document.getElementById('workspaceList');
    list.innerHTML = '';
    const workspaces = currentState.workspaces || [];

    if (workspaces.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'empty-state';
      empty.innerHTML = `<i class="fa-solid fa-layer-group" aria-hidden="true"></i><h2>${t('emptyWorkspacesTitle')}</h2><p>${t('emptyWorkspacesDesc')}</p>`;
      list.appendChild(empty);
      return;
    }

    workspaces.forEach((ws) => {
      const item = document.createElement('div');
      item.className = 'profile-item';

      const top = document.createElement('div');
      top.className = 'profile-top';

      const info = document.createElement('div');
      info.className = 'profile-info';

      const nameEl = document.createElement('span');
      nameEl.className = 'profile-name';
      nameEl.textContent = ws.name;

      const meta = document.createElement('span');
      meta.className = 'profile-meta';
      meta.textContent = `${ws.tabs.length} ${t('workspaceTabsSuffix')}`;

      info.append(nameEl, meta);

      const actions = document.createElement('div');
      actions.className = 'profile-actions';

      const launchBtn = document.createElement('button');
      launchBtn.type = 'button';
      launchBtn.className = 'btn btn-primary';
      launchBtn.innerHTML = `<i class="fa-solid fa-play" aria-hidden="true"></i> ${t('launchWorkspaceBtn')}`;
      launchBtn.addEventListener('click', () => invoke('launch_workspace', { workspaceId: ws.id }));

      const delBtn = document.createElement('button');
      delBtn.type = 'button';
      delBtn.className = 'icon-btn';
      delBtn.title = t('deleteBtn');
      delBtn.setAttribute('aria-label', t('deleteBtn'));
      delBtn.innerHTML = '<i class="fa-solid fa-trash" aria-hidden="true"></i>';
      delBtn.addEventListener('click', () => invoke('delete_workspace', { workspaceId: ws.id }));

      actions.append(launchBtn, delBtn);
      top.append(info, actions);

      const badges = document.createElement('div');
      badges.className = 'ws-badges';
      ws.tabs.forEach((spec) => {
        const b = document.createElement('span');
        b.className = 'role-badge';
        b.style.backgroundColor = spec.profileColor || '#4b5563';
        b.textContent = spec.isEphemeral ? 'eph' : shortCode(spec.profileName);
        b.title = `${spec.profileName} | ${spec.url}`;
        badges.appendChild(b);
      });

      item.append(top, badges);
      list.appendChild(item);
    });
  }

  function syncViewTabs() {
    const isProf = activeView === 'profiles';
    document.getElementById('tabProfilesViewBtn').classList.toggle('active', isProf);
    document.getElementById('tabWorkspacesViewBtn').classList.toggle('active', !isProf);
    document.getElementById('profileList').hidden = !isProf;
    document.getElementById('workspaceList').hidden = isProf;
    document.getElementById('wsSaveForm').hidden = isProf;
    document.getElementById('toggleCreateBtn').hidden = !isProf;
    document.getElementById('importProfileBtn').hidden = !isProf;
    if (!isProf) {
      hideCreateForm();
    }
  }

  async function refresh() {
    applyTheme();
    applyTexts();
    syncViewTabs();
    const summaries = await invoke('get_profile_summaries');
    renderSummaries(summaries);
    renderWorkspaces();
  }

  document.getElementById('closePanelBtn').addEventListener('click', () => {
    invoke('set_profile_panel_open', { open: false });
  });
  document.getElementById('tabProfilesViewBtn').addEventListener('click', () => {
    activeView = 'profiles';
    syncViewTabs();
  });
  document.getElementById('tabWorkspacesViewBtn').addEventListener('click', () => {
    activeView = 'workspaces';
    syncViewTabs();
    renderWorkspaces();
  });
  document.getElementById('importProfileBtn').addEventListener('click', async () => {
    try {
      const res = await invoke('import_profile_zip', { sourcePath: null });
      if (res) {
        showNotice(t('importedNotice'));
        await refresh();
      }
    } catch (err) {
      showNotice(String(err));
    }
  });
  document.getElementById('wsSaveForm').addEventListener('submit', async (e) => {
    e.preventDefault();
    const inp = document.getElementById('wsNameInput');
    const name = inp.value.trim();
    if (!name) {
      inp.focus();
      return;
    }
    try {
      await invoke('save_current_workspace', { name });
      inp.value = '';
    } catch (err) {
      showNotice(String(err));
    }
  });
  document.getElementById('toggleCreateBtn').addEventListener('click', () => {
    const form = document.getElementById('sideCreateForm');
    if (form.hidden) showCreateForm();
    else hideCreateForm();
  });
  document.getElementById('sideCreateCancelBtn').addEventListener('click', hideCreateForm);
  document.getElementById('sideCreateForm').addEventListener('submit', submitCreateForm);
  document.getElementById('sideProfileInput').addEventListener('input', () => {
    const err = document.getElementById('sideCreateError');
    err.textContent = '';
    err.hidden = true;
  });

  window.addEventListener('click', closeActionMenu);
  window.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      closeActionMenu();
    }
  });
  document.getElementById('profileList').addEventListener('scroll', closeActionMenu);

  mediaDark.addEventListener('change', () => {
    if ((currentState.preferences?.theme || 'system') === 'system') applyTheme();
  });

  (async () => {
    await listen('shell-state', async (e) => {
      currentState = e.payload;
      await refresh();
    });
    currentState = await invoke('get_shell_state');
    await refresh();
  })();
})();
