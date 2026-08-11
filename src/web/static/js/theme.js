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

        async load() {
            try {
                const resp = await fetch('/api/models');
                const data = await resp.json();
                this.list = Array.isArray(data) ? data : [];
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
