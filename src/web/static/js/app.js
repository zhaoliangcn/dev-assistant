// ============================================================
// Dev-Assistant Web UI — Alpine.js 聊天组件
// ============================================================
// 依赖：Alpine.js 3.x + highlight.js（由 base.html 引入）
// 功能：WS 连接/重连、消息渲染（markdown + 语法高亮）、
//       可替换状态条、assistant 消息流式增量更新。

// W7: 全局状态 store — header 在线指示器与主题按钮位于 chatApp 的
// x-data 作用域之外，因此连接/主题状态通过 Alpine store 共享。
document.addEventListener('alpine:init', () => {
    if (!window.Alpine) return;

    window.Alpine.store('connection', { connected: false });

    // W15: 模型列表与切换 store（header 下拉框使用）
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

        // 当前主题图标（按钮显示）
        label() {
            return this.dark ? '☀️' : '🌙';
        },

        // 主题模式提示（按钮 title）：自动 / 深色 / 浅色
        modeLabel() {
            if (this.theme === 'auto') return '主题：自动（当前' + (this.dark ? '深色' : '浅色') + '）';
            return this.theme === 'dark' ? '主题：深色' : '主题：浅色';
        },

        // 应用 data-theme + 同步 highlight.js 主题样式表
        apply() {
            document.documentElement.setAttribute('data-theme', this.dark ? 'dark' : 'light');
            const hlCss = document.getElementById('hljs-theme');
            if (hlCss) {
                hlCss.href = this.dark
                    ? 'https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/styles/github-dark.min.css'
                    : 'https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/styles/github.min.css';
            }
        },

        // 循环切换：auto → dark → light → auto
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
});

function chatApp() {
    let wsInstance = null;
    let reconnectTimer = null;
    let reconnectAttempts = 0;

    return {
        connected: false,
        sessionId: null,
        messageId: 0,
        input: '',
        // C2: 对话区消息（user/assistant/system/error）
        chatMessages: [],
        // C2: 工具活动区消息（tool-call/tool-result）
        toolMessages: [],
        // 消息序号（渲染 :key 用，保证流式替换时 DOM 稳定）
        _msgSeq: 0,
        // W10: 是否正在生成（停止按钮显示）
        busy: false,
        // S2: 会话历史列表
        sessions: [],
        loadingSessions: false,
        activeSessionId: null,
        // S4: 会话重命名编辑态
        renamingId: null,
        renameTitle: '',
        // 状态条：thinking/status 事件显示在此（可替换，不进入消息列表）
        pendingStatus: null,

        init() {
            this.connectWS();
            this.loadSessions();
            // 加载模型列表（header 下拉框）
            if (window.Alpine) {
                window.Alpine.store('models').load();
                window.Alpine.store('theme').init();
            }
        },

        // ── 会话历史（S2） ────────────────────────────────────────────

        async loadSessions() {
            this.loadingSessions = true;
            try {
                const resp = await fetch('/api/sessions');
                const data = await resp.json();
                this.sessions = Array.isArray(data) ? data : [];
            } catch (e) {
                console.error('加载会话列表失败:', e);
                this.sessions = [];
            } finally {
                this.loadingSessions = false;
            }
        },

        // 切换会话：加载历史事件并渲染为消息（只读展示）
        async selectSession(id) {
            try {
                const resp = await fetch('/api/sessions/' + encodeURIComponent(id));
                const data = await resp.json();
                this.activeSessionId = id;
                const all = this.eventsToMessages(data.events || []);
                this.chatMessages = all.filter((m) => m.role !== 'tool-call' && m.role !== 'tool-result');
                this.toolMessages = all.filter((m) => m.role === 'tool-call' || m.role === 'tool-result');
                this.pendingStatus = null;
                this.scrollToBottomLater();
                this.scrollToolLater();
            } catch (e) {
                console.error('加载会话详情失败:', e);
            }
        },

        // 新对话：清空当前消息与选中态
        newChat() {
            this.chatMessages = [];
            this.toolMessages = [];
            this.activeSessionId = null;
            this.pendingStatus = null;
        },

        // 删除会话（前端乐观移除，失败时重新加载）
        async deleteSession(id) {
            if (!confirm('确定删除该会话？')) return;
            try {
                const resp = await fetch('/api/sessions/' + encodeURIComponent(id), {
                    method: 'DELETE',
                });
                const data = await resp.json();
                if (data.deleted) {
                    this.sessions = this.sessions.filter((s) => s.id !== id);
                    if (this.activeSessionId === id) {
                        this.newChat();
                    }
                } else {
                    console.error('删除失败:', data.error || '未知错误');
                }
            } catch (e) {
                console.error('删除会话失败:', e);
            }
        },

        // S4: 进入重命名编辑态（预填当前标题）
        startRename(session) {
            this.renamingId = session.id;
            this.renameTitle = session.title || '';
        },

        // S4: 取消重命名
        cancelRename() {
            this.renamingId = null;
            this.renameTitle = '';
        },

        // S4: 提交重命名（调用后端 /rename，成功后更新本地列表）
        async submitRename(id) {
            const title = this.renameTitle.trim();
            if (!title) {
                this.cancelRename();
                return;
            }
            try {
                const resp = await fetch('/api/sessions/' + encodeURIComponent(id) + '/rename', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ title: title }),
                });
                const data = await resp.json();
                if (data.success) {
                    const s = this.sessions.find((x) => x.id === id);
                    if (s) s.title = data.title;
                } else {
                    console.error('重命名失败:', data.error || '未知错误');
                }
            } catch (e) {
                console.error('重命名会话失败:', e);
            } finally {
                this.cancelRename();
            }
        },

        // 将持久化事件（serde tag=type, snake_case）映射为消息
        eventsToMessages(events) {
            return events
                .filter((ev) => ev && ev.type)
                .map((ev) => {
                    switch (ev.type) {
                        case 'user_message':
                            return { role: 'user', content: ev.content || '', time: ev.timestamp };
                        case 'assistant_message':
                            return { role: 'assistant', content: ev.content || '', time: ev.timestamp };
                        case 'system_message':
                            return { role: 'system', content: ev.content || '', time: ev.timestamp };
                        case 'tool_call_request':
                            return { role: 'tool-call', content: ev.name || '工具调用', time: ev.timestamp };
                        case 'tool_result':
                            return { role: 'tool-result', content: ev.content || '', time: ev.timestamp };
                        default:
                            return null;
                    }
                })
                .filter((m) => m !== null);
        },

        // 格式化会话时间（ISO → 月-日 时:分）
        formatSessionTime(iso) {
            if (!iso) return '';
            const d = new Date(iso);
            if (isNaN(d.getTime())) return iso;
            const pad = (n) => String(n).padStart(2, '0');
            return pad(d.getMonth() + 1) + '-' + pad(d.getDate()) + ' ' +
                pad(d.getHours()) + ':' + pad(d.getMinutes());
        },

        // ── 连接状态 ──────────────────────────────────────────────────

        // W7: 同步连接状态到全局 store（header 指示器）
        setConnected(v) {
            if (window.Alpine) {
                window.Alpine.store('connection').connected = v;
            }
        },

        // ── WebSocket ─────────────────────────────────────────────────

        connectWS() {
            const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
            const url = protocol + '//' + window.location.host + '/ws/chat';

            if (wsInstance) {
                wsInstance.onclose = null;
                wsInstance.onerror = null;
                if (wsInstance.readyState === WebSocket.OPEN ||
                    wsInstance.readyState === WebSocket.CONNECTING) {
                    wsInstance.close();
                }
            }

            const self = this;
            wsInstance = new WebSocket(url);

            wsInstance.onopen = () => {
                self.connected = true;
                reconnectAttempts = 0;
                self.setConnected(true);
            };

            wsInstance.onclose = () => {
                self.connected = false;
                self.setConnected(false);
                if (reconnectAttempts < 10) {
                    const delay = Math.min(1000 * Math.pow(2, reconnectAttempts), 30000);
                    reconnectAttempts++;
                    if (reconnectTimer) clearTimeout(reconnectTimer);
                    reconnectTimer = setTimeout(() => self.connectWS(), delay + Math.random() * 1000);
                }
            };

            wsInstance.onerror = () => {};

            wsInstance.onmessage = (event) => {
                try {
                    const msg = JSON.parse(event.data);
                    self.handleServerEvent(msg);
                } catch (e) {
                    console.error('WebSocket 消息解析失败:', e);
                }
            };
        },

        // ── 服务端事件处理 ────────────────────────────────────────────

        // 追加消息并附带时间戳（消息头显示用）。
        // C2: 按角色分流——tool-call/tool-result 进 toolMessages，
        // 其余进 chatMessages。
        addMessage(role, content, extra) {
            const item = Object.assign({
                role: role,
                content: content,
                time: new Date().toLocaleTimeString('zh-CN', { hour12: false }),
                _id: 'm' + (++this._msgSeq),
            }, extra || {});
            if (role === 'tool-call' || role === 'tool-result') {
                this.toolMessages = [...this.toolMessages, item];
                this.scrollToolLater();
            } else {
                this.chatMessages = [...this.chatMessages, item];
                this.scrollToBottomLater();
            }
        },

        // B25: 始终聚焦助手输出——对话区消息更新后滚动到底部。
        // 在 $nextTick（DOM 补丁完成）后再包一层 requestAnimationFrame，
        // 确保浏览器已 reflow，scrollHeight 为最新值，滚动必然到位。
        scrollToBottomLater() {
            this.$nextTick(() => {
                requestAnimationFrame(() => {
                    const el = this.$refs.list;
                    if (!el) return;
                    el.scrollTop = el.scrollHeight;
                });
            });
        },

        // C2: 工具活动区滚动到底部（独立容器）
        scrollToolLater() {
            this.$nextTick(() => {
                requestAnimationFrame(() => {
                    const el = this.$refs.toolList;
                    if (!el) return;
                    el.scrollTop = el.scrollHeight;
                });
            });
        },

        handleServerEvent(msg) {
            switch (msg.type) {
                case 'session_ready':
                    this.sessionId = msg.session_id;
                    break;
                case 'status':
                case 'thinking':
                    // W2: 状态/思考显示为可替换状态条，不进入消息列表
                    this.pendingStatus = { role: msg.type, content: msg.content };
                    break;
                case 'tool_call':
                    this.pendingStatus = null;
                    this.addMessage('tool-call', msg.tool_name || '操作');
                    break;
                case 'tool_result':
                    this.pendingStatus = null;
                    this.addMessage('tool-result', msg.content);
                    break;
                case 'assistant_message':
                    this.pendingStatus = null;
                    this.appendAssistant(msg);
                    break;
                case 'error':
                    this.pendingStatus = null;
                    this.busy = false;
                    this.addMessage('error', msg.content);
                    break;
                case 'done':
                    this.pendingStatus = null;
                    this.busy = false;
                    this.finishStreaming();
                    break;
            }
        },

        // 流式结束后移除最后一条 assistant 消息的 streaming 边框
        finishStreaming() {
            const idx = this.lastAssistantIndex();
            if (idx >= 0 && this.chatMessages[idx].streaming) {
                this.chatMessages[idx] = {
                    role: 'assistant',
                    content: this.chatMessages[idx].content,
                    streaming: false,
                    time: this.chatMessages[idx].time,
                };
            }
        },

        // W3: assistant 消息流式增量更新。
        // 服务端 `streaming: true` 事件携带的是**累积的完整内容**（见
        // MessageOutput::streaming_assistant 约定），因此前端只需替换
        // 最后一条 assistant 消息的 content，无需字符串拼接。
        // C2: 工具消息已在独立容器（toolMessages），对话区 assistant
        // 始终位于 chatMessages 末尾，无需再移动。
        appendAssistant(msg) {
            if (msg.streaming) {
                const idx = this.lastAssistantIndex();
                if (idx >= 0) {
                    const existing = this.chatMessages[idx];
                    this.chatMessages[idx] = {
                        role: 'assistant',
                        content: msg.content,
                        streaming: true,
                        time: existing.time,
                    };
                    // 流式跟随：始终聚焦助手输出，滚动到底部
                    this.scrollToBottomLater();
                    return;
                }
                // 找不到进行中的 assistant 消息则按完整消息追加
            }
            this.addMessage('assistant', msg.content);
        },

        lastAssistantIndex() {
            for (let i = this.chatMessages.length - 1; i >= 0; i--) {
                if (this.chatMessages[i].role === 'assistant') return i;
            }
            return -1;
        },

        sendMessage() {
            const text = this.input.trim();
            if (!text || this.busy) return;

            // 处于历史会话查看态时，发送消息自动开启新对话
            if (this.activeSessionId) {
                this.newChat();
            }

            this.addMessage('user', text);
            this.input = '';
            this.busy = true;
            // 用户主动发送：滚动到底部
            this.scrollToBottomLater();

            if (wsInstance && wsInstance.readyState === WebSocket.OPEN) {
                wsInstance.send(JSON.stringify({
                    type: 'user_message',
                    content: text,
                    id: 'msg_' + Date.now() + '_' + (this.messageId++)
                }));
            }
        },

        // W10: 停止生成 — 发送 Cancel 事件，服务端中断 agent 任务并回 done
        stopGeneration() {
            if (!wsInstance || wsInstance.readyState !== WebSocket.OPEN) {
                this.busy = false;
                return;
            }
            this.pendingStatus = null;
            this.busy = false;
            wsInstance.send(JSON.stringify({
                type: 'cancel',
                message_id: 'msg_' + Date.now() + '_' + (this.messageId++)
            }));
        },

        // W8: textarea 按键处理 — Enter 发送，Shift+Enter 换行。
        // isComposing 排除中文输入法选字时的 Enter 确认。
        onInputKeydown(event) {
            if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
                event.preventDefault();
                this.sendMessage();
            }
        },

        // ── Markdown 渲染（W1） ────────────────────────────────────────

        // 安全转义：所有用户/模型内容先转义再注入 x-html
        escapeHtml(text) {
            return text
                .replace(/&/g, '&amp;')
                .replace(/</g, '&lt;')
                .replace(/>/g, '&gt;')
                .replace(/"/g, '&quot;');
        },

        // 行内格式：行内码 / 粗体 / 链接（在已转义文本上做结构化替换）
        inlineFormat(line) {
            let out = this.escapeHtml(line);
            // 行内代码 `x` — 最优先，避免被后续规则误伤
            out = out.replace(/`([^`]+)`/g, (_, code) => '<code>' + code + '</code>');
            // 粗体 **x**
            out = out.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
            // 链接 [text](url) — 仅允许 http/https 协议，防 javascript: 注入
            out = out.replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g,
                (_, text, url) => '<a href="' + url + '" target="_blank" rel="noopener">' + text + '</a>');
            return out;
        },

        // 代码块高亮：优先用 highlight.js；未加载时回退为转义文本。
        // 外层包复制按钮（onclick 委托给全局 copyCode）。
        // W13: 超过 20 行的长代码块默认折叠，头部显示"展开"按钮。
        highlightCode(code, lang) {
            const escaped = this.escapeHtml(code);
            let html;
            if (typeof window.hljs === 'undefined') {
                html = escaped;
            } else {
                try {
                    const detected = lang && window.hljs.getLanguage(lang)
                        ? { language: lang }
                        : {};
                    html = window.hljs.highlight(code, detected).value;
                } catch (e) {
                    html = escaped;
                }
            }
            const cls = lang ? ' class="language-' + lang + '"' : '';
            const langLabel = lang ? lang : 'code';
            const lineCount = code.split('\n').length;
            const collapsible = lineCount > 20;
            const actions = (collapsible
                ? '<button type="button" class="copy-btn" onclick="toggleCodeBlock(this)">展开</button>'
                : '') +
                '<button type="button" class="copy-btn" onclick="copyCode(this)">📋 复制</button>';
            return '<div class="code-block' + (collapsible ? ' code-block-collapsed' : '') + '">' +
                '<div class="code-block-header">' +
                '<span class="code-block-lang">' + this.escapeHtml(langLabel) + '</span>' +
                '<div class="code-block-actions">' + actions + '</div>' +
                '</div>' +
                '<pre><code' + cls + '>' + html + '</code></pre>' +
                '</div>';
        },

        // W6: 复制消息全文（消息头复制按钮）
        copyMessage(content) {
            const self = this;
            copyTextToClipboard(content).then(() => {
                self.copiedMessageId = Date.now();
                setTimeout(() => { self.copiedMessageId = null; }, 1500);
            });
        },

        formatContent(content) {
            if (!content) return '';
            const lines = content.split('\n');
            const out = [];
            let i = 0;

            while (i < lines.length) {
                const line = lines[i];

                // 围栏代码块 ```lang ... ```
                const fence = line.match(/^```(\w*)/);
                if (fence) {
                    const lang = fence[1];
                    const buf = [];
                    i++;
                    while (i < lines.length && !lines[i].startsWith('```')) {
                        buf.push(lines[i]);
                        i++;
                    }
                    i++; // 跳过结束围栏
                    out.push(this.highlightCode(buf.join('\n'), lang));
                    continue;
                }

                // 标题 # ~ ######
                const heading = line.match(/^(#{1,6})\s+(.*)$/);
                if (heading) {
                    const level = Math.min(heading[1].length + 1, 6);
                    out.push('<h' + level + '>' + this.inlineFormat(heading[2]) + '</h' + level + '>');
                    i++;
                    continue;
                }

                // 引用 > text
                if (line.startsWith('>')) {
                    const buf = [];
                    while (i < lines.length && lines[i].startsWith('>')) {
                        buf.push(this.inlineFormat(lines[i].replace(/^>\s?/, '')));
                        i++;
                    }
                    out.push('<blockquote>' + buf.join('<br>') + '</blockquote>');
                    continue;
                }

                // 无序列表 - / * / + item
                const ul = line.match(/^[-*+]\s+(.*)$/);
                if (ul) {
                    const items = [];
                    while (i < lines.length) {
                        const m = lines[i].match(/^[-*+]\s+(.*)$/);
                        if (!m) break;
                        items.push('<li>' + this.inlineFormat(m[1]) + '</li>');
                        i++;
                    }
                    out.push('<ul>' + items.join('') + '</ul>');
                    continue;
                }

                // 有序列表 1. item
                const ol = line.match(/^\d+\.\s+(.*)$/);
                if (ol) {
                    const items = [];
                    while (i < lines.length) {
                        const m = lines[i].match(/^\d+\.\s+(.*)$/);
                        if (!m) break;
                        items.push('<li>' + this.inlineFormat(m[1]) + '</li>');
                        i++;
                    }
                    out.push('<ol>' + items.join('') + '</ol>');
                    continue;
                }

                // 表格 | a | b |（含分隔行 | --- | --- |）
                if (line.startsWith('|') && lines[i + 1] && /^\|[\s:|-]+\|/.test(lines[i + 1])) {
                    const rows = [];
                    while (i < lines.length && lines[i].startsWith('|')) {
                        const cells = lines[i]
                            .trim()
                            .replace(/^\||\|$/g, '')
                            .split('|')
                            .map((c) => c.trim());
                        rows.push(cells);
                        i++;
                    }
                    if (rows.length > 1) {
                        // 第二行是分隔行（| --- | --- |），跳过
                        const header = rows[0];
                        const body = rows.slice(2);
                        let html = '<table><thead><tr>';
                        header.forEach((c) => { html += '<th>' + this.inlineFormat(c) + '</th>'; });
                        html += '</tr></thead><tbody>';
                        body.forEach((row) => {
                            html += '<tr>';
                            header.forEach((_, ci) => {
                                html += '<td>' + this.inlineFormat(row[ci] || '') + '</td>';
                            });
                            html += '</tr>';
                        });
                        html += '</tbody></table>';
                        out.push(html);
                    }
                    continue;
                }

                // 空行
                if (line.trim() === '') {
                    if (out.length && !out[out.length - 1].endsWith('<br>')) {
                        out.push('<br>');
                    }
                    i++;
                    continue;
                }

                // 普通段落
                out.push('<p>' + this.inlineFormat(line) + '</p>');
                i++;
            }

            return out.join('\n');
        }
    };
}

// ── 文件浏览器组件（W11 / N1-N3） ────────────────────────────────
// 对接现有 /api/files* 接口，替代依赖未加载 htmx 的半成品模板。
// 支持：新建文件、编辑、Markdown/代码预览、修改的 diff 预览。

function fileExplorer() {
    return {
        entries: [],
        currentPath: '.',
        activePath: null,
        content: '',
        // 打开时的原始内容（diff 对比基线）
        originalContent: '',
        modified: false,
        saved: false,
        saving: false,
        loading: false,
        // 视图模式：edit / preview / diff
        viewMode: 'edit',
        previewHtml: '',
        diffLines: [],
        diffStats: null,

        async loadDir(path) {
            this.loading = true;
            this.currentPath = path;
            try {
                const resp = await fetch('/api/files?path=' + encodeURIComponent(path));
                const data = await resp.json();
                this.entries = data.entries || [];
            } catch (e) {
                console.error('加载目录失败:', e);
                this.entries = [];
            } finally {
                this.loading = false;
            }
        },

        // 上一级目录路径
        parentPath() {
            if (!this.currentPath || this.currentPath === '.') return '.';
            const idx = this.currentPath.lastIndexOf('/');
            const parent = idx <= 0 ? '.' : this.currentPath.slice(0, idx);
            return parent || '.';
        },

        // N1: 新建文件 — 提示输入路径，直接进入编辑（空内容）
        async newFile() {
            const name = prompt('输入新文件路径（相对项目目录，如 src/foo.rs）:', '');
            if (!name || !name.trim()) return;
            const path = name.trim().replace(/^\.\//, '');
            this.activePath = path;
            this.content = '';
            this.originalContent = '';
            this.modified = false;
            this.saved = false;
            this.viewMode = 'edit';
            this.previewHtml = '';
            this.diffLines = [];
            this.diffStats = null;
            // 进入所在目录便于保存后看到文件
            const idx = path.lastIndexOf('/');
            if (idx > 0) {
                this.loadDir(path.slice(0, idx));
            }
        },

        async openFile(path) {
            this.activePath = path;
            this.modified = false;
            this.saved = false;
            this.viewMode = 'edit';
            try {
                const resp = await fetch('/api/files/content?path=' + encodeURIComponent(path));
                const data = await resp.json();
                this.content = data.content || '';
                this.originalContent = this.content;
                this.previewHtml = '';
                this.diffLines = [];
                this.diffStats = null;
            } catch (e) {
                console.error('加载文件失败:', e);
                this.content = '';
                this.originalContent = '';
            }
        },

        onEdit() {
            this.modified = true;
            this.saved = false;
        },

        async saveFile() {
            if (!this.activePath || this.saving) return;
            this.saving = true;
            try {
                const resp = await fetch('/api/files/save', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ path: this.activePath, content: this.content }),
                });
                const data = await resp.json();
                if (data.success) {
                    this.originalContent = this.content;
                    this.modified = false;
                    this.saved = true;
                    setTimeout(() => { this.saved = false; }, 1500);
                } else {
                    console.error('保存失败:', data.error);
                }
            } catch (e) {
                console.error('保存失败:', e);
            } finally {
                this.saving = false;
            }
        },

        // 状态文案：新建 / 已修改 / 已保存
        statusText() {
            if (this.saved) return '✅ 已保存';
            if (this.modified) return '📝 已修改';
            if (this.isNewFile()) return '🆕 新建文件';
            return '';
        },

        isNewFile() {
            return this.originalContent === '' && this.modified === false &&
                this.activePath !== null;
        },

        // N2: 切换视图；进入 preview/diff 时按需生成内容
        switchView(mode) {
            this.viewMode = mode;
            if (mode === 'preview') {
                this.previewHtml = this.renderPreview();
            } else if (mode === 'diff') {
                this.computeDiff();
            }
        },

        // N2: 预览渲染 — Markdown 用 chatApp 的渲染器（消息区复用），
        // 其他类型做 HTML 转义 + 代码高亮
        renderPreview() {
            const path = this.activePath || '';
            const isMarkdown = /\.(md|markdown|mdown)$/i.test(path);
            if (isMarkdown) {
                // 复用聊天组件的 markdown 渲染（生成器函数）
                return this.markdownToHtml(this.content);
            }
            // 代码/文本：转义后包 <pre>，交给 highlight.js 高亮
            const escaped = escapeHtmlText(this.content);
            if (typeof window.hljs !== 'undefined' && this.content.trim()) {
                try {
                    const langMatch = path.match(/\.([a-zA-Z0-9]+)$/);
                    const lang = langMatch ? langMatch[1] : '';
                    const detected = lang && window.hljs.getLanguage(lang)
                        ? { language: lang }
                        : {};
                    const highlighted = window.hljs.highlight(this.content, detected).value;
                    const cls = lang ? ' class="language-' + lang + '"' : '';
                    return '<pre class="preview-code"><code' + cls + '>' + highlighted + '</code></pre>';
                } catch (e) {
                    return '<pre class="preview-code"><code>' + escaped + '</code></pre>';
                }
            }
            return '<pre class="preview-code"><code>' + escaped + '</code></pre>';
        },

        // 简版 Markdown 渲染（标题/列表/引用/代码块/粗体/行内码），
        // 与聊天消息渲染保持一致体验。
        markdownToHtml(text) {
            if (!text) return '<p class="file-empty">（空文件）</p>';
            const lines = text.split('\n');
            const out = [];
            let i = 0;
            while (i < lines.length) {
                const line = lines[i];
                const fence = line.match(/^```(\w*)/);
                if (fence) {
                    const lang = fence[1];
                    const buf = [];
                    i++;
                    while (i < lines.length && !lines[i].startsWith('```')) {
                        buf.push(lines[i]);
                        i++;
                    }
                    i++;
                    const code = buf.join('\n');
                    const escaped = escapeHtmlText(code);
                    let html = escaped;
                    if (typeof window.hljs !== 'undefined') {
                        try {
                            const detected = lang && window.hljs.getLanguage(lang)
                                ? { language: lang } : {};
                            html = window.hljs.highlight(code, detected).value;
                        } catch (e) { /* fallback */ }
                    }
                    const cls = lang ? ' class="language-' + lang + '"' : '';
                    out.push('<pre><code' + cls + '>' + html + '</code></pre>');
                    continue;
                }
                const heading = line.match(/^(#{1,6})\s+(.*)$/);
                if (heading) {
                    const level = Math.min(heading[1].length + 1, 6);
                    out.push('<h' + level + '>' + inlineMdFormat(heading[2]) + '</h' + level + '>');
                    i++;
                    continue;
                }
                if (line.startsWith('>')) {
                    out.push('<blockquote>' + inlineMdFormat(line.replace(/^>\s?/, '')) + '</blockquote>');
                    i++;
                    continue;
                }
                const ul = line.match(/^[-*+]\s+(.*)$/);
                if (ul) {
                    const items = [];
                    while (i < lines.length) {
                        const m = lines[i].match(/^[-*+]\s+(.*)$/);
                        if (!m) break;
                        items.push('<li>' + inlineMdFormat(m[1]) + '</li>');
                        i++;
                    }
                    out.push('<ul>' + items.join('') + '</ul>');
                    continue;
                }
                if (line.trim() === '') {
                    if (out.length && !out[out.length - 1].endsWith('<br>')) out.push('<br>');
                    i++;
                    continue;
                }
                out.push('<p>' + inlineMdFormat(line) + '</p>');
                i++;
            }
            return out.join('\n');
        },

        // N2: LCS 行级 diff — 对比 originalContent 与 content
        computeDiff() {
            const oldLines = (this.originalContent || '').split('\n');
            const newLines = this.content.split('\n');
            const result = lcsDiff(oldLines, newLines);
            this.diffLines = result.lines;
            this.diffStats = result.stats;
        },

        formatSize(bytes) {
            if (bytes < 1024) return bytes + ' B';
            if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
            return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
        },
    };
}

// ── 全局文件工具（N2） ────────────────────────────────────────────

// HTML 转义（预览用）
function escapeHtmlText(text) {
    return String(text)
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;');
}

// 行内 markdown 格式：行内码 / 粗体 / 链接
function inlineMdFormat(line) {
    let out = escapeHtmlText(line);
    out = out.replace(/`([^`]+)`/g, (_, code) => '<code>' + code + '</code>');
    out = out.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
    out = out.replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g,
        (_, text, url) => '<a href="' + url + '" target="_blank" rel="noopener">' + text + '</a>');
    return out;
}

// LCS 最长公共子序列 diff，返回带类型标记的行与统计
function lcsDiff(oldLines, newLines) {
    const n = oldLines.length;
    const m = newLines.length;
    // dp[i][j] = oldLines[i..] 与 newLines[j..] 的 LCS 长度
    const dp = [];
    for (let i = 0; i <= n; i++) {
        dp.push(new Array(m + 1).fill(0));
    }
    for (let i = n - 1; i >= 0; i--) {
        for (let j = m - 1; j >= 0; j--) {
            if (oldLines[i] === newLines[j]) {
                dp[i][j] = dp[i + 1][j + 1] + 1;
            } else {
                dp[i][j] = Math.max(dp[i + 1][j], dp[i][j + 1]);
            }
        }
    }

    const lines = [];
    let i = 0;
    let j = 0;
    let add = 0;
    let del = 0;
    let ctx = 0;
    while (i < n && j < m) {
        if (oldLines[i] === newLines[j]) {
            lines.push({ type: 'ctx', marker: ' ', text: oldLines[i] });
            ctx++;
            i++;
            j++;
        } else if (dp[i + 1][j] >= dp[i][j + 1]) {
            lines.push({ type: 'del', marker: '-', text: oldLines[i] });
            del++;
            i++;
        } else {
            lines.push({ type: 'add', marker: '+', text: newLines[j] });
            add++;
            j++;
        }
    }
    while (i < n) {
        lines.push({ type: 'del', marker: '-', text: oldLines[i] });
        del++;
        i++;
    }
    while (j < m) {
        lines.push({ type: 'add', marker: '+', text: newLines[j] });
        add++;
        j++;
    }

    return { lines, stats: { add, del, ctx } };
}

// ── 全局复制工具（W6） ─────────────────────────────────────────────

// 异步复制文本到剪贴板；回退到 document.execCommand（旧浏览器/非 https）
function copyTextToClipboard(text) {
    if (navigator.clipboard && window.isSecureContext) {
        return navigator.clipboard.writeText(text).catch(() => fallbackCopy(text));
    }
    return Promise.resolve(fallbackCopy(text));
}

function fallbackCopy(text) {
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.style.position = 'fixed';
    ta.style.opacity = '0';
    document.body.appendChild(ta);
    ta.select();
    try {
        document.execCommand('copy');
    } catch (e) {
        console.error('复制失败:', e);
    }
    document.body.removeChild(ta);
}

// 代码块复制按钮：从按钮向上找到 .code-block 容器内的 <code> 文本
function copyCode(btn) {
    const block = btn.closest('.code-block');
    if (!block) return;
    const code = block.querySelector('pre code');
    const text = code ? code.innerText : '';
    copyTextToClipboard(text).then(() => {
        btn.textContent = '✅ 已复制';
        setTimeout(() => { btn.textContent = '📋 复制'; }, 1500);
    });
}

// W13: 展开/折叠长代码块（切换 .code-block-collapsed 类）
function toggleCodeBlock(btn) {
    const block = btn.closest('.code-block');
    if (!block) return;
    const collapsed = block.classList.toggle('code-block-collapsed');
    btn.textContent = collapsed ? '展开' : '收起';
}


