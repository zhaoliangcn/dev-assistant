// ============================================================
// Dev-Assistant Web UI — 全局 Alpine Store 初始化
// ============================================================
// 依赖：Alpine.js 3.x
// 功能：连接状态、模型列表/切换、Token 用量、主题管理

document.addEventListener('alpine:init', () => {
    if (!window.Alpine) return;

    // ── 连接状态 Store ──
    window.Alpine.store('connection', {
        connected: false,
    });

    // ── 模型列表与切换 Store ──
    window.Alpine.store('models', {
        list: [],
        active: '',
        loaded: false,
        // P6: 模型设置面板状态（配置模型 URL / 提供商 / 密钥等）
        configPath: '',
        panelOpen: false,
        editorOpen: false,
        saving: false,
        saveError: null,
        // 编辑中的原名称（新增为 null）；名称在编辑时不可改，避免歧义
        editingName: null,
        // 支持的 provider 类型（与后端 create_provider 对齐）
        providers: ['openai', 'openai-compatible', 'deepseek', 'moonshot', 'zhipu', 'baidu', 'aliyun', 'siliconflow', 'anthropic', 'ollama'],
        // 表单模型
        form: {
            name: '', provider: 'openai', api_url: '', api_key: '',
            model: '', temperature: 0.2, max_output_tokens: '', clear_api_key: false, hasKey: false,
        },

        async load() {
            try {
                const resp = await fetch('/api/models');
                const data = await resp.json();
                const models = Array.isArray(data) ? data : (data.models || []);
                this.list = models;
                this.configPath = data.config_path || '';
                const active = this.list.find((m) => m.active);
                this.active = active ? active.name : (this.list[0] ? this.list[0].name : '');
                this.loaded = true;
            } catch (e) {
                console.error('加载模型列表失败:', e);
            }
        },

        async switchModel(name) {
            if (!name || name === this.active) return;
            try {
                const resp = await fetch('/api/models/switch', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ name: name }),
                });
                const data = await resp.json();
                if (data.success) {
                    this.active = name;
                    this.list.forEach((m) => { m.active = m.name === name; });
                } else {
                    console.error('切换模型失败:', data.error || '未知错误');
                }
            } catch (e) {
                console.error('切换模型失败:', e);
            }
        },

        // ── 设置面板（P6） ──

        openPanel() {
            this.panelOpen = true;
            this.closeEditor();
        },

        closePanel() {
            this.panelOpen = false;
            this.closeEditor();
        },

        newModel() {
            this.editingName = null;
            this.saveError = null;
            this.form = {
                name: '', provider: 'openai', api_url: '', api_key: '',
                model: '', temperature: 0.2, max_output_tokens: '', clear_api_key: false, hasKey: false,
            };
            this.editorOpen = true;
            this._scrollEditorIntoView();
        },

        editModel(m) {
            this.editingName = m.name;
            this.saveError = null;
            this.form = {
                name: m.name,
                provider: m.provider,
                api_url: m.api_url || '',
                api_key: '', // 不回显完整密钥；留空表示保持不变
                model: m.model || '',
                temperature: m.temperature ?? 0.2,
                max_output_tokens: m.max_output_tokens ?? '',
                clear_api_key: false,
                hasKey: m.has_api_key,
            };
            this.editorOpen = true;
            this._scrollEditorIntoView();
        },

        // 打开编辑/新增表单时，自动把面板滚动到编辑区（表单位于列表下方）。
        // 需等 Alpine 渲染完（x-show 生效、元素有布局尺寸）后再滚动，否则 scrollHeight 为 0。
        _scrollEditorIntoView() {
            window.Alpine.nextTick(() => {
                const body = document.querySelector('#model-settings-panel .model-settings-body');
                if (body) body.scrollTop = body.scrollHeight;
            });
        },

        closeEditor() {
            this.editorOpen = false;
            this.editingName = null;
            this.saveError = null;
            this.saving = false;
        },

        async saveForm() {
            const i18n = window.Alpine.store('i18n');
            const f = this.form;
            if (!f.name || !f.provider || !f.api_url || !f.model) {
                this.saveError = i18n.form_required;
                return;
            }
            this.saving = true;
            this.saveError = null;
            const num = (v) => (v === '' || v === null || v === undefined) ? null : Number(v);
            const body = {
                name: f.name.trim(),
                provider: f.provider.trim(),
                api_url: f.api_url.trim(),
                api_key: f.api_key ? f.api_key.trim() : '',
                clear_api_key: !!f.clear_api_key,
                model: f.model.trim(),
                temperature: num(f.temperature),
                max_output_tokens: num(f.max_output_tokens),
            };
            try {
                const resp = await fetch('/api/models', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify(body),
                });
                const data = await resp.json();
                if (data.success) {
                    this.closeEditor();
                    await this.load();
                } else {
                    this.saveError = data.error || i18n.save_failed;
                }
            } catch (e) {
                console.error('保存模型配置失败:', e);
                this.saveError = i18n.save_failed + ': ' + e.message;
            } finally {
                this.saving = false;
            }
        },

        async deleteModel(name) {
            const i18n = window.Alpine.store('i18n');
            if (!confirm(i18n.delete_model_confirm + '「' + name + '」')) return;
            try {
                const resp = await fetch('/api/models/' + encodeURIComponent(name), {
                    method: 'DELETE',
                });
                const data = await resp.json();
                if (data.success) {
                    await this.load();
                } else {
                    alert(data.error || i18n.delete_failed);
                }
            } catch (e) {
                console.error('删除模型配置失败:', e);
                alert(i18n.delete_failed + ': ' + e.message);
            }
        },
    });

    // ── 会话列表 Store（C6+D1：sidebar/chat 单一数据源，消除重复请求与双份实现） ──
    window.Alpine.store('sessions', {
        list: [],
        loading: false,
        error: null,
        loaded: false,

        // 拉取会话列表。多次并发调用会被 loading 守卫合并为一次实际请求。
        async load() {
            if (this.loading) return;
            this.loading = true;
            this.error = null;
            try {
                const resp = await fetch('/api/sessions');
                if (!resp.ok) throw new Error('HTTP ' + resp.status);
                const data = await resp.json();
                this.list = Array.isArray(data) ? data : [];
                this.loaded = true;
            } catch (e) {
                console.error('加载会话列表失败:', e);
                this.list = [];
                this.error = e.message || '加载失败';
            } finally {
                this.loading = false;
            }
        },

        // 删除会话（后端删除成功后从本地列表移除）。返回是否成功。
        async remove(id) {
            try {
                const resp = await fetch('/api/sessions/' + encodeURIComponent(id), {
                    method: 'DELETE',
                });
                const data = await resp.json();
                if (data.deleted) {
                    this.list = this.list.filter((s) => s.id !== id);
                    return true;
                }
                console.error('删除失败:', data.error || '未知错误');
            } catch (e) {
                console.error('删除会话失败:', e);
            }
            return false;
        },

        // 重命名会话。成功时更新本地列表对应项并返回新标题，否则返回 null。
        async rename(id, title) {
            const trimmed = (title || '').trim();
            if (!trimmed) return null;
            try {
                const resp = await fetch('/api/sessions/' + encodeURIComponent(id) + '/rename', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ title: trimmed }),
                });
                const data = await resp.json();
                if (data.success) {
                    const s = this.list.find((x) => x.id === id);
                    if (s) s.title = data.title;
                    return data.title;
                }
                console.error('重命名失败:', data.error || '未知错误');
            } catch (e) {
                console.error('重命名会话失败:', e);
            }
            return null;
        },

        // ISO 时间 → "MM-DD HH:MM" 简短展示（纯函数，供侧栏复用）
        formatTime(iso) {
            if (!iso) return '';
            const d = new Date(iso);
            if (isNaN(d.getTime())) return iso;
            const pad = (n) => String(n).padStart(2, '0');
            return pad(d.getMonth() + 1) + '-' + pad(d.getDate()) + ' ' +
                pad(d.getHours()) + ':' + pad(d.getMinutes());
        },
    });

    // ── Token 用量 Store ──
    window.Alpine.store('tokenUsage', {
        prompt: 0,
        completion: 0,
        total: 0,

        reset() {
            this.prompt = 0;
            this.completion = 0;
            this.total = 0;
        },

        add(prompt, completion, total) {
            this.prompt += prompt || 0;
            this.completion += completion || 0;
            this.total += total || 0;
        },

        format(n) {
            if (!n) return '0';
            if (n < 1000) return String(n);
            return (n / 1000).toFixed(1) + 'K';
        },

        label() {
            if (!this.total) return '';
            return '🔤 ' + this.format(this.total) + ' tokens';
        },

        detail() {
            if (!this.total) return '';
            return 'Prompt: ' + this.format(this.prompt) +
                ' · Completion: ' + this.format(this.completion) +
                ' · Total: ' + this.format(this.total);
        },
    });

    // ── 主题 Store ──
    window.Alpine.store('theme', {
        theme: localStorage.getItem('dev-assistant-theme') || 'auto',
        systemDark: false,

        init() {
            this.systemDark = window.matchMedia &&
                window.matchMedia('(prefers-color-scheme: dark)').matches;
            this.apply();
        },

        get dark() {
            return this.theme === 'dark' ||
                (this.theme === 'auto' && this.systemDark);
        },

        label() {
            return this.dark ? '☀️' : '🌙';
        },

        modeLabel() {
            if (this.theme === 'auto') return '主题：自动（当前' + (this.dark ? '深色' : '浅色') + '）';
            return this.theme === 'dark' ? '主题：深色' : '主题：浅色';
        },

        apply() {
            document.documentElement.setAttribute('data-theme', this.dark ? 'dark' : 'light');
            const hlCss = document.getElementById('hljs-theme');
            if (hlCss) {
                hlCss.href = this.dark
                    ? 'https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/styles/github-dark.min.css'
                    : 'https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/styles/github.min.css';
            }
            // 同步 PWA 状态栏/浏览器主题色，避免亮暗切换后残留白色状态栏
            const meta = document.querySelector('meta[name="theme-color"]');
            if (meta) {
                meta.setAttribute('content', this.dark ? '#0f1117' : '#ffffff');
            }
        },

        toggle() {
            if (this.theme === 'auto') {
                this.theme = this.systemDark ? 'light' : 'dark';
            } else if (this.theme === 'dark') {
                this.theme = 'light';
            } else {
                this.theme = 'auto';
            }
            localStorage.setItem('dev-assistant-theme', this.theme);
            this.apply();
        },
    });

    // ── 侧栏抽屉状态（移动端用） ──
    window.Alpine.store('sidebar', {
        open: false,
        // 响应式移动端判断：随窗口尺寸实时更新（而非一次性读取 window.innerWidth）
        isMobile: window.matchMedia ? window.matchMedia('(max-width: 480px)').matches : false,

        toggle() {
            this.open = !this.open;
        },

        close() {
            this.open = false;
        },

        init() {
            if (!window.matchMedia) return;
            const mq = window.matchMedia('(max-width: 480px)');
            const update = (e) => { this.isMobile = e.matches; };
            // 兼容旧版 Safari 的 addListener / 新版 addEventListener
            if (typeof mq.addEventListener === 'function') {
                mq.addEventListener('change', update);
            } else if (typeof mq.addListener === 'function') {
                mq.addListener(update);
            }
        },
    });

    // ── 性能监控 Store（P5） ──
    window.Alpine.store('performance', {
        visible: false,
        fps: 0,
        memory: 'N/A',
        listeners: 0,

        toggle() {
            this.visible = !this.visible;
            if (this.visible) {
                this.startMonitoring();
            } else {
                this.stopMonitoring();
            }
        },

        startMonitoring() {
            if (this._monitoring) return;
            this._monitoring = true;
            this._lastTime = performance.now();
            this._frameCount = 0;
            this._tick();
        },

        stopMonitoring() {
            this._monitoring = false;
        },

        _tick() {
            if (!this._monitoring) return;
            this._frameCount++;
            const now = performance.now();
            const delta = now - this._lastTime;
            
            if (delta >= 1000) {
                this.fps = Math.round((this._frameCount * 1000) / delta);
                this._frameCount = 0;
                this._lastTime = now;
                
                if (performance.memory) {
                    const mb = Math.round(performance.memory.usedJSHeapSize / 1048576);
                    this.memory = mb + ' MB';
                } else {
                    this.memory = 'N/A';
                }
            }
            
            requestAnimationFrame(() => this._tick());
        },
    });

    // ── 主题定制 Store（P5） ──
    window.Alpine.store('customization', {
        fontSize: localStorage.getItem('dev-assistant-font-size') || '16',
        borderRadius: localStorage.getItem('dev-assistant-radius') || '8',
        primaryColor: localStorage.getItem('dev-assistant-primary') || '#3b82f6',

        updateFontSize(size) {
            this.fontSize = size;
            localStorage.setItem('dev-assistant-font-size', size);
            document.documentElement.style.setProperty('--pico-font-size', size + 'px');
        },

        updateRadius(radius) {
            this.borderRadius = radius;
            localStorage.setItem('dev-assistant-radius', radius);
            document.documentElement.style.setProperty('--pico-border-radius', radius + 'px');
        },

        updatePrimaryColor(color) {
            this.primaryColor = color;
            localStorage.setItem('dev-assistant-primary', color);
            document.documentElement.style.setProperty('--pico-primary', color);
        },
    });
});
